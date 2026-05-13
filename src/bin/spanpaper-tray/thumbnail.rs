//! On-demand thumbnail cache for span/side source files.
//!
//! Strategy:
//!   * One cache file per source-file absolute path, keyed by a
//!     DefaultHasher of the path bytes — collision-resistance isn't
//!     required, only stability across the same process. Cache lives
//!     at `$XDG_CACHE_HOME/spanpaper/thumbs/<hash>.png`.
//!   * Cache entry is treated as stale when the source's mtime is
//!     newer than the cache's mtime — covers the "user edited the
//!     file in place" case.
//!   * A single `ffmpeg -frames:v 1 -vf scale=256:-1` invocation
//!     handles both stills (any common image format) and videos. No
//!     pre-roll seek — `-ss 0.5` was tempting for video fade-ins, but
//!     a still has a 1-frame stream of duration 1/25 s, so the seek
//!     pushes past EOF and ffmpeg exits zero with no file written.
//!     The skipped-fade-in benefit isn't worth the still-image
//!     breakage. `-update 1` tells the image2 muxer this is a single
//!     image, not a sequence — suppresses an otherwise-spurious
//!     pattern warning.
//!   * If ffmpeg is unavailable or fails for a particular file, we
//!     return Err and the caller falls back to text-only rendering
//!     — the tray must never block the popover on thumbnail trouble.

use anyhow::{Context, Result};
use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    process::Command,
    time::SystemTime,
};

const THUMB_WIDTH: u32 = 256;

fn cache_dir() -> Result<PathBuf> {
    let base = dirs::cache_dir().context("XDG cache dir not set")?;
    let dir = base.join("spanpaper").join("thumbs");
    fs::create_dir_all(&dir).context("create thumbnail cache dir")?;
    Ok(dir)
}

fn cache_path_for(source: &Path) -> Result<PathBuf> {
    // Canonicalise so two different relative spellings of the same
    // file share a cache entry. Fall back to as-given when canonicalize
    // fails (e.g. file vanished) — the lookup will then miss and we'll
    // attempt generation, which will produce a clearer error.
    let abs = source
        .canonicalize()
        .unwrap_or_else(|_| source.to_path_buf());
    let mut h = DefaultHasher::new();
    abs.hash(&mut h);
    let key = format!("{:016x}.png", h.finish());
    Ok(cache_dir()?.join(key))
}

fn mtime(p: &Path) -> Option<SystemTime> {
    fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Return the cache path of a 256-wide PNG thumbnail for `source`.
/// Regenerates on demand when the cache is missing or stale.
pub fn ensure(source: &Path) -> Result<PathBuf> {
    let cache = cache_path_for(source)?;
    if let (Some(c_mtime), Some(s_mtime)) = (mtime(&cache), mtime(source)) {
        if c_mtime >= s_mtime {
            return Ok(cache);
        }
    }
    generate(source, &cache)?;
    Ok(cache)
}

fn generate(source: &Path, dest: &Path) -> Result<()> {
    let ffmpeg = which::which("ffmpeg")
        .context("`ffmpeg` not on PATH (install: pacman -S ffmpeg)")?;

    // Write to a temp sibling, then rename, so a partial file from a
    // killed ffmpeg never leaks into the cache.
    let tmp = dest.with_extension("png.tmp");
    let _ = fs::remove_file(&tmp);

    let status = Command::new(&ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel", "error",
            "-y",
            "-i",
        ])
        .arg(source)
        .args([
            "-frames:v", "1",
            "-vf", &format!("scale={THUMB_WIDTH}:-1:flags=lanczos"),
            // Force PNG output to match the cache file's extension —
            // ffmpeg's image2 muxer defaults to mjpeg, which works for
            // GdkPixbuf (it sniffs) but leaves us with .png files that
            // are actually JPEG. Honest naming wins.
            "-c:v", "png",
            // Treat this as a single image, not a numbered sequence —
            // otherwise the image2 muxer prints a "filename does not
            // contain an image sequence pattern" warning on every call.
            "-update", "1",
            "-f", "image2",
        ])
        .arg(&tmp)
        .status()
        .context("invoke ffmpeg")?;
    if !status.success() {
        let _ = fs::remove_file(&tmp);
        anyhow::bail!("ffmpeg exited {status} on {}", source.display());
    }
    fs::rename(&tmp, dest).context("rename thumbnail into cache")?;
    Ok(())
}
