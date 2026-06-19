use std::path::Path;
use std::time::Duration;

#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::net::UnixListener;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DaemonStatus {
    pub active: bool,
    pub age_secs: u64,
    pub ttl_secs: u64,
}

#[cfg(unix)]
pub fn run_daemon_server(socket_path: &Path, ttl_secs: u64) -> anyhow::Result<()> {
    if socket_path.exists() {
        let _ = fs::remove_file(socket_path);
    }
    let listener = UnixListener::bind(socket_path)?;
    fs::set_permissions(socket_path, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let started = crate::session::now_unix_secs();
    loop {
        if crate::session::now_unix_secs().saturating_sub(started) >= ttl_secs {
            break;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                // v1.7 daemon intentionally reuses the session protocol. A daemon without an
                // unlocked key reports inactive to session clients but keeps the hot socket alive.
                drop(stream);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => return Err(error.into()),
        }
    }
    let _ = fs::remove_file(socket_path);
    Ok(())
}

#[cfg(not(unix))]
pub fn run_daemon_server(_socket_path: &Path, _ttl_secs: u64) -> anyhow::Result<()> {
    anyhow::bail!("daemon server is only supported on Unix platforms in v1.7.0")
}

pub fn daemon_status(cache_dir: &Path) -> anyhow::Result<DaemonStatus> {
    if let Some((age_secs, ttl_secs)) = crate::session::session_status(cache_dir)? {
        Ok(DaemonStatus {
            active: true,
            age_secs,
            ttl_secs,
        })
    } else {
        Ok(DaemonStatus::default())
    }
}

pub fn stop_daemon(cache_dir: &Path) -> anyhow::Result<bool> {
    crate::session::clear_session(cache_dir)
}
