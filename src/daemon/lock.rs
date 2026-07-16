use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;

use anyhow::Context;

use crate::config::ipc_socket_path;

/// Exclusive flock held for the lifetime of the daemon process.
pub struct DaemonLock {
    _file: File,
}

fn lock_path() -> PathBuf {
    let mut path = ipc_socket_path();
    path.set_extension("lock");
    path
}

/// Acquire an exclusive non-blocking lock. Returns `Ok(None)` if another daemon holds it.
pub fn try_acquire() -> anyhow::Result<Option<DaemonLock>> {
    let path = lock_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o644)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;

    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        return Ok(None);
    }

    let _ = writeln!(file, "{}", std::process::id());
    let _ = file.flush();
    Ok(Some(DaemonLock { _file: file }))
}
