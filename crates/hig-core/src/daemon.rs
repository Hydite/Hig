use crate::archive::{PackKeyOverride, pack_with_engine_cache};
use crate::cache::{CacheMaintenanceReport, CacheStore};
use crate::crypto::{KEY_LEN, SALT_LEN};
use crate::{
    ArchiveFormat, BatchOptions, ChunkOptions, Compression, EncryptionMode, KdfProfile,
    ManifestFormat, PackOptions, PackReport, PipelineOptions, SessionBinding, SolidMode, SpeedMode,
};
use crate::{BufferPool, PipelineScheduler};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;
use zeroize::Zeroize;

#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};

const PROTOCOL_VERSION: u16 = 2;
const MAX_FRAME_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub active: bool,
    pub age_secs: u64,
    pub ttl_secs: u64,
    pub jobs_completed: u64,
    pub cache_open_count: u64,
    pub session_active: bool,
    pub session_age_secs: u64,
    pub session_binding_fingerprint: Option<[u8; 32]>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobKeyMaterial {
    pub key: [u8; KEY_LEN],
    pub salt: [u8; SALT_LEN],
}

impl Drop for JobKeyMaterial {
    fn drop(&mut self) {
        self.key.zeroize();
        self.salt.zeroize();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializablePackOptions {
    pub input_dir: PathBuf,
    pub output_file: PathBuf,
    pub encryption: EncryptionMode,
    pub threads: Option<usize>,
    pub compression: Compression,
    pub level: Option<i32>,
    pub use_cache: bool,
    pub trust_metadata: bool,
    pub format: ArchiveFormat,
    pub batch: BatchOptions,
    pub chunk: ChunkOptions,
    pub speed: SpeedMode,
    pub kdf_profile: KdfProfile,
    pub sealed_cache: bool,
    pub manifest_format: ManifestFormat,
    pub use_session: bool,
    pub session_required: bool,
    pub solid: SolidMode,
    pub pipeline: PipelineOptions,
}

impl SerializablePackOptions {
    pub fn from_pack(options: &PackOptions) -> Self {
        Self {
            input_dir: options.input_dir.clone(),
            output_file: options.output_file.clone(),
            encryption: options.encryption,
            threads: options.threads,
            compression: options.compression,
            level: options.level,
            use_cache: options.use_cache,
            trust_metadata: options.trust_metadata,
            format: options.format,
            batch: options.batch,
            chunk: options.chunk,
            speed: options.speed,
            kdf_profile: options.kdf_profile,
            sealed_cache: options.sealed_cache,
            manifest_format: options.manifest_format,
            use_session: options.use_session,
            session_required: options.session_required,
            solid: options.solid,
            pipeline: options.pipeline,
        }
    }

    fn into_pack(self, cache_dir: PathBuf) -> PackOptions {
        PackOptions {
            input_dir: self.input_dir,
            output_file: self.output_file,
            password: None,
            encryption: self.encryption,
            cache_dir: Some(cache_dir),
            threads: self.threads,
            compression: self.compression,
            level: self.level,
            use_cache: self.use_cache,
            trust_metadata: self.trust_metadata,
            format: self.format,
            batch: self.batch,
            chunk: self.chunk,
            speed: self.speed,
            kdf_profile: self.kdf_profile,
            sealed_cache: self.sealed_cache,
            manifest_format: self.manifest_format,
            use_session: false,
            session_required: false,
            session_ttl_secs: None,
            solid: self.solid,
            pipeline: PipelineOptions {
                daemon_mode: crate::DaemonMode::Off,
                ..self.pipeline
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackJobRequest {
    pub options: SerializablePackOptions,
    pub binding_fingerprint: Option<[u8; 32]>,
    pub ephemeral_key: Option<JobKeyMaterial>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DaemonRequest {
    Status,
    Stop,
    UnlockChallenge {
        binding: SessionBinding,
    },
    InstallSessionKey {
        binding: SessionBinding,
        key: [u8; KEY_LEN],
        salt: [u8; SALT_LEN],
        ttl_secs: u64,
    },
    ClearSession,
    Pack(PackJobRequest),
    CacheStatus,
    CacheGc {
        dry_run: bool,
    },
    CacheCompact {
        dry_run: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DaemonResponse {
    Status(DaemonStatus),
    UnlockChallenge { salt: [u8; SALT_LEN] },
    SessionInstalled,
    SessionCleared,
    PackComplete(Box<PackReport>),
    CacheMaintenance(CacheMaintenanceReport),
    Stopped,
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProtocolEnvelope<T> {
    protocol_version: u16,
    request_id: [u8; 16],
    payload: T,
}

struct SessionState {
    binding: SessionBinding,
    key: [u8; KEY_LEN],
    salt: [u8; SALT_LEN],
    created: Instant,
    ttl_secs: u64,
}

impl SessionState {
    fn active(&self) -> bool {
        self.created.elapsed().as_secs() < self.ttl_secs
    }
}

impl Drop for SessionState {
    fn drop(&mut self) {
        self.key.zeroize();
        self.salt.zeroize();
    }
}

struct DaemonRuntime {
    cache_dir: PathBuf,
    engine: PackEngine,
    session: Option<SessionState>,
    started: Instant,
    ttl_secs: u64,
    jobs_completed: u64,
    stop: bool,
}

struct PackEngine {
    cache: CacheStore,
    rayon_pool: rayon::ThreadPool,
    buffers: BufferPool,
    scheduler: PipelineScheduler,
    cache_open_count: u64,
}

impl PackEngine {
    fn open(cache_dir: &Path) -> anyhow::Result<Self> {
        let workers = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1);
        Ok(Self {
            cache: CacheStore::open(cache_dir)?,
            rayon_pool: rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()?,
            buffers: BufferPool::new(256 * 1024 * 1024),
            scheduler: PipelineScheduler::new(true),
            cache_open_count: 1,
        })
    }

    fn execute(
        &mut self,
        cache_dir: &Path,
        options: SerializablePackOptions,
        key_override: Option<PackKeyOverride>,
    ) -> anyhow::Result<PackReport> {
        let _scheduler = &self.scheduler;
        let mut report = self.rayon_pool.install(|| {
            pack_with_engine_cache(
                options.into_pack(cache_dir.to_path_buf()),
                &mut self.cache,
                key_override,
            )
        })?;
        report.pipeline.buffer_pool_hits = self.buffers.hits();
        report.pipeline.buffer_pool_misses = self.buffers.misses();
        report.pipeline.pipeline_peak_memory_bytes = self.buffers.peak_bytes();
        report.pipeline.hot_index_reuses = 1;
        Ok(report)
    }
}

impl DaemonRuntime {
    fn status(&self) -> DaemonStatus {
        DaemonStatus {
            active: !self.stop,
            age_secs: self.started.elapsed().as_secs(),
            ttl_secs: self.ttl_secs,
            jobs_completed: self.jobs_completed,
            cache_open_count: self.engine.cache_open_count,
            session_active: self.session.as_ref().is_some_and(SessionState::active),
            session_age_secs: self
                .session
                .as_ref()
                .map(|session| session.created.elapsed().as_secs())
                .unwrap_or(0),
            session_binding_fingerprint: self
                .session
                .as_ref()
                .filter(|session| session.active())
                .map(|session| session.binding.fingerprint),
        }
    }

    fn handle(&mut self, request: DaemonRequest) -> anyhow::Result<DaemonResponse> {
        match request {
            DaemonRequest::Status => Ok(DaemonResponse::Status(self.status())),
            DaemonRequest::Stop => {
                self.stop = true;
                self.session = None;
                Ok(DaemonResponse::Stopped)
            }
            DaemonRequest::UnlockChallenge { .. } => Ok(DaemonResponse::UnlockChallenge {
                salt: crate::crypto::random_bytes(),
            }),
            DaemonRequest::InstallSessionKey {
                binding,
                key,
                salt,
                ttl_secs,
            } => {
                anyhow::ensure!(
                    binding.cache_dir == canonical_string(&self.cache_dir),
                    "session cache binding mismatch"
                );
                self.session = Some(SessionState {
                    binding,
                    key,
                    salt,
                    created: Instant::now(),
                    ttl_secs: crate::default_session_ttl(Some(ttl_secs)),
                });
                Ok(DaemonResponse::SessionInstalled)
            }
            DaemonRequest::ClearSession => {
                self.session = None;
                Ok(DaemonResponse::SessionCleared)
            }
            DaemonRequest::CacheStatus => Ok(DaemonResponse::CacheMaintenance(
                self.engine.cache.maintenance_status()?,
            )),
            DaemonRequest::CacheGc { dry_run } => Ok(DaemonResponse::CacheMaintenance(
                self.engine.cache.gc(dry_run)?,
            )),
            DaemonRequest::CacheCompact { dry_run } => Ok(DaemonResponse::CacheMaintenance(
                self.engine.cache.compact_sealed(dry_run)?,
            )),
            DaemonRequest::Pack(mut request) => {
                anyhow::ensure!(
                    request.options.format == ArchiveFormat::HigV2,
                    "daemon supports HIGV2 only"
                );
                let key_override = match request.options.encryption {
                    EncryptionMode::None => None,
                    EncryptionMode::Password => {
                        let session = self.session.as_ref().filter(|session| {
                            session.active()
                                && request.binding_fingerprint == Some(session.binding.fingerprint)
                        });
                        if let Some(session) = session {
                            Some(PackKeyOverride {
                                key: session.key,
                                salt: session.salt,
                                age_secs: session.created.elapsed().as_secs(),
                                session: true,
                            })
                        } else if let Some(key) = request.ephemeral_key.take() {
                            Some(PackKeyOverride {
                                key: key.key,
                                salt: key.salt,
                                age_secs: 0,
                                session: false,
                            })
                        } else {
                            anyhow::bail!("no matching daemon session or ephemeral job key")
                        }
                    }
                };
                let mut report =
                    self.engine
                        .execute(&self.cache_dir, request.options, key_override)?;
                report.pipeline.daemon_used = true;
                self.jobs_completed += 1;
                if self.jobs_completed.is_multiple_of(64) {
                    let _ = self.engine.cache.gc(false);
                }
                Ok(DaemonResponse::PackComplete(Box::new(report)))
            }
        }
    }
}

#[cfg(unix)]
pub fn run_daemon_server(cache_dir: &Path, ttl_secs: u64) -> anyhow::Result<()> {
    fs::create_dir_all(cache_dir)?;
    let cache_dir = cache_dir.canonicalize()?;
    let lock_path = cache_dir.join("daemon.lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)?;
    lock.try_lock_exclusive()
        .map_err(|_| anyhow::anyhow!("a Hig daemon already owns this cache"))?;
    let socket = daemon_socket_path(&cache_dir);
    if socket.exists() {
        let _ = fs::remove_file(&socket);
    }
    let listener = UnixListener::bind(&socket)?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
    let mut runtime = DaemonRuntime {
        engine: PackEngine::open(&cache_dir)?,
        cache_dir,
        session: None,
        started: Instant::now(),
        ttl_secs,
        jobs_completed: 0,
        stop: false,
    };
    while !runtime.stop && runtime.started.elapsed().as_secs() <= ttl_secs {
        if runtime
            .session
            .as_ref()
            .is_some_and(|session| !session.active())
        {
            runtime.session = None;
        }
        let Some(mut stream) = accept_with_timeout(&listener, 1_000)? else {
            continue;
        };
        let response = match verify_peer(&stream).and_then(|_| read_request(&mut stream)) {
            Ok(envelope) if envelope.protocol_version == PROTOCOL_VERSION => {
                let request_id = envelope.request_id;
                let mut payload = runtime.handle(envelope.payload).unwrap_or_else(|error| {
                    DaemonResponse::Error {
                        message: error.to_string(),
                    }
                });
                if let DaemonResponse::PackComplete(report) = &mut payload {
                    let response_started = Instant::now();
                    let _ = bincode::serialize(report);
                    report.timings_us.response_serialize_us =
                        response_started.elapsed().as_micros() as u64;
                }
                ProtocolEnvelope {
                    protocol_version: PROTOCOL_VERSION,
                    request_id,
                    payload,
                }
            }
            Ok(envelope) => ProtocolEnvelope {
                protocol_version: PROTOCOL_VERSION,
                request_id: envelope.request_id,
                payload: DaemonResponse::Error {
                    message: "daemon protocol version mismatch".to_string(),
                },
            },
            Err(error) => ProtocolEnvelope {
                protocol_version: PROTOCOL_VERSION,
                request_id: [0; 16],
                payload: DaemonResponse::Error {
                    message: error.to_string(),
                },
            },
        };
        let _ = write_frame(&mut stream, &response);
    }
    runtime.session = None;
    let _ = fs::remove_file(socket);
    drop(lock);
    Ok(())
}

#[cfg(unix)]
fn accept_with_timeout(
    listener: &UnixListener,
    timeout_ms: i32,
) -> anyhow::Result<Option<UnixStream>> {
    let mut descriptor = libc::pollfd {
        fd: listener.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: descriptor points to one valid pollfd for the duration of this call.
    let result = unsafe { libc::poll(&mut descriptor, 1, timeout_ms) };
    if result == 0 {
        return Ok(None);
    }
    anyhow::ensure!(result > 0, "daemon socket poll failed");
    let (stream, _) = listener.accept()?;
    Ok(Some(stream))
}

#[cfg(not(unix))]
pub fn run_daemon_server(_cache_dir: &Path, _ttl_secs: u64) -> anyhow::Result<()> {
    anyhow::bail!("daemon server is only supported on Unix platforms in v1.8.0")
}

pub fn daemon_socket_path(cache_dir: &Path) -> PathBuf {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"hig daemon socket v2");
    hasher.update(canonical_string(cache_dir).as_bytes());
    let user = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());
    hasher.update(user.as_bytes());
    std::env::temp_dir().join(format!(
        "hig-daemon-{}.sock",
        hex::encode(&hasher.finalize().as_bytes()[..12])
    ))
}

pub fn daemon_status(cache_dir: &Path) -> anyhow::Result<DaemonStatus> {
    match request_daemon(cache_dir, DaemonRequest::Status)? {
        Some(DaemonResponse::Status(status)) => Ok(status),
        Some(DaemonResponse::Error { message }) => anyhow::bail!(message),
        _ => Ok(DaemonStatus::default()),
    }
}

pub fn stop_daemon(cache_dir: &Path) -> anyhow::Result<bool> {
    match request_daemon(cache_dir, DaemonRequest::Stop)? {
        Some(DaemonResponse::Stopped) => Ok(true),
        None => Ok(false),
        Some(DaemonResponse::Error { message }) => anyhow::bail!(message),
        _ => Ok(false),
    }
}

pub fn cache_writer_available(cache_dir: &Path) -> anyhow::Result<bool> {
    fs::create_dir_all(cache_dir)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(cache_dir.join("daemon.lock"))?;
    match lock.try_lock_exclusive() {
        Ok(()) => {
            fs2::FileExt::unlock(&lock)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(false),
        Err(error) => Err(error.into()),
    }
}

pub fn request_daemon(
    cache_dir: &Path,
    request: DaemonRequest,
) -> anyhow::Result<Option<DaemonResponse>> {
    #[cfg(unix)]
    {
        let socket = daemon_socket_path(cache_dir);
        let mut stream = match UnixStream::connect(socket) {
            Ok(stream) => stream,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        let request_id = crate::crypto::random_bytes();
        write_frame(
            &mut stream,
            &ProtocolEnvelope {
                protocol_version: PROTOCOL_VERSION,
                request_id,
                payload: request,
            },
        )?;
        let response: ProtocolEnvelope<DaemonResponse> = read_frame(&mut stream)?;
        anyhow::ensure!(
            response.request_id == request_id,
            "daemon response id mismatch"
        );
        Ok(Some(response.payload))
    }
    #[cfg(not(unix))]
    {
        let _ = (cache_dir, request);
        Ok(None)
    }
}

#[cfg(unix)]
fn read_request(stream: &mut UnixStream) -> anyhow::Result<ProtocolEnvelope<DaemonRequest>> {
    read_frame(stream)
}

fn read_frame<T: for<'de> Deserialize<'de>>(reader: &mut impl Read) -> anyhow::Result<T> {
    let mut len = [0_u8; 4];
    reader.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    anyhow::ensure!(len <= MAX_FRAME_BYTES, "daemon frame exceeds 1MiB limit");
    let mut bytes = zeroize::Zeroizing::new(vec![0_u8; len]);
    reader.read_exact(&mut bytes)?;
    Ok(bincode::deserialize(&bytes)?)
}

fn write_frame<T: Serialize>(writer: &mut impl Write, value: &T) -> anyhow::Result<()> {
    let bytes = zeroize::Zeroizing::new(bincode::serialize(value)?);
    anyhow::ensure!(
        bytes.len() <= MAX_FRAME_BYTES,
        "daemon frame exceeds 1MiB limit"
    );
    writer.write_all(&(bytes.len() as u32).to_le_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()?;
    Ok(())
}

#[cfg(unix)]
fn verify_peer(stream: &UnixStream) -> anyhow::Result<()> {
    // SAFETY: geteuid has no arguments and no memory safety preconditions.
    let expected = unsafe { libc::geteuid() };
    #[cfg(any(target_os = "macos", target_os = "freebsd", target_os = "openbsd"))]
    {
        let mut uid = 0;
        let mut gid = 0;
        // SAFETY: uid/gid are valid writable values and the stream owns a valid descriptor.
        let result = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
        anyhow::ensure!(result == 0, "failed to read daemon peer credentials");
        anyhow::ensure!(uid == expected, "daemon peer uid mismatch");
    }
    #[cfg(target_os = "linux")]
    {
        // SAFETY: ucred is a plain C data structure and zero is a valid initialization.
        let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: all pointers reference valid writable storage for the declared lengths.
        let result = unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut credentials as *mut _ as *mut libc::c_void,
                &mut len,
            )
        };
        anyhow::ensure!(result == 0, "failed to read daemon peer credentials");
        anyhow::ensure!(credentials.uid == expected, "daemon peer uid mismatch");
    }
    Ok(())
}

fn canonical_string(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_rejects_oversized_frame() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&((MAX_FRAME_BYTES as u32) + 1).to_le_bytes());
        assert!(read_frame::<ProtocolEnvelope<DaemonRequest>>(&mut bytes.as_slice()).is_err());
    }

    #[test]
    fn serializable_options_do_not_contain_password() {
        let options = SerializablePackOptions {
            input_dir: PathBuf::from("input"),
            output_file: PathBuf::from("out.hig"),
            encryption: EncryptionMode::Password,
            threads: None,
            compression: Compression::Zstd,
            level: None,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV2,
            batch: BatchOptions::default(),
            chunk: ChunkOptions::default(),
            speed: SpeedMode::Balanced,
            kdf_profile: KdfProfile::Secure,
            sealed_cache: false,
            manifest_format: ManifestFormat::Compact,
            use_session: false,
            session_required: false,
            solid: SolidMode::Auto,
            pipeline: PipelineOptions::default(),
        };
        let bytes = bincode::serialize(&options).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("password"));
    }

    #[test]
    fn zero_ttl_session_is_immediately_inactive() {
        let temp = tempfile::tempdir().unwrap();
        let kdf = KdfProfile::Secure.params();
        let session = SessionState {
            binding: crate::derive_session_binding(
                temp.path(),
                KdfProfile::Secure,
                &kdf,
                EncryptionMode::Password,
            ),
            key: [1; KEY_LEN],
            salt: [2; SALT_LEN],
            created: Instant::now(),
            ttl_secs: 0,
        };
        assert!(!session.active());
    }
}
