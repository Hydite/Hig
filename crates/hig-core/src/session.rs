use crate::crypto::{KEY_LEN, KdfParams, SALT_LEN};
use crate::{EncryptionMode, KdfProfile};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};

const DEFAULT_TTL_SECS: u64 = 1_800;
const MAX_TTL_SECS: u64 = 7_200;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionBinding {
    pub fingerprint: [u8; 32],
    pub cache_dir: String,
    pub kdf_profile: KdfProfile,
    pub kdf: KdfParams,
    pub encryption: EncryptionMode,
    pub hig_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionMaterial {
    pub binding: SessionBinding,
    pub key: [u8; KEY_LEN],
    pub salt: [u8; SALT_LEN],
    pub created_unix_secs: u64,
    pub ttl_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLookup {
    pub key: [u8; KEY_LEN],
    pub salt: [u8; SALT_LEN],
    pub age_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum SessionRequest {
    Lookup { fingerprint: [u8; 32] },
    Status,
    Clear,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum SessionResponse {
    LookupHit {
        key: [u8; KEY_LEN],
        salt: [u8; SALT_LEN],
        age_secs: u64,
    },
    LookupMiss {
        reason: String,
    },
    Status {
        active: bool,
        age_secs: u64,
        ttl_secs: u64,
    },
    Cleared,
    Error {
        message: String,
    },
}

pub fn default_session_ttl(ttl: Option<u64>) -> u64 {
    ttl.unwrap_or(DEFAULT_TTL_SECS).min(MAX_TTL_SECS)
}

pub fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

pub fn derive_session_binding(
    cache_dir: &Path,
    kdf_profile: KdfProfile,
    kdf: &KdfParams,
    encryption: EncryptionMode,
) -> SessionBinding {
    let cache_dir = cache_dir
        .canonicalize()
        .unwrap_or_else(|_| cache_dir.to_path_buf());
    let cache_dir_string = cache_dir.to_string_lossy().to_string();
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"hig session binding v1");
    hasher.update(cache_dir_string.as_bytes());
    hasher.update(format!("{kdf_profile:?}:{encryption:?}:1.6.0").as_bytes());
    hasher.update(&kdf.memory_cost_kib.to_le_bytes());
    hasher.update(&kdf.time_cost.to_le_bytes());
    hasher.update(&kdf.parallelism.to_le_bytes());
    SessionBinding {
        fingerprint: *hasher.finalize().as_bytes(),
        cache_dir: cache_dir_string,
        kdf_profile,
        kdf: kdf.clone(),
        encryption,
        hig_version: "1.6.0".to_string(),
    }
}

pub fn session_socket_path(cache_dir: &Path) -> PathBuf {
    let cache_dir = cache_dir
        .canonicalize()
        .unwrap_or_else(|_| cache_dir.to_path_buf());
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"hig session socket v1");
    hasher.update(cache_dir.to_string_lossy().as_bytes());
    let user = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());
    hasher.update(user.as_bytes());
    let hex = hex::encode(&hasher.finalize().as_bytes()[..12]);
    std::env::temp_dir().join(format!("hig-session-{hex}.sock"))
}

pub fn lookup_session(
    cache_dir: &Path,
    binding: &SessionBinding,
) -> anyhow::Result<Option<SessionLookup>> {
    match request(
        cache_dir,
        &SessionRequest::Lookup {
            fingerprint: binding.fingerprint,
        },
    )? {
        Some(SessionResponse::LookupHit {
            key,
            salt,
            age_secs,
        }) => Ok(Some(SessionLookup {
            key,
            salt,
            age_secs,
        })),
        Some(SessionResponse::LookupMiss { .. }) | None => Ok(None),
        Some(SessionResponse::Error { message }) => anyhow::bail!(message),
        Some(_) => Ok(None),
    }
}

pub fn session_status(cache_dir: &Path) -> anyhow::Result<Option<(u64, u64)>> {
    match request(cache_dir, &SessionRequest::Status)? {
        Some(SessionResponse::Status {
            active: true,
            age_secs,
            ttl_secs,
        }) => Ok(Some((age_secs, ttl_secs))),
        Some(SessionResponse::Status { active: false, .. }) | None => Ok(None),
        Some(SessionResponse::Error { message }) => anyhow::bail!(message),
        Some(_) => Ok(None),
    }
}

pub fn clear_session(cache_dir: &Path) -> anyhow::Result<bool> {
    match request(cache_dir, &SessionRequest::Clear)? {
        Some(SessionResponse::Cleared) => Ok(true),
        None => Ok(false),
        Some(SessionResponse::Error { message }) => anyhow::bail!(message),
        Some(_) => Ok(false),
    }
}

#[cfg(unix)]
pub fn run_session_server(socket_path: &Path, material: SessionMaterial) -> anyhow::Result<()> {
    if socket_path.exists() {
        let _ = fs::remove_file(socket_path);
    }
    let listener = UnixListener::bind(socket_path)?;
    fs::set_permissions(socket_path, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let started = now_unix_secs();
    loop {
        if now_unix_secs().saturating_sub(started) >= material.ttl_secs {
            break;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                if handle_stream(stream, &material)? {
                    break;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                // Keep expiry checks responsive without adding a visible fixed cost to each pack.
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(error) => return Err(error.into()),
        }
    }
    let _ = fs::remove_file(socket_path);
    Ok(())
}

#[cfg(not(unix))]
pub fn run_session_server(_socket_path: &Path, _material: SessionMaterial) -> anyhow::Result<()> {
    anyhow::bail!("session server is only supported on Unix platforms in v1.6.0")
}

#[cfg(unix)]
fn handle_stream(mut stream: UnixStream, material: &SessionMaterial) -> anyhow::Result<bool> {
    let mut line = String::new();
    {
        let mut reader = BufReader::new(&mut stream);
        reader.read_line(&mut line)?;
    }
    let request: SessionRequest = serde_json::from_str(line.trim())?;
    let now = now_unix_secs();
    let age_secs = now.saturating_sub(material.created_unix_secs);
    let response = match request {
        SessionRequest::Lookup { fingerprint } => {
            if fingerprint == material.binding.fingerprint && age_secs <= material.ttl_secs {
                SessionResponse::LookupHit {
                    key: material.key,
                    salt: material.salt,
                    age_secs,
                }
            } else {
                SessionResponse::LookupMiss {
                    reason: "session binding mismatch or expired".to_string(),
                }
            }
        }
        SessionRequest::Status => SessionResponse::Status {
            active: age_secs <= material.ttl_secs,
            age_secs,
            ttl_secs: material.ttl_secs,
        },
        SessionRequest::Clear => SessionResponse::Cleared,
    };
    writeln!(stream, "{}", serde_json::to_string(&response)?)?;
    Ok(matches!(response, SessionResponse::Cleared))
}

#[cfg(unix)]
fn request(cache_dir: &Path, request: &SessionRequest) -> anyhow::Result<Option<SessionResponse>> {
    let socket = session_socket_path(cache_dir);
    if !socket.exists() {
        return Ok(None);
    }
    let mut stream = match UnixStream::connect(socket) {
        Ok(stream) => stream,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    writeln!(stream, "{}", serde_json::to_string(request)?)?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    if line.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(line.trim())?))
}

#[cfg(not(unix))]
fn request(
    _cache_dir: &Path,
    _request: &SessionRequest,
) -> anyhow::Result<Option<SessionResponse>> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ttl_defaults_and_caps() {
        assert_eq!(default_session_ttl(None), 1_800);
        assert_eq!(default_session_ttl(Some(10)), 10);
        assert_eq!(default_session_ttl(Some(99_999)), 7_200);
    }

    #[test]
    fn binding_changes_with_cache_dir() {
        let temp_a = tempfile::tempdir().unwrap();
        let temp_b = tempfile::tempdir().unwrap();
        let kdf = KdfProfile::Secure.params();
        let a = derive_session_binding(
            temp_a.path(),
            KdfProfile::Secure,
            &kdf,
            EncryptionMode::Password,
        );
        let b = derive_session_binding(
            temp_b.path(),
            KdfProfile::Secure,
            &kdf,
            EncryptionMode::Password,
        );
        assert_ne!(a.fingerprint, b.fingerprint);
    }

    #[cfg(unix)]
    #[test]
    fn session_server_returns_memory_key_and_clears() {
        let temp = tempfile::tempdir().unwrap();
        let cache_dir = temp.path().join("cache");
        fs::create_dir_all(&cache_dir).unwrap();
        let kdf = KdfProfile::Secure.params();
        let binding = derive_session_binding(
            &cache_dir,
            KdfProfile::Secure,
            &kdf,
            EncryptionMode::Password,
        );
        let socket = session_socket_path(&cache_dir);
        let material = SessionMaterial {
            binding: binding.clone(),
            key: [7; KEY_LEN],
            salt: [8; SALT_LEN],
            created_unix_secs: now_unix_secs(),
            ttl_secs: 30,
        };
        let server_socket = socket.clone();
        let server = std::thread::spawn(move || run_session_server(&server_socket, material));
        for _ in 0..100 {
            if socket.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }

        let mode = fs::metadata(&socket).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let lookup = lookup_session(&cache_dir, &binding).unwrap().unwrap();
        assert_eq!(lookup.key, [7; KEY_LEN]);
        assert_eq!(lookup.salt, [8; SALT_LEN]);
        assert!(clear_session(&cache_dir).unwrap());
        server.join().unwrap().unwrap();
        assert!(!socket.exists());
    }
}
