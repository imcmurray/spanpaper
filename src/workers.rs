// Subprocess supervisors.
//
// We spawn one mpvpaper per video output (with a per-monitor crop filter so a
// single source MP4 is sliced across the stack) and one swaybg for the static
// image. Each is wrapped in a Worker with a restart-on-crash policy and a
// graceful-shutdown SIGTERM path.
//
// Why mpvpaper instead of binding libmpv ourselves: mpvpaper already plumbs
// libmpv's render API into an EGL'd wlr-layer-shell surface. It is the
// upstream-accepted reference for this exact use case. Wrapping it gives us
// reliable hardware-accelerated playback in a fraction of the code we'd need
// to write from scratch — and it ships in the Arch repos.

use anyhow::{Context, Result};
use nix::{
    sys::signal::{kill, Signal},
    unistd::Pid,
};
use std::{
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use crate::config::{Config, SpanDirection};

pub struct Worker {
    pub label: String,
    pub kind: WorkerKind,
    pub child: Child,
    started_at: Instant,
    /// crashes within the last minute; clears on a clean run
    recent_failures: u32,
}

#[derive(Debug, Clone)]
pub enum WorkerKind {
    /// (output, video path, crop spec, audio, fit mode, extra opts)
    Video {
        output: String,
        video: std::path::PathBuf,
        crop: String,
        audio: bool,
        fit: String,
        extra: Vec<String>,
    },
    /// (output, image path, fit mode)
    Image {
        output: String,
        image: std::path::PathBuf,
        mode: String,
    },
}

impl Worker {
    pub fn spawn(kind: WorkerKind) -> Result<Self> {
        let (label, child) = match &kind {
            WorkerKind::Video { output, video, crop, audio, fit, extra } => {
                (format!("video:{output}"),
                 spawn_video(output, video, crop, *audio, fit, extra)?)
            }
            WorkerKind::Image { output, image, mode } => {
                (format!("image:{output}"),
                 spawn_image(output, image, mode)?)
            }
        };
        Ok(Self {
            label,
            kind,
            child,
            started_at: Instant::now(),
            recent_failures: 0,
        })
    }

    /// Returns `Ok(true)` if the worker exited and was successfully restarted.
    /// Returns `Ok(false)` if the worker is still alive. Errors propagate from
    /// restart attempts that themselves fail (e.g. binary missing).
    pub fn poll_and_maybe_restart(&mut self) -> Result<bool> {
        match self.child.try_wait().context("waitpid worker")? {
            None => Ok(false),
            Some(status) => {
                let uptime = self.started_at.elapsed();
                tracing::warn!(
                    "{}: exited {:?} after {:.1}s",
                    self.label, status, uptime.as_secs_f32()
                );

                // Window the failure counter: a clean 60s run resets it so
                // long-lived crashes don't permanently disable the worker.
                if uptime > Duration::from_secs(60) {
                    self.recent_failures = 0;
                }
                self.recent_failures = self.recent_failures.saturating_add(1);

                if self.recent_failures > 5 {
                    anyhow::bail!(
                        "{}: 5 rapid restarts; giving up (check binary & paths)",
                        self.label
                    );
                }

                // Linear backoff, capped at 5s.
                let backoff = Duration::from_millis(500u64.saturating_mul(self.recent_failures as u64));
                std::thread::sleep(backoff.min(Duration::from_secs(5)));

                let new = Self::spawn(self.kind.clone())?;
                self.child = new.child;
                self.started_at = Instant::now();
                tracing::info!("{}: restarted", self.label);
                Ok(true)
            }
        }
    }

    /// Send SIGTERM, then SIGKILL if it doesn't exit within ~2s.
    pub fn shutdown(mut self) {
        let pid = Pid::from_raw(self.child.id() as i32);
        let _ = kill(pid, Signal::SIGTERM);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if Instant::now() >= deadline => break,
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => {
                    tracing::warn!("{}: waitpid error: {e}", self.label);
                    return;
                }
            }
        }
        tracing::warn!("{}: SIGTERM ignored, sending SIGKILL", self.label);
        let _ = kill(pid, Signal::SIGKILL);
        let _ = self.child.wait();
    }
}

