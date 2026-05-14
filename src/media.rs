// Content type detection for spanpaper inputs.
//
// We accept both images and videos in either slot:
//   * span outputs (HDMI-A-4 + DP-6): always mpvpaper with a per-monitor
//     crop filter — for an image we hold the single frame indefinitely via
//     `image-display-duration=inf` so playback is effectively a still.
//   * side output (DP-5): swaybg for an image (lighter than libmpv for a
//     still); mpvpaper for a video (no crop).
//
// Detection is extension-first for speed; falls back to file(1)'s MIME
// probe so paths without an extension still work.

use anyhow::{anyhow, Context, Result};
use std::{path::Path, process::Command};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Image,
    Video,
}

impl MediaKind {
    pub fn detect(path: &Path) -> Result<Self> {
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            let ext = ext.to_ascii_lowercase();
            if IMAGE_EXTS.iter().any(|x| *x == ext) {
                return Ok(MediaKind::Image);
            }
            if VIDEO_EXTS.iter().any(|x| *x == ext) {
                return Ok(MediaKind::Video);
            }
        }
        Self::probe_mime(path)
    }

    fn probe_mime(path: &Path) -> Result<Self> {
        let out = Command::new("file")
            .args(["--brief", "--mime-type"])
            .arg(path)
            .output()
            .context(
                "invoke `file` for MIME detection (install `file` to support unknown extensions)",
            )?;
        let mime = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if mime.starts_with("image/") {
            return Ok(MediaKind::Image);
        }
        if mime.starts_with("video/") {
            return Ok(MediaKind::Video);
        }
        Err(anyhow!(
            "unrecognised media type ({mime}) for {}",
            path.display()
        ))
    }
}

const IMAGE_EXTS: &[&str] = &[
    "jpg", "jpeg", "png", "webp", "bmp", "gif", "tiff", "tif", "avif", "heic", "heif", "jxl",
    "qoi", "pbm", "pgm", "ppm",
];

const VIDEO_EXTS: &[&str] = &[
    "mp4", "m4v", "mkv", "webm", "mov", "avi", "wmv", "flv", "ts", "mpg", "mpeg", "ogv", "ogm",
    "3gp", "3g2",
];
