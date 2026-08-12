use crate::adaptive_io::{AdaptiveIoController, IoDirection};
use crate::{IoOptions, WriterStrategy};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, IoSlice, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::time::Instant;

const SMALL_ARCHIVE_BYTES: u64 = 16 * 1024 * 1024;
const LARGE_ARCHIVE_BYTES: u64 = 128 * 1024 * 1024;
const PREALLOCATE_MIN_BYTES: u64 = 64 * 1024 * 1024;
const PREFETCH_MIN_BYTES: u64 = 8 * 1024 * 1024;
const DIRECT_WRITE_MIN_BYTES: usize = 8 * 1024 * 1024;
const COALESCED_MAX_BYTES: usize = 8 * 1024 * 1024;
const COALESCED_MAX_SLICES: usize = 64;
const CONSTRAINED_COALESCED_MAX_BYTES: usize = 1024 * 1024;
const CONSTRAINED_COALESCED_MAX_SLICES: usize = 16;

pub(crate) enum PayloadSource {
    Memory(Vec<u8>),
    CachedFile {
        path: PathBuf,
        len: u64,
    },
    CachedRange {
        path: PathBuf,
        offset: u64,
        len: u64,
    },
}

pub(crate) struct PayloadStager {
    memory_budget_bytes: u64,
    memory_bytes: u64,
    spool_path: PathBuf,
    spool_writer: Option<BufWriter<File>>,
    spool_bytes: u64,
    spool_payloads: usize,
    io_controller: Option<Arc<AdaptiveIoController>>,
}

impl PayloadStager {
    #[cfg(test)]
    pub(crate) fn new(final_path: &Path, memory_budget_bytes: usize) -> Self {
        Self::new_with_io(final_path, memory_budget_bytes, None)
    }

    pub(crate) fn new_with_io(
        final_path: &Path,
        memory_budget_bytes: usize,
        io_controller: Option<Arc<AdaptiveIoController>>,
    ) -> Self {
        Self {
            memory_budget_bytes: memory_budget_bytes as u64,
            memory_bytes: 0,
            spool_path: unique_sidecar_path(final_path, "payload-spool"),
            spool_writer: None,
            spool_bytes: 0,
            spool_payloads: 0,
            io_controller,
        }
    }

    pub(crate) fn stage(&mut self, bytes: Vec<u8>) -> anyhow::Result<PayloadSource> {
        let len = bytes.len() as u64;
        if self
            .memory_bytes
            .checked_add(len)
            .is_some_and(|total| total <= self.memory_budget_bytes)
        {
            self.memory_bytes += len;
            return Ok(PayloadSource::Memory(bytes));
        }

        let offset = self.spool_bytes;
        if self.spool_writer.is_none() {
            if let Some(parent) = self.spool_path.parent()
                && !parent.as_os_str().is_empty()
            {
                fs::create_dir_all(parent)?;
            }
            let file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&self.spool_path)?;
            self.spool_writer = Some(BufWriter::with_capacity(8 * 1024 * 1024, file));
        }
        for chunk in bytes.chunks(1024 * 1024) {
            let buffered_before = self
                .spool_writer
                .as_ref()
                .expect("payload spool opened")
                .buffer()
                .len();
            let will_flush = buffered_before.saturating_add(chunk.len())
                >= self
                    .spool_writer
                    .as_ref()
                    .expect("payload spool opened")
                    .capacity();
            let permit = if will_flush {
                self.io_controller.as_ref().map(|controller| {
                    controller.acquire(
                        "payload-spool-write",
                        IoDirection::Write,
                        buffered_before.saturating_add(chunk.len()) as u64,
                    )
                })
            } else {
                None
            };
            self.spool_writer
                .as_mut()
                .expect("payload spool opened")
                .write_all(chunk)?;
            let buffered_after = self
                .spool_writer
                .as_ref()
                .expect("payload spool opened")
                .buffer()
                .len();
            if let Some(permit) = permit {
                permit.finish_with_bytes(
                    buffered_before
                        .saturating_add(chunk.len())
                        .saturating_sub(buffered_after) as u64,
                );
            }
        }
        self.spool_bytes = self
            .spool_bytes
            .checked_add(len)
            .ok_or_else(|| anyhow::anyhow!("payload spool length overflow"))?;
        self.spool_payloads += 1;
        Ok(PayloadSource::CachedRange {
            path: self.spool_path.clone(),
            offset,
            len,
        })
    }

    pub(crate) fn finish_writes(&mut self) -> anyhow::Result<()> {
        let Some(mut writer) = self.spool_writer.take() else {
            return Ok(());
        };
        let pending = writer.buffer().len() as u64;
        let permit = self.io_controller.as_ref().map(|controller| {
            controller.acquire("payload-spool-write", IoDirection::Write, pending)
        });
        writer.flush()?;
        if let Some(permit) = permit {
            permit.finish_with_bytes(pending);
        }
        writer
            .into_inner()
            .map_err(|error| anyhow::anyhow!("payload spool flush failed: {}", error.error()))?;
        Ok(())
    }

    pub(crate) fn memory_bytes(&self) -> u64 {
        self.memory_bytes
    }

    pub(crate) fn spool_bytes(&self) -> u64 {
        self.spool_bytes
    }

    pub(crate) fn spool_payloads(&self) -> usize {
        self.spool_payloads
    }
}

