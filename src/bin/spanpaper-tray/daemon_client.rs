//! Daemon client for the tray applet.
//!
//! The tray is a CLI client of the daemon — for actions it shells out
//! to `spanpaper start --background` / `spanpaper stop` / `spanpaper
//! set …`; for liveness it uses the shared `spanpaper::state` lib
//! module; for pause/resume it broadcasts on the shared
//! `spanpaper::ipc` socket helpers.
//!
//! Before v0.4.0 this module reimplemented its own pid-file probe,
//! mpv IPC client, and socket enumeration. All of those have moved
//! into the lib so both binaries share one source of truth.

use nix::{sys::signal::kill, unistd::Pid};
use spanpaper::{ipc, state};
use std::{
    path::Path,
    process::Command,
    time::Duration,
};

/// Which slot of the wallpaper configuration a drop or picker targets.
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

// ---- liveness ---------------------------------------------------------------
// Thin re-export so the rest of the tray's call sites stay unchanged
// after the lib split.

pub fn daemon_alive() -> bool {
    state::daemon_alive()
}

// ---- daemon lifecycle (shelled out via the CLI) -----------------------------

pub fn start_daemon() -> std::io::Result<()> {
    // Belt-and-braces: if a previous daemon is still in the middle of
    // its shutdown, wait for it. Without this guard a rapid Stop→Start
    // can race: spanpaper sees the lingering pid file and refuses to
    // start with "daemon already running". stop_daemon also waits, so
    // this is normally instant.
    for _ in 0..100 {  // up to 5 s
        if !daemon_alive() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let status = Command::new("spanpaper")
        .args(["start", "--background"])
        .status()?;
    if !status.success() {
        return Err(std::io::Error::other(format!(
            "spanpaper start --background exited {status} \
             (daemon may already be running or in the middle of shutdown)"
        )));
    }

    // Spin briefly for the child daemon to actually become alive (pid
    // file written + process visible to kill -0).
    for _ in 0..40 {
        if daemon_alive() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

pub fn stop_daemon() -> std::io::Result<()> {
    // SIGTERM directly via the pid we read from the lib, then poll for
    // exit (so a follow-up Start sees a clean pid-file state). 5 s
    // covers the worst case: 3 mpv workers × 2 s SIGTERM grace each
    // before the daemon falls back to SIGKILL.
    let pid = match state::current_pid() {
        Ok(p) => p,
        Err(e) => {
            return Err(std::io::Error::other(format!("daemon not running: {e}")));
        }
    };
    kill(Pid::from_raw(pid), nix::sys::signal::Signal::SIGTERM)
        .map_err(|e| std::io::Error::other(format!("kill(SIGTERM, {pid}): {e}")))?;
    for _ in 0..100 {  // up to 5 s
        if !daemon_alive() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

pub fn reload_daemon() -> std::io::Result<()> {
    let pid = state::current_pid()
        .map_err(|e| std::io::Error::other(format!("daemon not running: {e}")))?;
    kill(Pid::from_raw(pid), nix::sys::signal::Signal::SIGHUP)
        .map_err(|e| std::io::Error::other(format!("kill(SIGHUP, {pid}): {e}")))
}

// ---- config edits (shelled out to `spanpaper set`) --------------------------

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

pub fn set_span_fit(value: &str) -> std::io::Result<()> {
    spanpaper_set(&["--span-fit", value])
}

pub fn set_side_fit(value: &str) -> std::io::Result<()> {
    spanpaper_set(&["--side-fit", value])
}

pub fn set_audio(enabled: bool) -> std::io::Result<()> {
    spanpaper_set(&[if enabled { "--audio" } else { "--no-audio" }])
}

fn spanpaper_set(args: &[&str]) -> std::io::Result<()> {
    let status = Command::new("spanpaper")
        .arg("set")
        .args(args)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("spanpaper set {args:?} exited {status}")))
    }
}

// ---- "Open config folder" via FileManager1 D-Bus ---------------------------

/// Tries `org.freedesktop.FileManager1.ShowFolders` first (Nautilus /
/// Nemo / Dolphin / Files / etc. all implement it) so we open a real
/// file manager regardless of how the user's `inode/directory` MIME
/// default is set. Falls back to `xdg-open`.
pub fn open_config_folder() -> std::io::Result<()> {
    let dir = dirs::config_dir()
        .map(|d| d.join("spanpaper"))
        .ok_or_else(|| std::io::Error::other("XDG config dir not set"))?;
    let uri = format!("file://{}", dir.display());

    let dbus_status = Command::new("dbus-send")
        .args([
            "--session",
            "--print-reply",
            "--dest=org.freedesktop.FileManager1",
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1.ShowFolders",
            &format!("array:string:{uri}"),
            "string:",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    if matches!(dbus_status, Ok(s) if s.success()) {
        return Ok(());
    }

    Command::new("xdg-open").arg(&dir).spawn().map(|_| ())
}

// ---- mpv IPC pause / resume -------------------------------------------------
// Pre-v0.4.0 this enumerated $XDG_RUNTIME_DIR/spanpaper/mpv-*.sock and
// did its own UnixStream-based JSON IPC. That logic is now in
// `spanpaper::ipc` — both the daemon (for sync-unpause) and the tray
// use it.

fn broadcast(set_pause: bool) {
    let socks = ipc::enumerate_sockets();
    if socks.is_empty() {
        tracing::warn!("no mpv ipc sockets found at $XDG_RUNTIME_DIR/spanpaper/");
        return;
    }
    for s in socks {
        let r = if set_pause { ipc::pause(&s) } else { ipc::unpause(&s) };
        if let Err(e) = r {
            tracing::warn!("ipc {}: {e:#}", s.display());
        }
    }
}

pub fn pause_playback()  { broadcast(true)  }
pub fn resume_playback() { broadcast(false) }
