//! Cross-binary daemon-state probes.
//!
//! Both the daemon (for its `current_pid` / `stop` / `reload` flows)
//! and the tray applet (for the panel-icon liveness indicator and the
//! Start/Stop menu items) need to know whether the daemon is alive and
//! where its pid file lives. Before v0.4.0 each binary had its own
//! near-identical copy of this logic; now both consume this one
//! module.

use anyhow::{Context, Result};
use nix::{sys::signal::kill, unistd::Pid};
use std::{fs, path::PathBuf};

/// `$XDG_RUNTIME_DIR/spanpaper/` — the directory the daemon writes its
/// pid file and mpv IPC sockets into. Falls back to `/tmp/spanpaper-<uid>/`
/// when XDG_RUNTIME_DIR is unset (rare; mostly tests / cron).
pub fn runtime_dir() -> Result<PathBuf> {
    if let Ok(d) = std::env::var("XDG_RUNTIME_DIR") {
        let p = PathBuf::from(d).join("spanpaper");
        fs::create_dir_all(&p).ok();
        return Ok(p);
    }
    let uid = nix::unistd::getuid().as_raw();
    let p = PathBuf::from(format!("/tmp/spanpaper-{uid}"));
    fs::create_dir_all(&p).ok();
    Ok(p)
}

pub fn pid_file_path() -> Result<PathBuf> {
    Ok(runtime_dir()?.join("spanpaper.pid"))
}

/// Read the pid file and signal-0 the pid to verify the process exists.
/// Returns the pid on success; Err if there's no pid file, the pid is
/// malformed, or the process is dead. Callers that just want a boolean
/// answer should use [`daemon_alive`].
pub fn current_pid() -> Result<i32> {
    let p = pid_file_path()?;
    let text = fs::read_to_string(&p)
        .with_context(|| format!("daemon not running (no pid file at {})", p.display()))?;
    let pid: i32 = text.trim().parse().context("malformed pid file")?;
    kill(Pid::from_raw(pid), None)
        .with_context(|| format!("daemon not running (pid {pid} dead)"))?;
    Ok(pid)
}

/// Cheap "is the daemon running?" check — no subprocess, no Wayland
/// round-trip. Used by the tray's 2 s poll loop and its menu's
/// enabled-state computation.
pub fn daemon_alive() -> bool {
    current_pid().is_ok()
}
