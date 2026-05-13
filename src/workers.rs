// Subprocess supervisors.
//
// Worker dispatch:
//
//   span outputs (always mpvpaper):
//     We build a single `vf` filter chain that first scales/fits the source
//     to the COMBINED virtual canvas (sum of span-output dimensions), then
//     crops to *this monitor's* slice of that canvas. Doing the fit on the
//     combined canvas — not per-monitor after slicing — is what prevents
//     content near the seam from disappearing on aspect-mismatched sources.
//     For images we additionally tell mpv to hold the single frame.
//
//   side output:
//     · Image → swaybg (lighter than libmpv for a still)
//     · Video → mpvpaper, no vf; mpv's panscan/keepaspect handle the fit
//
// Each worker is wrapped in a `Worker` with restart-on-crash backoff and a
// graceful SIGTERM shutdown path.

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
    outputs::Output,
};

pub struct Worker {
    pub label: String,
    pub kind: WorkerKind,
    pub child: Child,
    started_at: Instant,
    recent_failures: u32,
}

#[derive(Debug, Clone)]
pub enum WorkerKind {
    /// mpvpaper for video OR image. `vf = Some(...)` for span outputs (the
    /// vf chain already produces output sized exactly for the monitor);
    /// `vf = None` for a solo video on the side output (mpv fits via
    /// panscan/keepaspect per `fit`).
    Mpv {
        output: String,
        path: PathBuf,
        vf: Option<String>,
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
            WorkerKind::Mpv { output, path, vf, media, audio, fit, extra } => (
                format!("mpv:{output}"),
                spawn_mpv(output, path, vf.as_deref(), *media, *audio, fit, extra)?,
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

    pub fn poll_and_maybe_restart(&mut self) -> Result<bool> {
        match self.child.try_wait().context("waitpid worker")? {
            None => Ok(false),
            Some(status) => {
                let uptime = self.started_at.elapsed();
                tracing::warn!(
                    "{}: exited {:?} after {:.1}s",
                    self.label, status, uptime.as_secs_f32()
                );

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

/// Build the per-monitor `vf` chain for a span output.
///
/// `(vw,vh)`: combined virtual canvas dimensions
/// `(w,h)`:   this monitor's dimensions
/// `(x,y)`:   this monitor's offset within the virtual canvas
/// `fit`:     `crop` (cover, default) | `fit` (contain/letterbox) | `stretch`
fn build_span_vf(vw: i32, vh: i32, w: i32, h: i32, x: i32, y: i32, fit: &str) -> String {
    let canvas = match fit {
        "stretch" => format!("scale={vw}:{vh}"),
        "fit"     => format!(
            "scale={vw}:{vh}:force_original_aspect_ratio=decrease,\
             pad={vw}:{vh}:(ow-iw)/2:(oh-ih)/2:color=black"
        ),
        // "crop" / default = cover. Scale until both dims >= canvas, then
        // center-crop the overflow off the *outer* edges of the canvas.
        // Content near the seam is preserved.
        _ => format!(
            "scale={vw}:{vh}:force_original_aspect_ratio=increase,\
             crop={vw}:{vh}"
        ),
    };
    format!("{canvas},crop={w}:{h}:{x}:{y}")
}

/// Compute the combined virtual canvas size and each output's offset within it.
fn compute_canvas(group: &[&Output], dir: SpanDirection) -> ((i32, i32), Vec<(i32, i32)>) {
    let mut offsets = Vec::with_capacity(group.len());
    let (mut acc_x, mut acc_y) = (0_i32, 0_i32);
    for o in group {
        offsets.push((acc_x, acc_y));
        match dir {
            SpanDirection::Vertical   => acc_y += o.height,
            SpanDirection::Horizontal => acc_x += o.width,
        }
    }
    let canvas = match dir {
        // For uniform spans we use any element's perpendicular dim; if the
        // user has mismatched output widths in a vertical span the seam
        // can't be uniform either way, so picking the first is reasonable.
        SpanDirection::Vertical   => (group[0].width, acc_y),
        SpanDirection::Horizontal => (acc_x, group[0].height),
    };
    (canvas, offsets)
}

/// Plan workers for the current config, given the currently-detected outputs.
pub fn plan(cfg: &Config, detected: &[Output]) -> Result<Vec<WorkerKind>> {
    let mut plan = Vec::new();

    let span_path = cfg.span.as_ref().context("config.span unset")?;
    let span_media = MediaKind::detect(span_path)
        .with_context(|| format!("classify span content: {}", span_path.display()))?;
    tracing::info!("span content: {} ({:?})", span_path.display(), span_media);

    // Resolve span output names → detected outputs. If any are missing we
    // skip the whole span group rather than render misaligned slices.
    let span_group: Option<Vec<&Output>> = cfg
        .span_outputs
        .iter()
        .map(|name| detected.iter().find(|o| &o.name == name))
        .collect();

    match span_group {
        Some(group) if !group.is_empty() => {
            let ((vw, vh), offsets) = compute_canvas(&group, cfg.span_direction);
            tracing::info!(
                "span canvas: {vw}x{vh} ({} outputs, direction={:?})",
                group.len(), cfg.span_direction
            );
            for (i, out) in group.iter().enumerate() {
                let (x, y) = offsets[i];
                let vf = build_span_vf(vw, vh, out.width, out.height, x, y, &cfg.span_fit);
                plan.push(WorkerKind::Mpv {
                    output: out.name.clone(),
                    path: span_path.clone(),
                    vf: Some(vf),
                    media: span_media,
                    audio: cfg.audio && i == 0 && span_media == MediaKind::Video,
                    fit: cfg.span_fit.clone(),
                    extra: cfg.extra_mpv_options.clone(),
                });
            }
        }
        _ => {
            tracing::error!(
                "skipping span: one or more configured outputs not detected ({:?})",
                cfg.span_outputs
            );
        }
    }

    if let (Some(out), Some(path)) = (&cfg.side_output, &cfg.side) {
        let side_media = MediaKind::detect(path)
            .with_context(|| format!("classify side content: {}", path.display()))?;
        tracing::info!("side content: {} ({:?})", path.display(), side_media);
        match side_media {
            MediaKind::Image => plan.push(WorkerKind::Swaybg {
                output: out.clone(),
                path: path.clone(),
                mode: cfg.side_mode.clone(),
            }),
            MediaKind::Video => plan.push(WorkerKind::Mpv {
                output: out.clone(),
                path: path.clone(),
                vf: None,
                media: MediaKind::Video,
                audio: false,
                fit: cfg.span_fit.clone(),
                extra: cfg.extra_mpv_options.clone(),
            }),
        }
    }

    if plan.is_empty() {
        anyhow::bail!("no workers planned — check config and detected outputs");
    }
    Ok(plan)
}

fn spawn_mpv(
    output: &str,
    path: &Path,
    vf: Option<&str>,
    media: MediaKind,
    audio: bool,
    fit: &str,
    extra: &[String],
) -> Result<Child> {
    let bin = which::which("mpvpaper")
        .context("`mpvpaper` not found on PATH (install: pacman -S mpvpaper)")?;

    let mut opts: Vec<String> = vec!["loop-file=inf".into()];
    if vf.is_some() {
        // Software filters (scale/crop) need CPU-resident frames; pure
        // `hwdec=auto-safe` keeps frames in CUDA/VAAPI memory and our
        // libavfilter chain then silently fails (`crop: Failed to configure
        // input pad on filter`) because it doesn't speak hwframes. Use the
        // copy-back variant so the decoder still runs on the GPU but frames
        // are downloaded to RAM before the filter graph.
        opts.push("hwdec=auto-copy-safe".into());
    } else {
        opts.push("hwdec=auto-safe".into());
    }
    if let Some(v) = vf {
        opts.push(format!("vf={v}"));
        // Our vf chain produces output sized exactly to the monitor. mpv's
        // default keepaspect=yes preserves the *source* aspect (e.g. 928:1376
        // for a portrait source), which would pillarbox the chain's
        // already-correct 1920x1080 output back inside the source aspect.
        // keepaspect=no makes mpv display the post-filter frame 1:1 to the
        // VO surface — no second-guessing, no bars.
        opts.push("keepaspect=no".into());
    } else {
        // Solo (side video): mpv handles fit.
        match fit {
            "stretch" => opts.push("keepaspect=no".into()),
            "fit"     => { /* default mpv letterbox */ }
            _         => {
                opts.push("panscan=1.0".into());
                opts.push("keepaspect=yes".into());
            }
        }
    }
    if !audio {
        opts.push("no-audio".into());
        opts.push("mute=yes".into());
    } else {
        opts.push("volume=100".into());
    }
    if media == MediaKind::Image {
        opts.push("image-display-duration=inf".into());
        opts.push("loop=inf".into());
    }
    opts.push("really-quiet=yes".into());
    opts.push("force-window=no".into());

    for e in extra { opts.push(e.clone()); }

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
