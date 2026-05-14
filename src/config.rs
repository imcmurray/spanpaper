// Persistent config at $XDG_CONFIG_HOME/spanpaper/config.toml
// (typically ~/.config/spanpaper/config.toml).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::media::MediaKind;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Content for the span outputs. Image or video — auto-detected.
    #[serde(default)]
    pub span: Option<PathBuf>,

    /// Content for the side output (DP-5 by default). Image or video.
    #[serde(default)]
    pub side: Option<PathBuf>,

    /// Unmute the video. Only meaningful when `span` is a video. Default: muted.
    #[serde(default)]
    pub audio: bool,

    /// Outputs that share the span content, ordered top → bottom (or left → right).
    /// Default = your stacked rig: HDMI-A-4 on top, DP-6 on bottom.
    #[serde(default = "default_span_outputs")]
    pub span_outputs: Vec<String>,

    /// Output that gets the side content.
    #[serde(default = "default_side_output")]
    pub side_output: Option<String>,

    /// Direction of the span. "vertical" = top/bottom (default);
    /// "horizontal" = left/right.
    #[serde(default = "default_span_direction")]
    pub span_direction: SpanDirection,

    /// Extra raw mpv options appended to every video worker. Power-user knob.
    #[serde(default)]
    pub extra_mpv_options: Vec<String>,

    /// How aggressively to fit the source onto the combined span canvas
    /// before per-monitor slicing.
    ///   `crop`    = scale-to-cover + center-crop (default, fills both monitors)
    ///   `fit`     = scale-to-contain + letterbox/pillarbox
    ///   `stretch` = ignore aspect, stretch to canvas dimensions
    #[serde(default = "default_span_fit")]
    pub span_fit: String,

    /// Fit mode for the side content — independent of `span_fit`. Same
    /// three values (`crop`/`fit`/`stretch`) apply to both side images
    /// (mapped to swaybg's fill/fit/stretch) and side videos (mpv
    /// panscan/keepaspect, same translation as for span).
    #[serde(default = "default_side_fit")]
    pub side_fit: String,

    /// Saved presets, ordered by insertion (newest at the end). Cycle
    /// order = this Vec's order. Default empty.
    #[serde(default)]
    pub presets: Vec<Preset>,

    /// Name of the preset whose values currently match the active
    /// config. Set by `spanpaper preset load NAME` / `save NAME`,
    /// cleared (strict mode) by any `spanpaper set --…` that mutates
    /// preset-relevant fields. `None` is normal — it just means the
    /// active config doesn't correspond to any saved preset.
    #[serde(default)]
    pub active_preset: Option<String>,
}

/// A named snapshot of preset-relevant Config fields. Hardware
/// identity (`span_outputs`, `side_output`) is deliberately NOT
/// included — a preset recorded on Rig A shouldn't dictate output
/// names on Rig B.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    #[serde(default)]
    pub span: Option<PathBuf>,
    #[serde(default)]
    pub side: Option<PathBuf>,
    #[serde(default)]
    pub audio: bool,
    #[serde(default = "default_span_fit")]
    pub span_fit: String,
    #[serde(default = "default_side_fit")]
    pub side_fit: String,
    #[serde(default = "default_span_direction")]
    pub span_direction: SpanDirection,
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
fn default_side_output() -> Option<String> { Some("DP-5".into()) }
fn default_span_direction() -> SpanDirection { SpanDirection::Vertical }
fn default_span_fit() -> String { "crop".into() }
fn default_side_fit() -> String { "crop".into() }

impl Default for Config {
    fn default() -> Self {
        Self {
            span: None,
            side: None,
            audio: false,
            span_outputs: default_span_outputs(),
            side_output: default_side_output(),
            span_direction: default_span_direction(),
            extra_mpv_options: vec![],
            span_fit: default_span_fit(),
            side_fit: default_side_fit(),
            presets: vec![],
            active_preset: None,
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
        let span = self.span.as_deref().context(
            "config.span is unset; run `spanpaper set --span PATH` (any image or video)",
        )?;
        ensure_file(span, "span")?;
        MediaKind::detect(span).with_context(|| {
            format!("could not classify span content: {}", span.display())
        })?;

        if let Some(side) = &self.side {
            ensure_file(side, "side")?;
            MediaKind::detect(side).with_context(|| {
                format!("could not classify side content: {}", side.display())
            })?;
        }

        if self.span_outputs.is_empty() {
            anyhow::bail!("config.span_outputs is empty");
        }
        Ok(())
    }

    /// Snapshot the active preset-relevant fields into a `Preset`.
    /// Doesn't touch the config; callers wire the result into
    /// `self.presets` themselves.
    pub fn snapshot_as_preset(&self, name: String) -> Preset {
        Preset {
            name,
            span: self.span.clone(),
            side: self.side.clone(),
            audio: self.audio,
            span_fit: self.span_fit.clone(),
            side_fit: self.side_fit.clone(),
            span_direction: self.span_direction,
        }
    }

    /// Copy a preset's fields onto the active config and mark it as
    /// active. Returns Err if no preset with the given name exists.
    pub fn apply_preset(&mut self, name: &str) -> Result<()> {
        let preset = self.presets.iter().find(|p| p.name == name).cloned()
            .with_context(|| format!("no preset named {name:?}"))?;
        self.span = preset.span;
        self.side = preset.side;
        self.audio = preset.audio;
        self.span_fit = preset.span_fit;
        self.side_fit = preset.side_fit;
        self.span_direction = preset.span_direction;
        self.active_preset = Some(preset.name);
        Ok(())
    }

    /// Find the index of the named preset in the cycle order
    /// (`presets` Vec is the source of truth for next/prev).
    pub fn preset_index(&self, name: &str) -> Option<usize> {
        self.presets.iter().position(|p| p.name == name)
    }
}

/// Sanity-check a preset name. Names appear in CLI args, in the tray
/// menu, and in the TOML config — they must be safe in all three.
pub fn validate_preset_name(name: &str) -> Result<()> {
    if name.is_empty() {
        anyhow::bail!("preset name is empty");
    }
    if name.starts_with('.') {
        anyhow::bail!("preset name must not start with '.': {name:?}");
    }
    for c in name.chars() {
        if c.is_control() || c == '/' || c == '\\' || c == '\n' {
            anyhow::bail!(
                "preset name contains forbidden character {c:?} in {name:?}"
            );
        }
    }
    Ok(())
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
