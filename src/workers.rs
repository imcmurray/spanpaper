// Subprocess supervisors.
//
// Worker dispatch is driven by `MediaKind` per slot:
//
//   span outputs (always mpvpaper, with per-monitor crop):
//     · Video → libmpv decode + crop
//     · Image → libmpv with image-display-duration=inf (single frame held)
//
//   side output (one of these, depending on content):
//     · Image → swaybg (lighter than libmpv for a still)
//     · Video → mpvpaper, no crop
//
// Each worker is wrapped in a `Worker` with a restart-on-crash policy and a
// graceful-shutdown SIGTERM path. We keep mpvpaper as a subprocess because
// it's the upstream-accepted reference for libmpv-into-wlr-layer-shell;
// reimplementing that in-process would be a big and fragile code investment.

use anyhow::{Context, Result};
use nix::{
    sys::signal::{kill, Signal},
    unistd::Pid,
};
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use crate::{
    config::{Config, SpanDirection},
    media::MediaKind,
};

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
    /// mpvpaper handles video OR image. `crop` = None for non-span outputs.
    Mpv {
        output: String,
        path: PathBuf,
        crop: Option<String>,
        media: MediaKind,
        audio: bool,
        fit: String,
        extra: Vec<String>,
    },
    /// swaybg for a static image on a single output.
    Swaybg {
        output: String,
        path: PathBuf,
        mode: String,
    },
}

impl Worker {
    pub fn spawn(kind: WorkerKind) -> Result<Self> {
        let (label, child) = match &kind {
            WorkerKind::Mpv { output, path, crop, media, audio, fit, extra } => (
                format!("mpv:{output}"),
                spawn_mpv(output, path, crop.as_deref(), *media, *audio, fit, extra)?,
            ),
            WorkerKind::Swaybg { output, path, mode } => (
                format!("swaybg:{output}"),
                spawn_swaybg(output, path, mode)?,
            ),
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
    /// Returns `Ok(false)` if the worker is still alive.
    pub fn poll_and_maybe_restart(&mut self) -> Result<bool> {
        match self.child.try_wait().context("waitpid worker")? {
            None => Ok(false),
            Some(status) => {
                let uptime = self.started_at.elapsed();
                tracing::warn!(
                    "{}: exited {:?} after {:.1}s",
                    self.label, status, uptime.as_secs_f32()
                );

                // Window the failure counter so long-lived crashes reset it.
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

/// Compute the mpv `vf=crop=` expression for slice `index` of `total` along
/// the span direction. Uses `iw`/`ih` so it works regardless of source size.
pub fn crop_spec(index: usize, total: usize, dir: SpanDirection) -> String {
    let total = total.max(1) as i32;
    let i = index as i32;
    match dir {
        SpanDirection::Vertical =>
            format!("crop=iw:ih/{t}:0:ih*{i}/{t}", t = total, i = i),
        SpanDirection::Horizontal =>
            format!("crop=iw/{t}:ih:iw*{i}/{t}:0", t = total, i = i),
    }
}

/// Plan workers for the current config without spawning them yet. The
/// content type of each slot is detected here and recorded in the plan, so
/// the spawn step doesn't need to re-probe disk.
pub fn plan(cfg: &Config) -> Result<Vec<WorkerKind>> {
    let mut plan = Vec::new();

    let span_path = cfg.span.as_ref().context("config.span unset")?;
    let span_media = MediaKind::detect(span_path)
        .with_context(|| format!("classify span content: {}", span_path.display()))?;
    tracing::info!(
        "span content: {} ({:?})",
        span_path.display(), span_media
    );

    let total = cfg.span_outputs.len();
    for (i, out) in cfg.span_outputs.iter().enumerate() {
        plan.push(WorkerKind::Mpv {
            output: out.clone(),
            path: span_path.clone(),
            crop: Some(crop_spec(i, total, cfg.span_direction)),
            media: span_media,
            // Audio only meaningful for video, and only on the first instance.
            audio: cfg.audio && i == 0 && span_media == MediaKind::Video,
            fit: cfg.span_fit.clone(),
            extra: cfg.extra_mpv_options.clone(),
        });
    }

    if let (Some(out), Some(path)) = (&cfg.side_output, &cfg.side) {
        let side_media = MediaKind::detect(path)
            .with_context(|| format!("classify side content: {}", path.display()))?;
        tracing::info!(
            "side content: {} ({:?})",
            path.display(), side_media
        );
        match side_media {
            MediaKind::Image => {
                plan.push(WorkerKind::Swaybg {
                    output: out.clone(),
                    path: path.clone(),
                    mode: cfg.side_mode.clone(),
                });
            }
            MediaKind::Video => {
                plan.push(WorkerKind::Mpv {
                    output: out.clone(),
                    path: path.clone(),
                    crop: None,
                    media: MediaKind::Video,
                    audio: false,
                    fit: cfg.span_fit.clone(),
                    extra: cfg.extra_mpv_options.clone(),
                });
            }
        }
    }

    Ok(plan)
}

fn spawn_mpv(
    output: &str,
    path: &Path,
    crop: Option<&str>,
    media: MediaKind,
    audio: bool,
    fit: &str,
    extra: &[String],
) -> Result<Child> {
    let bin = which::which("mpvpaper")
        .context("`mpvpaper` not found on PATH (install: pacman -S mpvpaper)")?;

    let mut opts: Vec<String> = vec![
        "loop-file=inf".into(),
        "hwdec=auto-safe".into(),
    ];
    if let Some(c) = crop {
        opts.push(format!("vf={c}"));
    }
    if !audio {
        opts.push("no-audio".into());
        opts.push("mute=yes".into());
    } else {
        opts.push("volume=100".into());
    }
    if media == MediaKind::Image {
        // Hold the single frame forever — mpv's default is 1s and then
        // playback ends.
        opts.push("image-display-duration=inf".into());
        opts.push("loop=inf".into());
    }
    match fit {
        "stretch" => opts.push("keepaspect=no".into()),
        "fit"     => { /* default mpv behavior; letterboxes */ }
        _         => {
            opts.push("panscan=1.0".into());
            opts.push("keepaspect=yes".into());
        }
    }
    opts.push("really-quiet=yes".into());
    opts.push("force-window=no".into());

    for e in extra {
        opts.push(e.clone());
    }

    let opt_str = opts.join(" ");

    let mut cmd = Command::new(bin);
    cmd.arg("-o").arg(&opt_str)
       .arg(output)
       .arg(path)
       .stdin(Stdio::null())
       .stdout(Stdio::null())
       .stderr(Stdio::inherit());

    tracing::info!(
        "spawning mpvpaper worker: -o {:?} {} {} ({:?})",
        opt_str, output, path.display(), media
    );
    cmd.spawn().context("spawn mpvpaper")
}

fn spawn_swaybg(output: &str, image: &Path, mode: &str) -> Result<Child> {
    let bin = which::which("swaybg")
        .context("`swaybg` not found on PATH (install: pacman -S swaybg)")?;

    let mut cmd = Command::new(bin);
    cmd.arg("-o").arg(output)
       .arg("-i").arg(image)
       .arg("-m").arg(mode)
       .stdin(Stdio::null())
       .stdout(Stdio::null())
       .stderr(Stdio::inherit());

    tracing::info!("spawning swaybg worker: -o {} -i {} -m {}",
                   output, image.display(), mode);
    cmd.spawn().context("spawn swaybg")
}
