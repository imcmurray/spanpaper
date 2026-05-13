// Persistent config at $XDG_CONFIG_HOME/spanpaper/config.toml
// (typically ~/.config/spanpaper/config.toml).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Single source MP4 to span across `span_outputs`, top → bottom.
    #[serde(default)]
    pub video: Option<PathBuf>,

    /// Static image displayed on `image_output`.
    #[serde(default)]
    pub left_image: Option<PathBuf>,

    /// Unmute the video. Default: muted.
    #[serde(default)]
    pub audio: bool,

    /// Outputs that share the video, ordered top → bottom (or left → right).
    /// Default = your stacked rig: HDMI-A-4 on top, DP-6 on bottom.
    #[serde(default = "default_span_outputs")]
    pub span_outputs: Vec<String>,

    /// Output that gets the static image.
    #[serde(default = "default_image_output")]
    pub image_output: Option<String>,

    /// Image fit mode passed to swaybg (fill|fit|stretch|center|tile).
    #[serde(default = "default_image_mode")]
    pub image_mode: String,

    /// Direction of the span. "vertical" = top/bottom (your setup);
    /// "horizontal" = left/right.
    #[serde(default = "default_span_direction")]
    pub span_direction: SpanDirection,

    /// Extra raw mpv options appended to every video worker. Power-user knob.
    #[serde(default)]
    pub extra_mpv_options: Vec<String>,

    /// How aggressively to fit the (already-cropped) slice to its monitor.
    /// `crop`   = panscan=1.0, zoom-fill, may clip sides (recommended)
    /// `fit`    = letterbox, may show black bars
    /// `stretch`= ignore aspect (--keepaspect=no)
    #[serde(default = "default_video_fit")]
    pub video_fit: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SpanDirection {
    Vertical,
    Horizontal,
}

fn default_span_outputs() -> Vec<String> {
    vec!["HDMI-A-4".into(), "DP-6".into()]
}
fn default_image_output() -> Option<String> { Some("DP-5".into()) }
fn default_image_mode() -> String { "fill".into() }
fn default_span_direction() -> SpanDirection { SpanDirection::Vertical }
fn default_video_fit() -> String { "crop".into() }

impl Default for Config {
    fn default() -> Self {
        Self {
            video: None,
            left_image: None,
            audio: false,
            span_outputs: default_span_outputs(),
            image_output: default_image_output(),
            image_mode: default_image_mode(),
            span_direction: default_span_direction(),
            extra_mpv_options: vec![],
            video_fit: default_video_fit(),
        }
    }
}

impl Config {
    pub fn path() -> Result<PathBuf> {
        let base = dirs::config_dir().context("no XDG config dir")?;
        Ok(base.join("spanpaper").join("config.toml"))
    }

    pub fn load() -> Result<Self> {
        let p = Self::path()?;
        let text = fs::read_to_string(&p)
            .with_context(|| format!("read {}", p.display()))?;
        let cfg: Config = toml::from_str(&text)
            .with_context(|| format!("parse {}", p.display()))?;
        Ok(cfg)
    }

    pub fn load_or_default() -> Result<Self> {
        match Self::load() {
            Ok(c) => Ok(c),
            Err(_) => Ok(Self::default()),
        }
    }

    pub fn save(&self) -> Result<()> {
        let p = Self::path()?;
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("mkdir {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(self).context("serialize toml")?;
        // Atomic write: tmp + rename, so a crash mid-write never leaves a
        // partial config that breaks subsequent loads.
        let tmp = p.with_extension("toml.tmp");
        fs::write(&tmp, text).with_context(|| format!("write {}", tmp.display()))?;
        fs::rename(&tmp, &p).with_context(|| format!("rename to {}", p.display()))?;
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        let v = self.video.as_deref().context("config.video is unset; run `spanpaper set --video PATH`")?;
        ensure_file(v, "video")?;
        if let Some(img) = &self.left_image {
            ensure_file(img, "left_image")?;
        }
        if self.span_outputs.is_empty() {
            anyhow::bail!("config.span_outputs is empty");
        }
        Ok(())
    }
}

fn ensure_file(p: &Path, label: &str) -> Result<()> {
    if !p.exists() {
        anyhow::bail!("{label} path does not exist: {}", p.display());
    }
    if !p.is_file() {
        anyhow::bail!("{label} path is not a regular file: {}", p.display());
    }
    Ok(())
}
