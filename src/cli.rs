// CLI surface: clap definitions and subcommand dispatch.
//
// Subcommands:
//   set     — update config (paths, audio, span/image outputs) and reload daemon
//   start   — run as daemon (foreground; use --background to fork)
//   stop    — terminate running daemon
//   restart — stop + start
//   status  — show daemon state, config, detected outputs
//   outputs — print detected Wayland outputs and exit

use crate::{config::Config, daemon, outputs};
use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "spanpaper",
    version,
    about = "Single-MP4 video wallpaper spanning stacked Wayland monitors",
    long_about = None,
)]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Update configuration and reload the daemon if running.
    Set(SetArgs),
    /// Run the spanpaper daemon (foreground).
    Start(StartArgs),
    /// Stop the running spanpaper daemon.
    Stop,
    /// Restart the daemon (stop + start in background).
    Restart(StartArgs),
    /// Show daemon status, config, and detected outputs.
    Status,
    /// Print the Wayland outputs spanpaper sees and exit.
    Outputs,
}

#[derive(Args, Debug)]
pub struct SetArgs {
    /// Path to the source MP4 to span across the vertical-stacked monitors.
    #[arg(long, value_name = "PATH")]
    pub video: Option<PathBuf>,

    /// Path to the static image for the side monitor (DP-5 by default).
    #[arg(long, value_name = "PATH")]
    pub left_image: Option<PathBuf>,

    /// Unmute video audio. Defaults to muted.
    #[arg(long)]
    pub audio: bool,

    /// Mute video audio (explicit, overrides --audio).
    #[arg(long, conflicts_with = "audio")]
    pub no_audio: bool,

    /// Comma-separated output names to span the video over, top→bottom.
    /// Example: --span-outputs HDMI-A-4,DP-6
    #[arg(long, value_name = "NAMES", value_delimiter = ',')]
    pub span_outputs: Option<Vec<String>>,

    /// Output name to display the static image on.
    #[arg(long, value_name = "NAME")]
    pub image_output: Option<String>,

    /// Fit mode for the static image: fill | fit | stretch | center | tile.
    #[arg(long, value_name = "MODE")]
    pub image_mode: Option<String>,

    /// Don't reload the running daemon after writing config.
    #[arg(long)]
    pub no_reload: bool,
}

#[derive(Args, Debug, Clone)]
pub struct StartArgs {
    /// Fork into the background after startup.
    #[arg(long)]
    pub background: bool,
}

pub fn dispatch(cli: Cli) -> Result<()> {
    match cli.cmd {
        Cmd::Set(a) => cmd_set(a),
        Cmd::Start(a) => daemon::run(a.background),
        Cmd::Stop => daemon::stop(),
        Cmd::Restart(a) => {
            let _ = daemon::stop(); // ignore "not running"
            daemon::spawn_background()?;
            if !a.background {
                tracing::info!("daemon restarted in background");
            }
            Ok(())
        }
        Cmd::Status => cmd_status(),
        Cmd::Outputs => cmd_outputs(),
    }
}

fn cmd_set(a: SetArgs) -> Result<()> {
    let mut cfg = Config::load_or_default().context("loading config")?;

    if let Some(v) = a.video {
        cfg.video = Some(canonicalize_user_path(&v)?);
    }
    if let Some(i) = a.left_image {
        cfg.left_image = Some(canonicalize_user_path(&i)?);
    }
    if a.audio {
        cfg.audio = true;
    }
    if a.no_audio {
        cfg.audio = false;
    }
    if let Some(s) = a.span_outputs {
        cfg.span_outputs = s;
    }
    if let Some(o) = a.image_output {
        cfg.image_output = Some(o);
    }
    if let Some(m) = a.image_mode {
        cfg.image_mode = m;
    }

    cfg.save().context("saving config")?;
    tracing::info!("config saved to {}", Config::path()?.display());

    if !a.no_reload {
        match daemon::reload() {
            Ok(()) => tracing::info!("running daemon reloaded"),
            Err(e) if e.to_string().contains("not running") => {
                tracing::info!("daemon not running (use `spanpaper start` to launch)")
            }
            Err(e) => tracing::warn!("reload failed: {e:#}"),
        }
    }
    Ok(())
}

fn cmd_status() -> Result<()> {
    let pid = daemon::current_pid().ok();
    println!("daemon: {}", match pid {
        Some(p) => format!("running (pid {p})"),
        None    => "not running".into(),
    });

    match Config::load() {
        Ok(cfg) => {
            println!("config: {}", Config::path()?.display());
            println!("  video        = {:?}", cfg.video);
            println!("  left_image   = {:?}", cfg.left_image);
            println!("  audio        = {}", cfg.audio);
            println!("  span_outputs = {:?}", cfg.span_outputs);
            println!("  image_output = {:?}", cfg.image_output);
            println!("  image_mode   = {}", cfg.image_mode);
        }
        Err(e) => println!("config: <missing or invalid> ({e})"),
    }

    match outputs::detect() {
        Ok(list) => {
            println!("outputs:");
            for o in list {
                println!(
                    "  {:<10} {}x{} @ ({},{})  scale={}",
                    o.name, o.width, o.height, o.x, o.y, o.scale
                );
            }
        }
        Err(e) => println!("outputs: <error: {e:#}>"),
    }
    Ok(())
}

fn cmd_outputs() -> Result<()> {
    let list = outputs::detect()?;
    for o in list {
        println!(
            "{}\t{}x{}\t+{}+{}\tscale={}",
            o.name, o.width, o.height, o.x, o.y, o.scale
        );
    }
    Ok(())
}

/// Expand ~ / env vars and canonicalize to absolute. We don't require the file
/// to exist (config may be written before files are placed), so we fall back to
/// the absolute form if canonicalize() fails.
fn canonicalize_user_path(p: &std::path::Path) -> Result<PathBuf> {
    let expanded = shellexpand::full(&p.to_string_lossy())
        .map_err(|e| anyhow::anyhow!("path expansion: {e}"))?
        .into_owned();
    let pb = PathBuf::from(expanded);
    Ok(pb.canonicalize().unwrap_or(pb))
}
