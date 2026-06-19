use crate::{IoOptions, WriterStrategy};
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::time::Instant;

const PREFETCH_MIN_BYTES: u64 = 4 * 1024 * 1024;

pub(crate) enum PayloadSource {
    Memory(Vec<u8>),
    CachedFile { path: PathBuf, len: u64 },
}

#[derive(Debug, Clone, Default)]
pub(crate) struct WriterReport {
    pub strategy: WriterStrategy,
    pub preallocated_bytes: u64,
    pub cached_payload_open_count: usize,
    pub cached_payload_read_bytes: u64,
    pub prefetched_bytes: u64,
    pub peak_pipeline_memory_bytes: u64,
    pub payload_read_ms: u128,
    pub payload_write_ms: u128,
    pub writer_wait_ms: u128,
    pub flush_ms: u128,
    pub rename_ms: u128,
}

pub(crate) struct ArchiveWriter {
    final_path: PathBuf,
    temp_path: PathBuf,
    writer: Option<BufWriter<File>>,
    expected_len: u64,
    written: u64,
    committed: bool,
    options: IoOptions,
    report: WriterReport,
}

impl ArchiveWriter {
    pub(crate) fn create(
        final_path: &Path,
        expected_len: u64,
        options: IoOptions,
    ) -> anyhow::Result<Self> {
        if options.writer_buffer_bytes == 0
            || options.transfer_chunk_bytes == 0
            || options.prefetch_depth == 0
        {
            anyhow::bail!("I/O buffer sizes and prefetch depth must be non-zero");
        }
        let parent = final_path.parent().unwrap_or_else(|| Path::new("."));
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
        let temp_path = unique_temp_path(final_path);
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)?;
        file.set_len(expected_len)?;
        Ok(Self {
            final_path: final_path.to_path_buf(),
            temp_path,
            writer: Some(BufWriter::with_capacity(options.writer_buffer_bytes, file)),
            expected_len,
            written: 0,
            committed: false,
            options,
            report: WriterReport {
                preallocated_bytes: expected_len,
                ..WriterReport::default()
            },
        })
    }

    pub(crate) fn write_payloads(&mut self, payloads: &[PayloadSource]) -> anyhow::Result<()> {
        let cached_count = payloads
            .iter()
            .filter(|payload| matches!(payload, PayloadSource::CachedFile { .. }))
            .count();
        let cached_bytes = payloads.iter().try_fold(0_u64, |total, payload| {
            let len = match payload {
                PayloadSource::Memory(_) => 0,
                PayloadSource::CachedFile { len, .. } => *len,
            };
            total
                .checked_add(len)
                .ok_or_else(|| anyhow::anyhow!("cached payload length overflow"))
        })?;
        if cached_count >= 2 && cached_bytes >= PREFETCH_MIN_BYTES {
            self.report.strategy = WriterStrategy::PrefetchedCachedFiles;
            self.write_prefetched(payloads)
        } else {
            self.report.strategy = WriterStrategy::Buffered;
            self.write_buffered(payloads)
        }
    }

    pub(crate) fn finish(mut self) -> anyhow::Result<WriterReport> {
        if self.written != self.expected_len {
            anyhow::bail!(
                "archive length mismatch: wrote {}, expected {}",
                self.written,
                self.expected_len
            );
        }
        let flush_started = Instant::now();
        let mut writer = self
            .writer
            .take()
            .ok_or_else(|| anyhow::anyhow!("archive writer already finished"))?;
        writer.flush()?;
        let file = writer
            .into_inner()
            .map_err(|error| anyhow::anyhow!("archive flush failed: {}", error.error()))?;
        self.report.flush_ms = flush_started.elapsed().as_millis();
        drop(file);

        let rename_started = Instant::now();
        fs::rename(&self.temp_path, &self.final_path)?;
        self.report.rename_ms = rename_started.elapsed().as_millis();
        self.committed = true;
        Ok(self.report.clone())
    }

    fn write_buffered(&mut self, payloads: &[PayloadSource]) -> anyhow::Result<()> {
        let mut buffer = vec![0_u8; self.options.transfer_chunk_bytes];
        self.report.peak_pipeline_memory_bytes = buffer.len() as u64;
        for payload in payloads {
            match payload {
                PayloadSource::Memory(bytes) => self.write_payload_bytes(bytes)?,
                PayloadSource::CachedFile { path, len } => {
                    validate_cached_file(path, *len)?;
                    let mut input = File::open(path)?;
                    self.report.cached_payload_open_count += 1;
                    let mut remaining = *len;
                    while remaining > 0 {
                        let limit = remaining.min(buffer.len() as u64) as usize;
                        let read_started = Instant::now();
                        input.read_exact(&mut buffer[..limit])?;
                        self.report.payload_read_ms += read_started.elapsed().as_millis();
                        self.write_payload_bytes(&buffer[..limit])?;
                        self.report.cached_payload_read_bytes += limit as u64;
                        remaining -= limit as u64;
                    }
                }
            }
        }
        Ok(())
    }

    fn write_prefetched(&mut self, payloads: &[PayloadSource]) -> anyhow::Result<()> {
        let cached = payloads
            .iter()
            .enumerate()
            .filter_map(|(index, payload)| match payload {
                PayloadSource::CachedFile { path, len } => Some((index, path.clone(), *len)),
                PayloadSource::Memory(_) => None,
            })
            .collect::<Vec<_>>();
        for (_, path, len) in &cached {
            validate_cached_file(path, *len)?;
        }

        let chunk_size = self.options.transfer_chunk_bytes;
        let pool_size = self.options.prefetch_depth + 1;
        self.report.peak_pipeline_memory_bytes = (chunk_size * pool_size) as u64;
        let (data_tx, data_rx) = sync_channel(self.options.prefetch_depth);
        let (free_tx, free_rx) = sync_channel(pool_size);
        for _ in 0..pool_size {
            free_tx.send(Vec::with_capacity(chunk_size))?;
        }

        std::thread::scope(|scope| -> anyhow::Result<()> {
            let producer =
                scope.spawn(move || prefetch_cached(cached, chunk_size, free_rx, data_tx));
            let write_result = (|| -> anyhow::Result<()> {
                for (index, payload) in payloads.iter().enumerate() {
                    match payload {
                        PayloadSource::Memory(bytes) => self.write_payload_bytes(bytes)?,
                        PayloadSource::CachedFile { len, .. } => {
                            self.report.cached_payload_open_count += 1;
                            let mut received = 0_u64;
                            loop {
                                let wait_started = Instant::now();
                                let message = data_rx.recv()?;
                                self.report.writer_wait_ms += wait_started.elapsed().as_millis();
                                match message {
                                    PrefetchMessage::Data {
                                        payload_index,
                                        mut bytes,
                                        read_ms,
                                    } => {
                                        ensure_payload_index(index, payload_index)?;
                                        received =
                                            received.checked_add(bytes.len() as u64).ok_or_else(
                                                || anyhow::anyhow!("payload length overflow"),
                                            )?;
                                        if received > *len {
                                            anyhow::bail!(
                                                "prefetched payload exceeds declared length"
                                            );
                                        }
                                        self.report.payload_read_ms += read_ms;
                                        self.report.cached_payload_read_bytes += bytes.len() as u64;
                                        self.report.prefetched_bytes += bytes.len() as u64;
                                        self.write_payload_bytes(&bytes)?;
                                        bytes.clear();
                                        let _ = free_tx.send(bytes);
                                    }
                                    PrefetchMessage::End {
                                        payload_index,
                                        total,
                                    } => {
                                        ensure_payload_index(index, payload_index)?;
                                        if total != *len || received != *len {
                                            anyhow::bail!("prefetched payload length mismatch");
                                        }
                                        break;
                                    }
                                    PrefetchMessage::Error(message) => anyhow::bail!(message),
                                }
                            }
                        }
                    }
                }
                Ok(())
            })();
            drop(data_rx);
            drop(free_tx);
            let producer_result = producer
                .join()
                .map_err(|_| anyhow::anyhow!("payload prefetch worker panicked"))?;
            write_result?;
            producer_result?;
            Ok(())
        })
    }

    fn write_payload_bytes(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        let started = Instant::now();
        if bytes.len() >= self.options.transfer_chunk_bytes {
            let writer = self
                .writer
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("archive writer already finished"))?;
            writer.flush()?;
            writer.get_mut().write_all(bytes)?;
            self.written = self
                .written
                .checked_add(bytes.len() as u64)
                .ok_or_else(|| anyhow::anyhow!("archive length overflow"))?;
        } else {
            self.write_all(bytes)?;
        }
        self.report.payload_write_ms += started.elapsed().as_millis();
        Ok(())
    }
}

