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
    io::Write,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
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
    // Belt-and-braces: if a previous daemon is still in the middle of
    // its shutdown (workers SIGTERM'd, waiting on them, hasn't removed
    // its pid file yet), wait for it. Without this guard a rapid
    // Stop→Start can race: spanpaper sees the lingering pid file and
    // refuses to start with "daemon already running". The stop_daemon
    // path also waits, so this normally returns instantly.
    for _ in 0..100 {  // up to 5 s
        if !daemon_alive() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // --background re-execs detached, the parent returns ~immediately.
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
    // file written + process visible to kill -0). Without this, the
    // tray's 2 s liveness poll can race in *between* the parent
    // exiting and the child writing its pid file, see "not alive",
    // and clobber our optimistic `tray.daemon_running = true` — which
    // is exactly what made the Stop menu item appear greyed out after
    // a fresh Start.
    for _ in 0..40 {
        if daemon_alive() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

pub fn stop_daemon() -> std::io::Result<()> {
    // Send SIGTERM directly instead of shelling out to `spanpaper stop`,
    // then wait for the daemon to actually exit before returning. We
    // wait up to 5 s because the daemon's `worker.shutdown()` gives
    // each mpvpaper child up to 2 s to exit on SIGTERM before resorting
    // to SIGKILL — with three workers that's a 6 s theoretical worst
    // case, but mpvpaper typically exits in ~200 ms so we almost
    // always return in well under a second.
    //
    // Why wait at all (rather than fire-and-forget): until the old
    // daemon's `supervisor_loop` returns and its pid file is removed,
    // a subsequent `spanpaper start --background` reads the stale pid
    // file, sees the process still alive, and bails with "daemon
    // already running". Waiting here keeps Stop→Start sequencing sane.
    let Some(p) = pid_file() else {
        return Err(std::io::Error::other("pid file path unavailable"));
    };
    let text = fs::read_to_string(&p)?;
    let pid: i32 = text.trim().parse()
        .map_err(|_| std::io::Error::other(format!("malformed pid file: {p:?}")))?;
    kill(Pid::from_raw(pid), nix::sys::signal::Signal::SIGTERM)
        .map_err(|e| std::io::Error::other(format!("kill(SIGTERM, {pid}): {e}")))?;
    for _ in 0..100 {  // up to 5 s
        if !daemon_alive() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // Daemon still alive after 5 s — return Ok anyway; start_daemon's
    // own pre-spawn wait will retry, and the eventual state will sync
    // through the 2 s liveness poll.
    Ok(())
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

/// Set the `span_fit` config key. `value` must be "crop" / "fit" / "stretch".
/// Affects mpvpaper's vf chain on the span pair (always) AND the side
/// worker when side is a video (current daemon quirk — side_mode only
/// applies to swaybg, which is the image path).
pub fn set_span_fit(value: &str) -> std::io::Result<()> {
    let status = Command::new("spanpaper")
        .args(["set", "--span-fit", value])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("spanpaper set --span-fit {value} exited {status}")))
    }
}

/// Set the `side_mode` config key — swaybg's fit mode for when the
/// side slot is an IMAGE. Values: fill | fit | stretch | center | tile.
/// (Side videos use span_fit; see comment on `set_span_fit`.)
pub fn set_side_mode(value: &str) -> std::io::Result<()> {
    let status = Command::new("spanpaper")
        .args(["set", "--side-mode", value])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("spanpaper set --side-mode {value} exited {status}")))
    }
}

/// Toggle audio on the spanned video.
pub fn set_audio(enabled: bool) -> std::io::Result<()> {
    let flag = if enabled { "--audio" } else { "--no-audio" };
    let status = Command::new("spanpaper").args(["set", flag]).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("spanpaper set {flag} exited {status}")))
    }
}

/// Open the config folder in the user's file manager.
///
/// Tries `org.freedesktop.FileManager1.ShowFolders` over D-Bus first
/// (Nautilus, Nemo, Dolphin, Files, etc. all implement this) — that
/// guarantees a file manager opens, regardless of how the user's
/// `inode/directory` MIME default is set. Falls back to `xdg-open`
/// (which honours `xdg-mime query default inode/directory`) if no
/// FileManager1 service is available on the bus.
pub fn open_config_folder() -> std::io::Result<()> {
    let dir = dirs::config_dir()
        .map(|d| d.join("spanpaper"))
        .ok_or_else(|| std::io::Error::other("XDG config dir not set"))?;
    let uri = format!("file://{}", dir.display());

    // dbus-send is in the dbus core package — universally present on
    // every freedesktop-conforming desktop. Spawning is fire-and-forget
    // and exits ~0 ms after the D-Bus call returns.
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

    // Fall back to xdg-open if no file manager registered FileManager1.
    Command::new("xdg-open").arg(&dir).spawn().map(|_| ())
}

/// SIGHUP the daemon to make it re-read config and roll workers.
pub fn reload_daemon() -> std::io::Result<()> {
    let Some(p) = pid_file() else {
        return Err(std::io::Error::other("pid file path unavailable"));
    };
    let text = fs::read_to_string(&p)?;
    let pid: i32 = text.trim().parse()
        .map_err(|_| std::io::Error::other(format!("malformed pid file: {p:?}")))?;
    kill(Pid::from_raw(pid), nix::sys::signal::Signal::SIGHUP)
        .map_err(|e| std::io::Error::other(format!("kill(SIGHUP, {pid}): {e}")))
}

// ---- mpv IPC pause / resume -----------------------------------------------
//
// The daemon's workers expose JSON IPC at $XDG_RUNTIME_DIR/spanpaper/mpv-*.sock
// (span pair + side video; side image uses swaybg and has no IPC). The tray
// enumerates those sockets and broadcasts a pause/resume command to each.
// We accept partial failure: as long as one socket accepts the command,
// playback state changes for that monitor; the others get the next click.

fn mpv_sockets() -> Vec<PathBuf> {
    let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") else { return vec![] };
    let dir = PathBuf::from(runtime).join("spanpaper");
    let Ok(rd) = fs::read_dir(&dir) else { return vec![] };
    rd.filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("mpv-") && n.ends_with(".sock"))
                .unwrap_or(false)
        })
        .collect()
}

fn send_ipc(socket: &Path, json: &str) -> std::io::Result<()> {
    let mut s = UnixStream::connect(socket)?;
    s.set_write_timeout(Some(Duration::from_millis(500))).ok();
    let mut line = String::with_capacity(json.len() + 1);
    line.push_str(json);
    line.push('\n');
    s.write_all(line.as_bytes())?;
    s.flush().ok();
    Ok(())
}

fn broadcast_pause(paused: bool) {
    let cmd = if paused {
        r#"{"command":["set_property","pause",true]}"#
    } else {
        r#"{"command":["set_property","pause",false]}"#
    };
    let socks = mpv_sockets();
    if socks.is_empty() {
        tracing::warn!("no mpv ipc sockets found at $XDG_RUNTIME_DIR/spanpaper/");
    }
    for s in socks {
        if let Err(e) = send_ipc(&s, cmd) {
            tracing::warn!("ipc {}: {e}", s.display());
        }
    }
}

pub fn pause_playback()  { broadcast_pause(true)  }
pub fn resume_playback() { broadcast_pause(false) }
