//! Daemon client for the tray applet.
//!
//! The tray is a CLI client of the daemon — for actions it shells out to
//! `spanpaper start --background` / `spanpaper stop`, and for liveness
//! checks it reads the daemon's pid file directly (no subprocess, no
//! Wayland round-trip). Reading the pid file matches what
//! `crate::daemon::current_pid` does — duplicating the logic here keeps
//! the tray a pure consumer of the public CLI contract instead of
//! pulling the daemon module into the tray binary.

use nix::{sys::signal::kill, unistd::Pid};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

/// Which slot of the wallpaper configuration a drop targets.
#[derive(Copy, Clone, Debug)]
pub enum Slot {
    Span,
    Side,
}

impl Slot {
    fn flag(self) -> &'static str {
        match self {
            Slot::Span => "--span",
            Slot::Side => "--side",
        }
    }
}

fn pid_file() -> Option<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
    Some(PathBuf::from(runtime).join("spanpaper").join("spanpaper.pid"))
}

/// True iff the pid file exists and points at a live process. Uses
/// `kill(pid, signal 0)` — a no-op syscall that succeeds iff the
/// caller can signal the pid, i.e. the process exists. Costs one
/// `read` and one `kill` syscall; safe to call every 2 s from a poll
/// loop.
pub fn daemon_alive() -> bool {
    let Some(p) = pid_file() else { return false };
    let Ok(text) = fs::read_to_string(&p) else { return false };
    let Ok(pid) = text.trim().parse::<i32>() else { return false };
    kill(Pid::from_raw(pid), None).is_ok()
}

pub fn start_daemon() -> std::io::Result<()> {
    // --background re-execs detached, returns immediately.
    Command::new("spanpaper")
        .args(["start", "--background"])
        .status()
        .map(|_| ())
}

pub fn stop_daemon() -> std::io::Result<()> {
    // Blocks up to 5 s waiting for the daemon to exit. Acceptable for
    // M2; a future milestone can move this into a tokio task so the
    // menu doesn't hang.
    Command::new("spanpaper").arg("stop").status().map(|_| ())
}

/// Assign a file to a slot. Shells out to `spanpaper set --span PATH`
/// or `--side PATH`; the daemon does the atomic config write and
/// SIGHUPs itself, returning in tens of milliseconds. Used by the
/// drop targets in the palette.
pub fn set_for(slot: Slot, path: &Path) -> std::io::Result<()> {
    Command::new("spanpaper")
        .args(["set", slot.flag()])
        .arg(path)
        .status()
        .and_then(|s| {
            if s.success() {
                Ok(())
            } else {
                Err(std::io::Error::other(format!(
                    "spanpaper set {} exited {s}",
                    slot.flag()
                )))
            }
        })
}
