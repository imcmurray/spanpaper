// mpv JSON IPC client — used to lockstep span workers.
//
// Each span mpvpaper instance is spawned with
//   --input-ipc-server=$XDG_RUNTIME_DIR/spanpaper/mpv-<output>.sock
//   --pause=yes --start=0
// so the file loads and the first frame decodes, then mpv sits waiting.
//
// The daemon then connects to all span sockets and broadcasts
//   {"command":["set_property","pause",false]}
// in a tight loop. Issuing all unpauses within ~1ms of each other puts
// frame 0 onto every span monitor within a fraction of a frame interval
// — independent of how long each mpvpaper took to finish its own startup
// (which varies a lot once a third mpv for a video side output joins the
// spawn batch and contends for hwdec init).
//
// No new crate dependencies: std::os::unix::net::UnixStream, raw JSON
// strings.

use anyhow::{Context, Result};
use std::{
    fs,
    io::Write,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// `$XDG_RUNTIME_DIR/spanpaper/` — same dir the pid file lives in.
/// Falls back to `/tmp/spanpaper-<uid>/` when XDG_RUNTIME_DIR is unset.
pub fn socket_dir() -> Result<PathBuf> {
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

/// Block until `path` is connectable as a unix socket, or `timeout` elapses.
/// Returns true if connectable. Polls at 25 ms granularity (~5% CPU on
/// the daemon thread for the brief window mpv is initializing).
pub fn wait_for_socket(path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() && UnixStream::connect(path).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

/// Send a single JSON line to an mpv IPC socket, fire-and-forget.
/// We don't read the reply — mpv drains the response queue itself; the
/// commands we send (`set_property pause false`, `seek …`) have no
/// payload worth blocking on.
pub fn send_command(socket: &Path, json: &str) -> Result<()> {
    let mut s = UnixStream::connect(socket)
        .with_context(|| format!("connect mpv ipc {}", socket.display()))?;
    s.set_write_timeout(Some(Duration::from_millis(500))).ok();
    let mut line = String::with_capacity(json.len() + 1);
    line.push_str(json);
    line.push('\n');
    s.write_all(line.as_bytes())
        .with_context(|| format!("write mpv ipc {}", socket.display()))?;
    s.flush().ok();
    Ok(())
}

pub fn unpause(socket: &Path) -> Result<()> {
    send_command(socket, r#"{"command":["set_property","pause",false]}"#)
}