impl Write for ArchiveWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let writer = self.writer.as_mut().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "archive writer finished")
        })?;
        let written = writer.write(bytes)?;
        self.written = self
            .written
            .checked_add(written as u64)
            .ok_or_else(|| std::io::Error::other("archive length overflow"))?;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.writer
            .as_mut()
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::BrokenPipe, "archive writer finished")
            })?
            .flush()
    }
}

impl Drop for ArchiveWriter {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.temp_path);
        }
    }
}

enum PrefetchMessage {
    Data {
        payload_index: usize,
        bytes: Vec<u8>,
        read_ms: u128,
    },
    End {
        payload_index: usize,
        total: u64,
    },
    Error(String),
}

fn prefetch_cached(
    cached: Vec<(usize, PathBuf, u64)>,
    chunk_size: usize,
    free_rx: Receiver<Vec<u8>>,
    data_tx: SyncSender<PrefetchMessage>,
) -> anyhow::Result<()> {
    for (payload_index, path, len) in cached {
        let result = (|| -> anyhow::Result<()> {
            let mut input = File::open(&path)?;
            let mut remaining = len;
            let mut total = 0_u64;
            while remaining > 0 {
                let mut buffer = free_rx.recv()?;
                let limit = remaining.min(chunk_size as u64) as usize;
                buffer.resize(limit, 0);
                let read_started = Instant::now();
                input.read_exact(&mut buffer)?;
                let read_ms = read_started.elapsed().as_millis();
                total += limit as u64;
                remaining -= limit as u64;
                data_tx.send(PrefetchMessage::Data {
                    payload_index,
                    bytes: buffer,
                    read_ms,
                })?;
            }
            data_tx.send(PrefetchMessage::End {
                payload_index,
                total,
            })?;
            Ok(())
        })();
        if let Err(error) = result {
            let message = error.to_string();
            let _ = data_tx.send(PrefetchMessage::Error(message.clone()));
            anyhow::bail!(message);
        }
    }
    Ok(())
}