/// Compute the mpv `vf=crop=` expression for the given slice index, total
/// slices, and direction. Uses `iw`/`ih` so it works regardless of the source
/// resolution.
pub fn crop_spec(index: usize, total: usize, dir: SpanDirection) -> String {
    let total = total.max(1) as i32;
    let i = index as i32;
    match dir {
        SpanDirection::Vertical => {
            // height divided into `total` horizontal strips, take the i-th.
            //  crop=W:H:X:Y
            format!("crop=iw:ih/{t}:0:ih*{i}/{t}", t = total, i = i)
        }
        SpanDirection::Horizontal => {
            format!("crop=iw/{t}:ih:iw*{i}/{t}:0", t = total, i = i)
        }
    }
}

/// Plan workers for the current config (without spawning them yet).
pub fn plan(cfg: &Config) -> Result<Vec<WorkerKind>> {
    let mut plan = Vec::new();

    let video = cfg.video.as_ref().context("config.video unset")?;
    let total = cfg.span_outputs.len();
    for (i, out) in cfg.span_outputs.iter().enumerate() {
        plan.push(WorkerKind::Video {
            output: out.clone(),
            video: video.clone(),
            crop: crop_spec(i, total, cfg.span_direction),
            audio: cfg.audio && i == 0, // only one instance produces audio
            fit: cfg.video_fit.clone(),
            extra: cfg.extra_mpv_options.clone(),
        });
    }

    if let (Some(out), Some(img)) = (&cfg.image_output, &cfg.left_image) {
        plan.push(WorkerKind::Image {
            output: out.clone(),
            image: img.clone(),
            mode: cfg.image_mode.clone(),
        });
    }

    Ok(plan)
}

fn spawn_video(
    output: &str,
    video: &Path,
    crop: &str,
    audio: bool,
    fit: &str,
    extra: &[String],
) -> Result<Child> {
    let bin = which::which("mpvpaper")
        .context("`mpvpaper` not found on PATH (install: pacman -S mpvpaper)")?;

    // Build the `-o` mpv-options string. mpvpaper passes this verbatim to mpv,
    // so we keep it as one space-separated string. Values containing spaces
    // are not expected here (crop uses `:` and `/`, no whitespace).
    let mut opts: Vec<String> = vec![
        "loop-file=inf".into(),
        "hwdec=auto-safe".into(),
        format!("vf={crop}"),
    ];
    if !audio {
        opts.push("no-audio".into());
        opts.push("mute=yes".into());
    } else {
        opts.push("volume=100".into());
    }
    match fit {
        "stretch" => opts.push("keepaspect=no".into()),
        "fit"     => { /* default mpv behavior; letterboxes */ }
        "crop" | _ => {
            opts.push("panscan=1.0".into());
            opts.push("keepaspect=yes".into());
        }
    }
    // Quiet by default; user gets per-worker stderr only on crashes.
    opts.push("really-quiet=yes".into());
    opts.push("force-window=no".into());

    for e in extra {
        opts.push(e.clone());
    }

    let opt_str = opts.join(" ");

    let mut cmd = Command::new(bin);
    cmd.arg("-o").arg(&opt_str)
       .arg(output)
       .arg(video)
       .stdin(Stdio::null())
       .stdout(Stdio::null())
       .stderr(Stdio::inherit());

    tracing::info!("spawning video worker: mpvpaper -o {:?} {} {}",
                   opt_str, output, video.display());
    cmd.spawn().context("spawn mpvpaper")
}

fn spawn_image(output: &str, image: &Path, mode: &str) -> Result<Child> {
    let bin = which::which("swaybg")
        .context("`swaybg` not found on PATH (install: pacman -S swaybg)")?;

    let mut cmd = Command::new(bin);
    cmd.arg("-o").arg(output)
       .arg("-i").arg(image)
       .arg("-m").arg(mode)
       .stdin(Stdio::null())
       .stdout(Stdio::null())
       .stderr(Stdio::inherit());

    tracing::info!("spawning image worker: swaybg -o {} -i {} -m {}",
                   output, image.display(), mode);
    cmd.spawn().context("spawn swaybg")
}