impl Drop for PayloadStager {
    fn drop(&mut self) {
        if self.spool_writer.is_some() || self.spool_bytes > 0 {
            let _ = fs::remove_file(&self.spool_path);
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ResolvedIoOptions {
    writer_buffer_bytes: usize,
    transfer_chunk_bytes: usize,
    prefetch_depth: usize,
    preallocate: bool,
}

impl ResolvedIoOptions {
    fn resolve(expected_len: u64, requested: IoOptions) -> anyhow::Result<Self> {
        if requested.prefetch_depth == 0 {
            anyhow::bail!("prefetch depth must be non-zero");
        }
        let writer_buffer_bytes =
            if requested.writer_buffer_bytes == IoOptions::default().writer_buffer_bytes {
                match expected_len {
                    0..SMALL_ARCHIVE_BYTES => 1024 * 1024,
                    SMALL_ARCHIVE_BYTES..LARGE_ARCHIVE_BYTES => 32 * 1024 * 1024,
                    _ => 32 * 1024 * 1024,
                }
            } else {
                requested.writer_buffer_bytes
            };
        let transfer_chunk_bytes =
            if requested.transfer_chunk_bytes == IoOptions::default().transfer_chunk_bytes {
                if expected_len >= SMALL_ARCHIVE_BYTES {
                    8 * 1024 * 1024
                } else {
                    4 * 1024 * 1024
                }
            } else {
                requested.transfer_chunk_bytes
            };
        if writer_buffer_bytes == 0 || transfer_chunk_bytes == 0 {
            anyhow::bail!("I/O buffer sizes must be non-zero");
        }
        Ok(Self {
            writer_buffer_bytes,
            transfer_chunk_bytes,
            prefetch_depth: requested.prefetch_depth,
            preallocate: expected_len >= PREALLOCATE_MIN_BYTES,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct WriterReport {
    pub strategy: WriterStrategy,
    pub preallocated_bytes: u64,
    pub cached_payload_open_count: usize,
    pub cached_range_open_count: usize,
    pub cached_payload_read_bytes: u64,
    pub prefetched_bytes: u64,
    pub peak_pipeline_memory_bytes: u64,
    pub payload_read_ms: u128,
    pub payload_write_ms: u128,
    pub writer_wait_ms: u128,
    pub flush_ms: u128,
    pub rename_ms: u128,
    pub temp_create_us: u64,
    pub preallocate_us: u64,
    pub payload_read_us: u64,
    pub payload_write_us: u64,
    pub payload_memory_write_us: u64,
    pub payload_cached_write_us: u64,
    pub direct_write_us: u64,
    pub buffered_write_us: u64,
    pub writer_wait_us: u64,
    pub flush_us: u64,
    pub fsync_us: u64,
    pub rename_us: u64,
    pub memory_payload_count: usize,
    pub memory_payload_bytes: u64,
    pub cached_file_payload_count: usize,
    pub cached_file_payload_bytes: u64,
    pub cached_range_payload_count: usize,
    pub cached_range_payload_bytes: u64,
    pub direct_write_count: usize,
    pub buffered_write_count: usize,
    pub coalesced_write_count: usize,
    pub coalesced_payload_count: usize,
    pub coalesced_bytes: u64,
    pub preallocation_enabled: bool,
}

pub(crate) struct ArchiveWriter {
    final_path: PathBuf,
    temp_path: PathBuf,
    writer: Option<BufWriter<File>>,
    expected_len: u64,
    written: u64,
    committed: bool,
    options: ResolvedIoOptions,
    report: WriterReport,
    io_controller: Option<Arc<AdaptiveIoController>>,
}

impl ArchiveWriter {
    pub(crate) fn create(
        final_path: &Path,
        expected_len: u64,
        options: IoOptions,
    ) -> anyhow::Result<Self> {
        Self::create_with_io(final_path, expected_len, options, None)
    }

    pub(crate) fn create_with_io(
        final_path: &Path,
        expected_len: u64,
        options: IoOptions,
        io_controller: Option<Arc<AdaptiveIoController>>,
    ) -> anyhow::Result<Self> {
        let options = ResolvedIoOptions::resolve(expected_len, options)?;
        let parent = final_path.parent().unwrap_or_else(|| Path::new("."));
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
        let temp_path = unique_temp_path(final_path);
        let temp_create_started = Instant::now();
        let create_permit = io_controller
            .as_ref()
            .map(|controller| controller.acquire("archive-create", IoDirection::Write, 0));
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)?;
        if let Some(permit) = create_permit {
            permit.finish();
        }
        let temp_create_us = temp_create_started.elapsed().as_micros() as u64;
        let mut preallocate_us = 0;
        if options.preallocate {
            let preallocate_started = Instant::now();
            let permit = io_controller
                .as_ref()
                .map(|controller| controller.acquire("archive-preallocate", IoDirection::Write, 0));
            file.set_len(expected_len)?;
            if let Some(permit) = permit {
                permit.finish();
            }
            preallocate_us = preallocate_started.elapsed().as_micros() as u64;
        }
        Ok(Self {
            final_path: final_path.to_path_buf(),
            temp_path,
            writer: Some(BufWriter::with_capacity(options.writer_buffer_bytes, file)),
            expected_len,
            written: 0,
            committed: false,
            options,
            report: WriterReport {
                temp_create_us,
                preallocate_us,
                preallocated_bytes: if options.preallocate { expected_len } else { 0 },
                preallocation_enabled: options.preallocate,
                peak_pipeline_memory_bytes: options.writer_buffer_bytes as u64,
                ..WriterReport::default()
            },
            io_controller,
        })
    }

    pub(crate) fn write_payloads(&mut self, payloads: &[PayloadSource]) -> anyhow::Result<()> {
        let cached_count = payloads
            .iter()
            .filter(|payload| {
                matches!(
                    payload,
                    PayloadSource::CachedFile { .. } | PayloadSource::CachedRange { .. }
                )
            })
            .count();
        let cached_bytes = payloads.iter().try_fold(0_u64, |total, payload| {
            let len = match payload {
                PayloadSource::Memory(_) => 0,
                PayloadSource::CachedFile { len, .. } | PayloadSource::CachedRange { len, .. } => {
                    *len
                }
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
        let pending = writer.buffer().len() as u64;
        let flush_permit = self
            .io_controller
            .as_ref()
            .map(|controller| controller.acquire("archive-write", IoDirection::Write, pending));
        writer.flush()?;
        if let Some(permit) = flush_permit {
            permit.finish_with_bytes(pending);
        }
        let file = writer
            .into_inner()
            .map_err(|error| anyhow::anyhow!("archive flush failed: {}", error.error()))?;
        self.report.flush_us = flush_started.elapsed().as_micros() as u64;
        self.report.flush_ms = (self.report.flush_us / 1000) as u128;
        drop(file);

        let rename_started = Instant::now();
        fs::rename(&self.temp_path, &self.final_path)?;
        self.report.rename_us = rename_started.elapsed().as_micros() as u64;
        self.report.rename_ms = (self.report.rename_us / 1000) as u128;
        self.committed = true;
        Ok(self.report.clone())
    }

    fn write_buffered(&mut self, payloads: &[PayloadSource]) -> anyhow::Result<()> {
        let mut buffer = vec![0_u8; self.options.transfer_chunk_bytes];
        self.report.peak_pipeline_memory_bytes = self
            .report
            .peak_pipeline_memory_bytes
            .max((self.options.writer_buffer_bytes + buffer.len()) as u64);
        let mut open_files = BTreeMap::<PathBuf, File>::new();
        let mut payload_index = 0;
        while payload_index < payloads.len() {
            match &payloads[payload_index] {
                PayloadSource::Memory(_) => {
                    let run_start = payload_index;
                    while payload_index < payloads.len()
                        && matches!(payloads[payload_index], PayloadSource::Memory(_))
                    {
                        payload_index += 1;
                    }
                    self.write_memory_run(&payloads[run_start..payload_index])?;
                    continue;
                }
                PayloadSource::CachedFile { path, len } => {
                    validate_cached_file(path, *len)?;
                    let mut input = File::open(path)?;
                    self.report.cached_payload_open_count += 1;
                    self.report.cached_file_payload_count += 1;
                    self.report.cached_file_payload_bytes += *len;
                    self.copy_range(&mut input, *len, &mut buffer)?;
                }
                PayloadSource::CachedRange { path, offset, len } => {
                    validate_cached_range(path, *offset, *len)?;
                    self.report.cached_range_payload_count += 1;
                    self.report.cached_range_payload_bytes += *len;
                    let input = match open_files.get_mut(path) {
                        Some(input) => input,
                        None => {
                            let file = File::open(path)?;
                            self.report.cached_range_open_count += 1;
                            open_files.insert(path.clone(), file);
                            open_files.get_mut(path).expect("range file inserted")
                        }
                    };
                    input.seek(SeekFrom::Start(*offset))?;
                    self.copy_range(input, *len, &mut buffer)?;
                }
            }
            payload_index += 1;
        }
        Ok(())
    }

    fn copy_range(&mut self, input: &mut File, len: u64, buffer: &mut [u8]) -> anyhow::Result<()> {
        let mut remaining = len;
        while remaining > 0 {
            let limit = remaining.min(buffer.len() as u64) as usize;
            let permit = self.io_controller.as_ref().map(|controller| {
                controller.acquire("archive-payload-read", IoDirection::Read, limit as u64)
            });
            let read_started = Instant::now();
            input.read_exact(&mut buffer[..limit])?;
            let read_us = read_started.elapsed().as_micros() as u64;
            if let Some(permit) = permit {
                permit.finish_with_bytes(limit as u64);
            }
            self.report.payload_read_us += read_us;
            self.report.payload_read_ms += (read_us / 1000) as u128;
            self.write_payload_bytes(&buffer[..limit], PayloadWriteKind::Cached)?;
            self.report.cached_payload_read_bytes += limit as u64;
            remaining -= limit as u64;
        }
        Ok(())
    }

    fn write_prefetched(&mut self, payloads: &[PayloadSource]) -> anyhow::Result<()> {
        let cached = payloads
            .iter()
            .enumerate()
            .filter_map(|(index, payload)| match payload {
                PayloadSource::CachedFile { path, len } => Some(CachedRead {
                    payload_index: index,
                    path: path.clone(),
                    offset: 0,
                    len: *len,
                    standalone_file: true,
                }),
                PayloadSource::CachedRange { path, offset, len } => Some(CachedRead {
                    payload_index: index,
                    path: path.clone(),
                    offset: *offset,
                    len: *len,
                    standalone_file: false,
                }),
                PayloadSource::Memory(_) => None,
            })
            .collect::<Vec<_>>();
        for cached in &cached {
            if cached.standalone_file {
                validate_cached_file(&cached.path, cached.len)?;
            } else {
                validate_cached_range(&cached.path, cached.offset, cached.len)?;
            }
        }

        let chunk_size = self.options.transfer_chunk_bytes;
        let pool_size = self.options.prefetch_depth + 1;
        self.report.peak_pipeline_memory_bytes = self
            .report
            .peak_pipeline_memory_bytes
            .max((self.options.writer_buffer_bytes + chunk_size * pool_size) as u64);
        let (data_tx, data_rx) = sync_channel(self.options.prefetch_depth);
        let (free_tx, free_rx) = sync_channel(pool_size);
        for _ in 0..pool_size {
            free_tx.send(Vec::with_capacity(chunk_size))?;
        }

        std::thread::scope(|scope| -> anyhow::Result<()> {
            let io_controller = self.io_controller.clone();
            let producer = scope.spawn(move || {
                prefetch_cached(cached, chunk_size, free_rx, data_tx, io_controller)
            });
            let write_result = (|| -> anyhow::Result<()> {
                let mut index = 0;
                while index < payloads.len() {
                    match &payloads[index] {
                        PayloadSource::Memory(_) => {
                            let run_start = index;
                            while index < payloads.len()
                                && matches!(payloads[index], PayloadSource::Memory(_))
                            {
                                index += 1;
                            }
                            self.write_memory_run(&payloads[run_start..index])?;
                            continue;
                        }
                        PayloadSource::CachedFile { len, .. }
                        | PayloadSource::CachedRange { len, .. } => {
                            if matches!(payloads[index], PayloadSource::CachedFile { .. }) {
                                self.report.cached_file_payload_count += 1;
                                self.report.cached_file_payload_bytes += *len;
                            } else {
                                self.report.cached_range_payload_count += 1;
                                self.report.cached_range_payload_bytes += *len;
                            }
                            let mut received = 0_u64;
                            loop {
                                let wait_started = Instant::now();
                                let message = data_rx.recv()?;
                                let wait_us = wait_started.elapsed().as_micros() as u64;
                                self.report.writer_wait_us += wait_us;
                                self.report.writer_wait_ms += (wait_us / 1000) as u128;
                                match message {
                                    PrefetchMessage::Data {
                                        payload_index,
                                        mut bytes,
                                        read_us,
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
                                        self.report.payload_read_us += read_us;
                                        self.report.payload_read_ms += (read_us / 1000) as u128;
                                        self.report.cached_payload_read_bytes += bytes.len() as u64;
                                        self.report.prefetched_bytes += bytes.len() as u64;
                                        self.write_payload_bytes(&bytes, PayloadWriteKind::Cached)?;
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
                                    PrefetchMessage::Open {
                                        path,
                                        standalone_file,
                                    } => {
                                        if standalone_file {
                                            self.report.cached_payload_open_count += 1;
                                        } else {
                                            self.report.cached_range_open_count += 1;
                                        }
                                        let _ = path;
                                    }
                                    PrefetchMessage::Error(message) => anyhow::bail!(message),
                                }
                            }
                        }
                    }
                    index += 1;
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

    fn write_payload_bytes(&mut self, bytes: &[u8], kind: PayloadWriteKind) -> anyhow::Result<()> {
        let started = Instant::now();
        if self.can_direct_write(bytes.len()) {
            self.flush_buffered_prefix()?;
            let permit = self.io_controller.as_ref().map(|controller| {
                controller.acquire("archive-write", IoDirection::Write, bytes.len() as u64)
            });
            let writer = self
                .writer
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("archive writer already finished"))?;
            writer.get_mut().write_all(bytes)?;
            if let Some(permit) = permit {
                permit.finish_with_bytes(bytes.len() as u64);
            }
            self.written = self
                .written
                .checked_add(bytes.len() as u64)
                .ok_or_else(|| anyhow::anyhow!("archive length overflow"))?;
            self.report.direct_write_count += 1;
            let elapsed_us = started.elapsed().as_micros() as u64;
            self.report.direct_write_us += elapsed_us;
            self.record_payload_write_elapsed(kind, elapsed_us);
        } else {
            self.write_all(bytes)?;
            self.report.buffered_write_count += 1;
            let elapsed_us = started.elapsed().as_micros() as u64;
            self.report.buffered_write_us += elapsed_us;
            self.record_payload_write_elapsed(kind, elapsed_us);
        }
        Ok(())
    }

    fn write_memory_run(&mut self, payloads: &[PayloadSource]) -> anyhow::Result<()> {
        for payload in payloads {
            let PayloadSource::Memory(bytes) = payload else {
                unreachable!("memory run contains a cached payload");
            };
            self.report.memory_payload_count += 1;
            self.report.memory_payload_bytes += bytes.len() as u64;
        }

        let mut index = 0;
        while index < payloads.len() {
            let PayloadSource::Memory(bytes) = &payloads[index] else {
                unreachable!("memory run contains a cached payload");
            };
            if bytes.len() >= DIRECT_WRITE_MIN_BYTES {
                self.write_payload_bytes(bytes, PayloadWriteKind::Memory)?;
                index += 1;
                continue;
            }

            let (max_bytes, max_slices) = self.coalescing_limits();
            let batch_start = index;
            let mut batch_bytes = 0_usize;
            while index < payloads.len() && index - batch_start < max_slices {
                let PayloadSource::Memory(bytes) = &payloads[index] else {
                    unreachable!("memory run contains a cached payload");
                };
                if bytes.len() >= DIRECT_WRITE_MIN_BYTES
                    || (batch_bytes > 0 && batch_bytes.saturating_add(bytes.len()) > max_bytes)
                {
                    break;
                }
                batch_bytes = batch_bytes.saturating_add(bytes.len());
                index += 1;
            }

            let batch = &payloads[batch_start..index];
            if batch.len() == 1 {
                let PayloadSource::Memory(bytes) = &batch[0] else {
                    unreachable!("memory run contains a cached payload");
                };
                self.write_payload_bytes(bytes, PayloadWriteKind::Memory)?;
            } else {
                let slices = batch
                    .iter()
                    .map(|payload| match payload {
                        PayloadSource::Memory(bytes) => bytes.as_slice(),
                        _ => unreachable!("memory run contains a cached payload"),
                    })
                    .collect::<Vec<_>>();
                self.write_payload_slices(&slices, batch_bytes)?;
            }
        }
        Ok(())
    }

    fn write_payload_slices(&mut self, slices: &[&[u8]], total: usize) -> anyhow::Result<()> {
        let started = Instant::now();
        let mut slice_index = 0;
        let mut slice_offset = 0;
        while slice_index < slices.len() {
            let mut io_slices = Vec::with_capacity(slices.len() - slice_index);
            io_slices.push(IoSlice::new(&slices[slice_index][slice_offset..]));
            io_slices.extend(
                slices[slice_index + 1..]
                    .iter()
                    .map(|bytes| IoSlice::new(bytes)),
            );
            let written = self.write_vectored_once(&io_slices)?;
            if written == 0 && io_slices.iter().any(|slice| !slice.is_empty()) {
                return Err(std::io::Error::from(std::io::ErrorKind::WriteZero).into());
            }
            let mut remaining = written;
            while slice_index < slices.len() {
                let available = slices[slice_index].len().saturating_sub(slice_offset);
                if remaining < available {
                    slice_offset += remaining;
                    break;
                }
                remaining -= available;
                slice_index += 1;
                slice_offset = 0;
            }
        }
        let elapsed_us = started.elapsed().as_micros() as u64;
        self.report.buffered_write_count += 1;
        self.report.buffered_write_us += elapsed_us;
        self.report.coalesced_write_count += 1;
        self.report.coalesced_payload_count += slices.len();
        self.report.coalesced_bytes += total as u64;
        self.record_payload_write_elapsed(PayloadWriteKind::Memory, elapsed_us);
        Ok(())
    }

    fn write_vectored_once(&mut self, slices: &[IoSlice<'_>]) -> std::io::Result<usize> {
        let writer = self.writer.as_mut().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "archive writer finished")
        })?;
        let buffered_before = writer.buffer().len();
        let requested = slices.iter().map(|slice| slice.len()).sum::<usize>();
        let will_flush = buffered_before.saturating_add(requested) >= writer.capacity();
        let permit = if will_flush {
            self.io_controller.as_ref().map(|controller| {
                controller.acquire(
                    "archive-write",
                    IoDirection::Write,
                    buffered_before.saturating_add(requested) as u64,
                )
            })
        } else {
            None
        };
        let written = writer.write_vectored(slices)?;
        let buffered_after = writer.buffer().len();
        if let Some(permit) = permit {
            permit.finish_with_bytes(
                buffered_before
                    .saturating_add(written)
                    .saturating_sub(buffered_after) as u64,
            );
        }
        self.written = self
            .written
            .checked_add(written as u64)
            .ok_or_else(|| std::io::Error::other("archive length overflow"))?;
        Ok(written)
    }

    fn coalescing_limits(&self) -> (usize, usize) {
        let constrained = self.io_controller.as_ref().is_some_and(|controller| {
            controller.target_concurrency() < controller.max_concurrency()
        });
        if constrained {
            (
                CONSTRAINED_COALESCED_MAX_BYTES,
                CONSTRAINED_COALESCED_MAX_SLICES,
            )
        } else {
            (COALESCED_MAX_BYTES, COALESCED_MAX_SLICES)
        }
    }

    fn flush_buffered_prefix(&mut self) -> anyhow::Result<()> {
        let writer = self
            .writer
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("archive writer already finished"))?;
        let pending = writer.buffer().len() as u64;
        let permit = self
            .io_controller
            .as_ref()
            .map(|controller| controller.acquire("archive-write", IoDirection::Write, pending));
        writer.flush()?;
        if let Some(permit) = permit {
            permit.finish_with_bytes(pending);
        }
        Ok(())
    }

    fn record_payload_write_elapsed(&mut self, kind: PayloadWriteKind, elapsed_us: u64) {
        self.report.payload_write_us += elapsed_us;
        self.report.payload_write_ms += (elapsed_us / 1000) as u128;
        match kind {
            PayloadWriteKind::Memory => self.report.payload_memory_write_us += elapsed_us,
            PayloadWriteKind::Cached => self.report.payload_cached_write_us += elapsed_us,
        }
    }

    fn can_direct_write(&self, len: usize) -> bool {
        len >= DIRECT_WRITE_MIN_BYTES
    }
}

#[derive(Debug, Clone, Copy)]
enum PayloadWriteKind {
    Memory,
    Cached,
}

impl Write for ArchiveWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let writer = self.writer.as_mut().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "archive writer finished")
        })?;
        let buffered_before = writer.buffer().len();
        let will_flush = buffered_before.saturating_add(bytes.len()) >= writer.capacity();
        let permit = if will_flush {
            self.io_controller.as_ref().map(|controller| {
                controller.acquire(
                    "archive-write",
                    IoDirection::Write,
                    buffered_before.saturating_add(bytes.len()) as u64,
                )
            })
        } else {
            None
        };
        let written = writer.write(bytes)?;
        let buffered_after = writer.buffer().len();
        if let Some(permit) = permit {
            permit.finish_with_bytes(
                buffered_before
                    .saturating_add(written)
                    .saturating_sub(buffered_after) as u64,
            );
        }
        self.written = self
            .written
            .checked_add(written as u64)
            .ok_or_else(|| std::io::Error::other("archive length overflow"))?;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let writer = self.writer.as_mut().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "archive writer finished")
        })?;
        let pending = writer.buffer().len() as u64;
        let permit = self
            .io_controller
            .as_ref()
            .map(|controller| controller.acquire("archive-write", IoDirection::Write, pending));
        let result = writer.flush();
        if result.is_ok()
            && let Some(permit) = permit
        {
            permit.finish_with_bytes(pending);
        }
        result
    }
}

impl Drop for ArchiveWriter {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.temp_path);
        }
    }
}

#[derive(Debug, Clone)]
struct CachedRead {
    payload_index: usize,
    path: PathBuf,
    offset: u64,
    len: u64,
    standalone_file: bool,
}

enum PrefetchMessage {
    Open {
        path: PathBuf,
        standalone_file: bool,
    },
    Data {
        payload_index: usize,
        bytes: Vec<u8>,
        read_us: u64,
    },
    End {
        payload_index: usize,
        total: u64,
    },
    Error(String),
}

fn prefetch_cached(
    cached: Vec<CachedRead>,
    chunk_size: usize,
    free_rx: Receiver<Vec<u8>>,
    data_tx: SyncSender<PrefetchMessage>,
    io_controller: Option<Arc<AdaptiveIoController>>,
) -> anyhow::Result<()> {
    let mut open_files = BTreeMap::<PathBuf, File>::new();
    for cached_read in cached {
        let result = (|| -> anyhow::Result<()> {
            let input = match open_files.get_mut(&cached_read.path) {
                Some(input) => input,
                None => {
                    let file = File::open(&cached_read.path)?;
                    data_tx.send(PrefetchMessage::Open {
                        path: cached_read.path.clone(),
                        standalone_file: cached_read.standalone_file,
                    })?;
                    open_files.insert(cached_read.path.clone(), file);
                    open_files
                        .get_mut(&cached_read.path)
                        .expect("prefetch file inserted")
                }
            };
            input.seek(SeekFrom::Start(cached_read.offset))?;
            let mut remaining = cached_read.len;
            let mut total = 0_u64;
            while remaining > 0 {
                let mut buffer = free_rx.recv()?;
                let limit = remaining.min(chunk_size as u64) as usize;
                buffer.resize(limit, 0);
                let permit = io_controller.as_ref().map(|controller| {
                    controller.acquire("archive-payload-read", IoDirection::Read, limit as u64)
                });
                let read_started = Instant::now();
                input.read_exact(&mut buffer)?;
                let read_us = read_started.elapsed().as_micros() as u64;
                if let Some(permit) = permit {
                    permit.finish_with_bytes(limit as u64);
                }
                total += limit as u64;
                remaining -= limit as u64;
                data_tx.send(PrefetchMessage::Data {
                    payload_index: cached_read.payload_index,
                    bytes: buffer,
                    read_us,
                })?;
            }
            data_tx.send(PrefetchMessage::End {
                payload_index: cached_read.payload_index,
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

fn validate_cached_range(path: &Path, offset: u64, len: u64) -> anyhow::Result<()> {
    let actual = fs::metadata(path)?.len();
    let end = offset
        .checked_add(len)
        .ok_or_else(|| anyhow::anyhow!("cached range length overflow"))?;
    if end > actual {
        anyhow::bail!(
            "cached range exceeds file bounds for {}: offset {}, len {}, file {}",
            path.display(),
            offset,
            len,
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
    unique_sidecar_path(final_path, "hig-tmp")
}

fn unique_sidecar_path(final_path: &Path, kind: &str) -> PathBuf {
    let name = final_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("archive.hig");
    let random = crate::crypto::random_bytes::<8>()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    final_path.with_file_name(format!(".{name}.{kind}-{}-{random}", std::process::id()))
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
    fn payload_stager_bounds_memory_and_cleans_spool() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("nested/archive.hig");
        let spool_path;
        {
            let mut stager = PayloadStager::new(&target, 4);
            assert!(matches!(
                stager.stage(b"aaaa".to_vec()).unwrap(),
                PayloadSource::Memory(_)
            ));
            let second = stager.stage(b"bbbb".to_vec()).unwrap();
            let third = stager.stage(b"cc".to_vec()).unwrap();
            stager.finish_writes().unwrap();

            assert_eq!(stager.memory_bytes(), 4);
            assert_eq!(stager.spool_bytes(), 6);
            assert_eq!(stager.spool_payloads(), 2);
            assert!(matches!(
                second,
                PayloadSource::CachedRange {
                    offset: 0,
                    len: 4,
                    ..
                }
            ));
            assert!(matches!(
                third,
                PayloadSource::CachedRange {
                    offset: 4,
                    len: 2,
                    ..
                }
            ));
            spool_path = stager.spool_path.clone();
            assert_eq!(fs::read(&spool_path).unwrap(), b"bbbbcc");
        }
        assert!(!spool_path.exists());
    }

    #[test]
    fn direct_payload_preserves_buffered_prefix() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("archive.hig");
        let payload = vec![7_u8; DIRECT_WRITE_MIN_BYTES];
        let expected_len = (6 + payload.len()) as u64;
        let mut writer =
            ArchiveWriter::create(&target, expected_len, IoOptions::default()).unwrap();
        writer.write_all(b"header").unwrap();
        writer
            .write_payloads(&[PayloadSource::Memory(payload)])
            .unwrap();
        let report = writer.finish().unwrap();
        let bytes = fs::read(target).unwrap();

        assert_eq!(&bytes[..6], b"header");
        assert!(bytes[6..].iter().all(|byte| *byte == 7));
        assert_eq!(report.direct_write_count, 1);
        assert_eq!(report.buffered_write_count, 0);
    }

    #[test]
    fn consecutive_memory_payloads_are_coalesced_without_reordering() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("coalesced.hig");
        let payloads = (0..130)
            .map(|index| PayloadSource::Memory(vec![index as u8; 4]))
            .collect::<Vec<_>>();
        let expected = (0..130)
            .flat_map(|index| vec![index as u8; 4])
            .collect::<Vec<_>>();
        let mut writer =
            ArchiveWriter::create(&target, expected.len() as u64, IoOptions::default()).unwrap();

        writer.write_payloads(&payloads).unwrap();
        let report = writer.finish().unwrap();

        assert_eq!(fs::read(target).unwrap(), expected);
        assert_eq!(report.memory_payload_count, 130);
        assert_eq!(report.memory_payload_bytes, 520);
        assert_eq!(report.buffered_write_count, 3);
        assert_eq!(report.coalesced_write_count, 3);
        assert_eq!(report.coalesced_payload_count, 130);
        assert_eq!(report.coalesced_bytes, 520);
    }

    #[test]
    fn direct_payload_splits_memory_coalescing_batches() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("mixed.hig");
        let large = vec![7_u8; DIRECT_WRITE_MIN_BYTES];
        let payloads = vec![
            PayloadSource::Memory(b"before".to_vec()),
            PayloadSource::Memory(large.clone()),
            PayloadSource::Memory(b"after".to_vec()),
        ];
        let expected_len = (11 + large.len()) as u64;
        let mut writer =
            ArchiveWriter::create(&target, expected_len, IoOptions::default()).unwrap();

        writer.write_payloads(&payloads).unwrap();
        let report = writer.finish().unwrap();
        let bytes = fs::read(target).unwrap();

        assert_eq!(&bytes[..6], b"before");
        assert_eq!(&bytes[6..6 + large.len()], large);
        assert_eq!(&bytes[6 + large.len()..], b"after");
        assert_eq!(report.direct_write_count, 1);
        assert_eq!(report.buffered_write_count, 2);
        assert_eq!(report.coalesced_write_count, 0);
    }

    #[test]
    fn prefetched_writer_preserves_payload_order() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first.sealed");
        let second = temp.path().join("second.sealed");
        fs::write(&first, vec![1_u8; 4 * 1024 * 1024]).unwrap();
        fs::write(&second, vec![2_u8; 4 * 1024 * 1024]).unwrap();
        let target = temp.path().join("archive.hig");
        let payloads = vec![
            PayloadSource::CachedFile {
                path: first,
                len: 4 * 1024 * 1024,
            },
            PayloadSource::Memory(b"middle".to_vec()),
            PayloadSource::CachedFile {
                path: second,
                len: 4 * 1024 * 1024,
            },
        ];
        let expected = 8 * 1024 * 1024 + 6;
        let mut writer = ArchiveWriter::create(&target, expected, IoOptions::default()).unwrap();
        writer.write_payloads(&payloads).unwrap();
        let report = writer.finish().unwrap();
        let bytes = fs::read(target).unwrap();
        assert_eq!(report.strategy, WriterStrategy::PrefetchedCachedFiles);
        assert_eq!(&bytes[..4 * 1024 * 1024], vec![1_u8; 4 * 1024 * 1024]);
        assert_eq!(&bytes[4 * 1024 * 1024..4 * 1024 * 1024 + 6], b"middle");
        assert_eq!(&bytes[4 * 1024 * 1024 + 6..], vec![2_u8; 4 * 1024 * 1024]);
    }

    #[test]
    fn cached_ranges_reuse_one_open_file() {
        let temp = tempfile::tempdir().unwrap();
        let pack = temp.path().join("pack.sealed");
        fs::write(
            &pack,
            [vec![1_u8; 4], vec![2_u8; 4], vec![3_u8; 4]].concat(),
        )
        .unwrap();
        let target = temp.path().join("archive.hig");
        let payloads = vec![
            PayloadSource::CachedRange {
                path: pack.clone(),
                offset: 0,
                len: 4,
            },
            PayloadSource::CachedRange {
                path: pack.clone(),
                offset: 4,
                len: 4,
            },
            PayloadSource::CachedRange {
                path: pack,
                offset: 8,
                len: 4,
            },
        ];
        let mut writer = ArchiveWriter::create(&target, 12, IoOptions::default()).unwrap();
        writer.write_payloads(&payloads).unwrap();
        let report = writer.finish().unwrap();
        assert_eq!(
            fs::read(target).unwrap(),
            [vec![1_u8; 4], vec![2_u8; 4], vec![3_u8; 4]].concat()
        );
        assert_eq!(report.cached_range_open_count, 1);
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
    fn small_cached_payload_uses_buffered_strategy_and_no_preallocation() {
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
        assert!(!report.preallocation_enabled);
        assert_eq!(report.cached_payload_open_count, 1);
        assert_eq!(report.cached_payload_read_bytes, 5);
    }

    #[test]
    fn large_archive_uses_preallocation() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("archive.hig");
        let writer =
            ArchiveWriter::create(&target, 64 * 1024 * 1024, IoOptions::default()).unwrap();
        assert!(writer.report.preallocation_enabled);
        assert_eq!(writer.report.preallocated_bytes, 64 * 1024 * 1024);
    }

    #[test]
    fn adaptive_writer_reports_bytes_flushed_to_the_archive() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("adaptive.hig");
        let payload = vec![5_u8; 2 * 1024 * 1024];
        let controller = AdaptiveIoController::new(4);
        let mut writer = ArchiveWriter::create_with_io(
            &target,
            payload.len() as u64,
            IoOptions::default(),
            Some(controller.clone()),
        )
        .unwrap();
        writer.write_all(&payload).unwrap();
        writer.finish().unwrap();

        let report = controller.report();
        assert_eq!(
            report.stages.get("archive-write").unwrap().bytes,
            payload.len() as u64
        );
        assert_eq!(fs::read(target).unwrap(), payload);
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
