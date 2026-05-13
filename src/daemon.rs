// Daemon: owns the lifecycle of every worker and the pid file.
//
//   * SIGTERM/SIGINT → graceful shutdown, kill workers, remove pid file
//   * SIGHUP         → reload config and roll workers
//   * any worker exits → restart with backoff (workers::Worker)
//
// Single-threaded poll loop at 200ms — zero CPU when idle, no async runtime.
// Signal handlers write into static AtomicBools; the loop checks them each tick.

use anyhow::{Context, Result};
use nix::{
    sys::signal::{kill, Signal},
    unistd::Pid,
};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Duration,
};

use crate::{
    config::Config,
    outputs,
    workers::{self, Worker},
};

static STOP: AtomicBool = AtomicBool::new(false);
static RELOAD: AtomicBool = AtomicBool::new(false);

fn runtime_dir() -> Result<PathBuf> {
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

fn pid_file() -> Result<PathBuf> {
    Ok(runtime_dir()?.join("spanpaper.pid"))
}

/// Read the pid file and verify the process is alive.
pub fn current_pid() -> Result<i32> {
    let p = pid_file()?;
    let text = fs::read_to_string(&p)
        .with_context(|| format!("daemon not running (no pid file at {})", p.display()))?;
    let pid: i32 = text.trim().parse().context("malformed pid file")?;
    // Signal 0 = existence check.
    kill(Pid::from_raw(pid), None)
        .with_context(|| format!("daemon not running (pid {pid} dead)"))?;
    Ok(pid)
}

pub fn reload() -> Result<()> {
    let pid = current_pid()?;
    kill(Pid::from_raw(pid), Signal::SIGHUP).context("SIGHUP to daemon")?;
    Ok(())
}

pub fn stop() -> Result<()> {
    let pid = current_pid()?;
    kill(Pid::from_raw(pid), Signal::SIGTERM).context("SIGTERM to daemon")?;
    for _ in 0..50 {
        if current_pid().is_err() {
            tracing::info!("daemon stopped");
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    anyhow::bail!("daemon did not exit within 5s; pid {pid} still running");
}

/// Re-exec ourselves detached so the caller returns immediately.
pub fn spawn_background() -> Result<()> {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe().context("current_exe")?;
    let child = unsafe {
        Command::new(&exe)
            .arg("start")
            .arg("--background")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .pre_exec(|| {
                // New session → detached from controlling terminal.
                nix::unistd::setsid().ok();
                Ok(())
            })
            .spawn()
            .context("spawn background daemon")?
    };
    tracing::info!("background daemon started (pid {})", child.id());
    Ok(())
}

/// Run the daemon. With `background = true` from a TTY we re-exec detached and
/// return; otherwise we run the supervisor loop in-process.
pub fn run(background: bool) -> Result<()> {
    if background && nix::unistd::isatty(0).unwrap_or(false) {
        return spawn_background();
    }

    if let Ok(pid) = current_pid() {
        anyhow::bail!("daemon already running (pid {pid}); use `spanpaper stop` first");
    }

    let pid_path = pid_file()?;
    write_pid_file(&pid_path)?;

    install_signal_handlers()?;

    let result = supervisor_loop();

    let _ = fs::remove_file(&pid_path);
    result
}

fn supervisor_loop() -> Result<()> {
    let mut workers = start_workers()?;

    tracing::info!(
        "daemon running with {} worker(s); SIGTERM to stop, SIGHUP to reload",
        workers.len()
    );

    loop {
        if STOP.load(Ordering::Relaxed) {
            tracing::info!("shutdown requested; stopping {} worker(s)", workers.len());
            for w in workers.drain(..) {
                w.shutdown();
            }
            return Ok(());
        }

        if RELOAD.swap(false, Ordering::Relaxed) {
            tracing::info!("reload requested (SIGHUP); restarting workers");
            for w in workers.drain(..) {
                w.shutdown();
            }
            match start_workers() {
                Ok(w) => workers = w,
                Err(e) => {
                    tracing::error!("reload failed: {e:#}; daemon exiting");
                    return Err(e);
                }
            }
        }

        for w in workers.iter_mut() {
            if let Err(e) = w.poll_and_maybe_restart() {
                tracing::error!("{}: {e:#}", w.label);
            }
        }

        thread::sleep(Duration::from_millis(200));
    }
}

fn start_workers() -> Result<Vec<Worker>> {
    let cfg = Config::load()
        .context("load config (run `spanpaper set --video PATH ...` first)")?;
    cfg.validate()?;

    let detected = outputs::detect().unwrap_or_default();
    let names: Vec<&str> = detected.iter().map(|o| o.name.as_str()).collect();
    for want in cfg.span_outputs.iter().chain(cfg.side_output.iter()) {
        if !names.iter().any(|n| n == want) {
            tracing::warn!(
                "configured output {:?} not currently present (have: {:?})",
                want, names
            );
        }
    }

    let plan = workers::plan(&cfg, &detected)?;
    let mut spawned = Vec::with_capacity(plan.len());
    for kind in plan {
        match Worker::spawn(kind) {
            Ok(w) => spawned.push(w),
            Err(e) => {
                tracing::error!("worker spawn failed: {e:#}");
                for s in spawned {
                    s.shutdown();
                }
                return Err(e);
            }
        }
    }
    Ok(spawned)
}

fn write_pid_file(path: &PathBuf) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let mut f = fs::File::create(path)
        .with_context(|| format!("create pid file {}", path.display()))?;
    writeln!(f, "{}", std::process::id()).context("write pid")?;
    Ok(())
}

// ---- Signal handling ---------------------------------------------------------

extern "C" fn on_term(_: i32) {
    STOP.store(true, Ordering::Relaxed);
}
extern "C" fn on_hup(_: i32) {
    RELOAD.store(true, Ordering::Relaxed);
}

fn install_signal_handlers() -> Result<()> {
    use nix::sys::signal::{sigaction, SaFlags, SigAction, SigHandler, SigSet};
    let term = SigAction::new(
        SigHandler::Handler(on_term),
        SaFlags::SA_RESTART,
        SigSet::empty(),
    );
    let hup = SigAction::new(
        SigHandler::Handler(on_hup),
        SaFlags::SA_RESTART,
        SigSet::empty(),
    );
    unsafe {
        sigaction(Signal::SIGTERM, &term).context("install SIGTERM handler")?;
        sigaction(Signal::SIGINT,  &term).context("install SIGINT handler")?;
        sigaction(Signal::SIGHUP,  &hup).context("install SIGHUP handler")?;
    }
    Ok(())
}