fn validate_cached_file(path: &Path, expected_len: u64) -> anyhow::Result<()> {
    let actual = fs::metadata(path)?.len();
    if actual != expected_len {
        anyhow::bail!(
            "cached payload length mismatch for {}: expected {}, got {}",
            path.display(),
            expected_len,
            actual
        );
    }
    Ok(())
}

fn ensure_payload_index(expected: usize, actual: usize) -> anyhow::Result<()> {
    if expected != actual {
        anyhow::bail!("prefetch order mismatch: expected payload {expected}, got {actual}");
    }
    Ok(())
}

fn unique_temp_path(final_path: &Path) -> PathBuf {
    let name = final_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("archive.hig");
    let random = crate::crypto::random_bytes::<8>()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    final_path.with_file_name(format!(".{name}.hig-tmp-{}-{random}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffered_writer_preserves_existing_target_on_failure() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("archive.hig");
        fs::write(&target, b"existing").unwrap();
        let mut writer = ArchiveWriter::create(&target, 8, IoOptions::default()).unwrap();
        writer.write_all(b"short").unwrap();
        assert!(writer.finish().is_err());
        assert_eq!(fs::read(target).unwrap(), b"existing");
    }

    #[test]
    fn prefetched_writer_preserves_payload_order() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first.sealed");
        let second = temp.path().join("second.sealed");
        fs::write(&first, vec![1_u8; 2 * 1024 * 1024]).unwrap();
        fs::write(&second, vec![2_u8; 2 * 1024 * 1024]).unwrap();
        let target = temp.path().join("archive.hig");
        let payloads = vec![
            PayloadSource::CachedFile {
                path: first,
                len: 2 * 1024 * 1024,
            },
            PayloadSource::Memory(b"middle".to_vec()),
            PayloadSource::CachedFile {
                path: second,
                len: 2 * 1024 * 1024,
            },
        ];
        let expected = 4 * 1024 * 1024 + 6;
        let mut writer = ArchiveWriter::create(&target, expected, IoOptions::default()).unwrap();
        writer.write_payloads(&payloads).unwrap();
        let report = writer.finish().unwrap();
        let bytes = fs::read(target).unwrap();
        assert_eq!(report.strategy, WriterStrategy::PrefetchedCachedFiles);
        assert_eq!(&bytes[..2 * 1024 * 1024], vec![1_u8; 2 * 1024 * 1024]);
        assert_eq!(&bytes[2 * 1024 * 1024..2 * 1024 * 1024 + 6], b"middle");
        assert_eq!(&bytes[2 * 1024 * 1024 + 6..], vec![2_u8; 2 * 1024 * 1024]);
    }

    #[test]
    fn cached_payload_length_mismatch_fails_without_replacing_target() {
        let temp = tempfile::tempdir().unwrap();
        let cached = temp.path().join("bad.sealed");
        fs::write(&cached, b"short").unwrap();
        let target = temp.path().join("archive.hig");
        fs::write(&target, b"existing").unwrap();
        let payloads = vec![PayloadSource::CachedFile {
            path: cached,
            len: 10,
        }];
        let mut writer = ArchiveWriter::create(&target, 10, IoOptions::default()).unwrap();
        assert!(writer.write_payloads(&payloads).is_err());
        assert_eq!(fs::read(target).unwrap(), b"existing");
    }

    #[test]
    fn small_cached_payload_uses_buffered_strategy() {
        let temp = tempfile::tempdir().unwrap();
        let cached = temp.path().join("small.sealed");
        fs::write(&cached, b"small").unwrap();
        let target = temp.path().join("archive.hig");
        let payloads = vec![PayloadSource::CachedFile {
            path: cached,
            len: 5,
        }];
        let mut writer = ArchiveWriter::create(&target, 5, IoOptions::default()).unwrap();
        writer.write_payloads(&payloads).unwrap();
        let report = writer.finish().unwrap();
        assert_eq!(report.strategy, WriterStrategy::Buffered);
        assert_eq!(report.cached_payload_open_count, 1);
        assert_eq!(report.cached_payload_read_bytes, 5);
    }

    #[test]
    fn temporary_paths_are_unique_and_local_to_target() {
        let target = Path::new("/tmp/example.hig");
        let first = unique_temp_path(target);
        let second = unique_temp_path(target);
        assert_ne!(first, second);
        assert_eq!(first.parent(), target.parent());
    }
}
