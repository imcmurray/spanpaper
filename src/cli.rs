// CLI surface: clap definitions and subcommand dispatch.

use spanpaper::{config::Config, outputs};
use crate::daemon;
use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "spanpaper",
    version,
    about = "Single-MP4 (or image) wallpaper spanning stacked Wayland monitors",
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
    /// Wire user-level autostart: writes ~/.config/autostart/spanpaper.desktop
    /// (always) and ~/.config/autostart/spanpaper-tray.desktop (when the tray
    /// binary lives alongside the daemon). Idempotent.
    Install(InstallArgs),
    /// Manage saved presets — named snapshots of span / side / fits /
    /// audio you can switch between or cycle through.
    #[command(subcommand)]
    Preset(PresetCmd),
}

#[derive(Subcommand, Debug)]
pub enum PresetCmd {
    /// Snapshot the active config into a named preset (overwrites if
    /// it already exists). Sets `active_preset = NAME`.
    Save { name: String },
    /// Print all saved preset names; the active one is marked with "*".
    List,
    /// Copy a preset's fields into the active config and SIGHUP daemon.
    /// Sets `active_preset = NAME`.
    Load { name: String },
    /// Remove a preset from the list. Clears `active_preset` if it matched.
    Delete { name: String },
    /// Rename a preset in place. Updates `active_preset` if it matched.
    Rename { old: String, new: String },
    /// Advance to the next preset in insertion order; wraps from the
    /// last back to the first. If nothing is currently active (or the
    /// active preset no longer exists), loads index 0.
    Next,
    /// Like `next` but in reverse.
    Prev,
}

#[derive(Args, Debug)]
pub struct InstallArgs {
    /// Where to install autostart for the DAEMON. Values:
    ///   xdg     — `~/.config/autostart/spanpaper.desktop`.
    ///             Works on every XDG-compliant session, including
    ///             Budgie. This is the safe default.
    ///   systemd — `~/.config/systemd/user/spanpaper.service` enabled
    ///             via `systemctl --user`. Better semantics
    ///             (restart-on-failure, journald logs), but the unit
    ///             is gated on `graphical-session.target` +
    ///             `WAYLAND_DISPLAY`, which Budgie's session does NOT
    ///             activate / import — pick xdg there.
    ///   both    — install both. Redundant but harmless; useful when
    ///             testing autostart behaviour across sessions.
    ///
    /// The tray (when present) always uses xdg — no service file is
    /// shipped for it, and restart-on-failure isn't important for the
    /// UI applet.
    #[arg(long, value_name = "MODE", default_value = "xdg")]
    pub method: String,

    /// Also start the daemon (and tray, if installed) now, instead of
    /// waiting for the next login.
    #[arg(long)]
    pub start: bool,
}

#[derive(Args, Debug)]
pub struct SetArgs {
    /// Content for the spanned monitor group. Image or video — auto-detected.
    #[arg(long, value_name = "PATH")]
    pub span: Option<PathBuf>,

    /// Content for the side monitor. Image or video — auto-detected.
    #[arg(long, value_name = "PATH")]
    pub side: Option<PathBuf>,

    /// Unmute video audio. Defaults to muted. Only relevant when --span is a video.
    #[arg(long)]
    pub audio: bool,

    /// Mute video audio (explicit, overrides --audio).
    #[arg(long, conflicts_with = "audio")]
    pub no_audio: bool,

    /// Comma-separated output names to span over, top→bottom (or left→right).
    /// Example: --span-outputs HDMI-A-4,DP-6
    #[arg(long, value_name = "NAMES", value_delimiter = ',')]
    pub span_outputs: Option<Vec<String>>,

    /// Output name to display the side content on.
    #[arg(long, value_name = "NAME")]
    pub side_output: Option<String>,

    /// Fit mode for the span content: crop | fit | stretch.
    #[arg(long, value_name = "MODE")]
    pub span_fit: Option<String>,

    /// Fit mode for the side content: crop | fit | stretch. Independent
    /// of --span-fit. Applies to both side images and side videos.
    #[arg(long, value_name = "MODE")]
    pub side_fit: Option<String>,

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
        Cmd::Install(a) => cmd_install(a),
        Cmd::Preset(sub) => cmd_preset(sub),
    }
}

fn cmd_preset(sub: PresetCmd) -> Result<()> {
    use spanpaper::config::validate_preset_name;

    let mut cfg = Config::load_or_default().context("loading config")?;

    match sub {
        PresetCmd::Save { name } => {
            validate_preset_name(&name)?;
            let preset = cfg.snapshot_as_preset(name.clone());
            // Overwrite if present, otherwise append (insertion order).
            match cfg.presets.iter().position(|p| p.name == name) {
                Some(i) => cfg.presets[i] = preset,
                None    => cfg.presets.push(preset),
            }
            cfg.active_preset = Some(name.clone());
            cfg.save().context("save config")?;
            tracing::info!("saved preset {name:?}");
        }
        PresetCmd::List => {
            if cfg.presets.is_empty() {
                println!("(no presets saved)");
            } else {
                for p in &cfg.presets {
                    let marker = if cfg.active_preset.as_deref() == Some(p.name.as_str()) {
                        "*"
                    } else {
                        " "
                    };
                    println!("{marker} {}", p.name);
                }
            }
        }
        PresetCmd::Load { name } => {
            cfg.apply_preset(&name).context("load preset")?;
            cfg.save().context("save config")?;
            tracing::info!("loaded preset {name:?}");
            sighup_running_daemon();
        }
        PresetCmd::Delete { name } => {
            let before = cfg.presets.len();
            cfg.presets.retain(|p| p.name != name);
            if cfg.presets.len() == before {
                anyhow::bail!("no preset named {name:?}");
            }
            if cfg.active_preset.as_deref() == Some(name.as_str()) {
                cfg.active_preset = None;
            }
            cfg.save().context("save config")?;
            tracing::info!("deleted preset {name:?}");
        }
        PresetCmd::Rename { old, new } => {
            validate_preset_name(&new)?;
            let i = cfg.presets.iter().position(|p| p.name == old)
                .with_context(|| format!("no preset named {old:?}"))?;
            if cfg.presets.iter().any(|p| p.name == new) && new != old {
                anyhow::bail!("preset {new:?} already exists");
            }
            cfg.presets[i].name = new.clone();
            if cfg.active_preset.as_deref() == Some(old.as_str()) {
                cfg.active_preset = Some(new.clone());
            }
            cfg.save().context("save config")?;
            tracing::info!("renamed preset {old:?} -> {new:?}");
        }
        PresetCmd::Next => cycle_preset(&mut cfg, 1)?,
        PresetCmd::Prev => cycle_preset(&mut cfg, -1)?,
    }
    Ok(())
}

fn cycle_preset(cfg: &mut Config, delta: isize) -> Result<()> {
    if cfg.presets.is_empty() {
        anyhow::bail!("no presets saved — use `spanpaper preset save NAME` first");
    }
    let len = cfg.presets.len() as isize;
    let next_idx = match cfg.active_preset.as_deref().and_then(|n| cfg.preset_index(n)) {
        Some(i) => ((i as isize + delta).rem_euclid(len)) as usize,
        // Nothing active (or stale name) — start cycle at index 0,
        // independent of direction. Predictable: cycle from a clean
        // state always begins at the first preset.
        None => 0,
    };
    let name = cfg.presets[next_idx].name.clone();
    cfg.apply_preset(&name).expect("apply just-indexed preset");
    cfg.save().context("save config")?;
    tracing::info!("cycled to preset {name:?} (index {next_idx})");
    sighup_running_daemon();
    Ok(())
}

fn sighup_running_daemon() {
    match daemon::reload() {
        Ok(()) => tracing::info!("running daemon reloaded"),
        Err(e) if e.to_string().contains("not running") => {
            tracing::info!("daemon not running (use `spanpaper start` to launch)")
        }
        Err(e) => tracing::warn!("reload failed: {e:#}"),
    }
}

fn cmd_install(a: InstallArgs) -> Result<()> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let exe = std::env::current_exe().context("locate current_exe")?;
    let exe_str = exe.to_string_lossy();
    let exe_dir = exe.parent().context("exe has no parent dir")?;
    let tray_exe = exe_dir.join("spanpaper-tray");

    let want_xdg = matches!(a.method.as_str(), "xdg" | "both");
    let want_systemd = matches!(a.method.as_str(), "systemd" | "both");
    if !want_xdg && !want_systemd {
        anyhow::bail!(
            "--method must be xdg, systemd, or both (got {:?})",
            a.method
        );
    }

    let config_dir = dirs::config_dir().context("XDG_CONFIG_HOME not set")?;
    let autostart_dir = config_dir.join("autostart");
    let systemd_user_dir = config_dir.join("systemd").join("user");

    if want_xdg {
        std::fs::create_dir_all(&autostart_dir)
            .with_context(|| format!("mkdir {}", autostart_dir.display()))?;
    }
    if want_systemd {
        std::fs::create_dir_all(&systemd_user_dir)
            .with_context(|| format!("mkdir {}", systemd_user_dir.display()))?;
    }

    // ---- Daemon autostart ----------------------------------------------
    if want_xdg {
        // Mirrors contrib/spanpaper.desktop but generated inline so we
        // don't depend on /usr/share/spanpaper/ being present (lets
        // `spanpaper install` work in source-install setups too, not
        // just pacman ones).
        let daemon_desktop = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=spanpaper\n\
             Comment=Spanning video wallpaper for Wayland\n\
             Exec={exe_str} start --background\n\
             Terminal=false\n\
             Categories=Utility;\n\
             X-GNOME-Autostart-enabled=true\n\
             NoDisplay=true\n"
        );
        let daemon_dst = autostart_dir.join("spanpaper.desktop");
        write_autostart(&daemon_dst, &daemon_desktop)?;
        println!("wrote {}", daemon_dst.display());
    }
    if want_systemd {
        // systemd --user unit. Mirrors contrib/spanpaper.service with
        // the @SPANPAPER_BIN@ placeholder already substituted.
        let unit = format!(
            "[Unit]\n\
             Description=spanpaper — spanning video wallpaper for Wayland\n\
             PartOf=graphical-session.target\n\
             After=graphical-session.target\n\
             ConditionEnvironment=WAYLAND_DISPLAY\n\
             \n\
             [Service]\n\
             Type=simple\n\
             ExecStart={exe_str} start\n\
             Restart=on-failure\n\
             RestartSec=2\n\
             \n\
             [Install]\n\
             WantedBy=graphical-session.target\n"
        );
        let unit_dst = systemd_user_dir.join("spanpaper.service");
        write_autostart(&unit_dst, &unit)?;
        println!("wrote {}", unit_dst.display());

        // Wire systemd: daemon-reload picks up the new unit; enable
        // (without --now) sets WantedBy symlinks. `--start` below
        // calls `start` on it; otherwise it kicks in at next login
        // (or whenever graphical-session.target activates).
        run_systemctl(&["daemon-reload"])?;
        run_systemctl(&["enable", "spanpaper.service"])?;
        println!("enabled spanpaper.service (systemctl --user)");
    }

    // ---- Tray autostart (always XDG) -----------------------------------
    // No service file is shipped for the tray — restart-on-failure
    // isn't important for a UI applet, and shipping a unit just to
    // keep methods symmetric isn't worth the maintenance.
    let tray_present = tray_exe.is_file();
    if tray_present {
        std::fs::create_dir_all(&autostart_dir)
            .with_context(|| format!("mkdir {}", autostart_dir.display()))?;
    }
    if tray_present {
        let tray_str = tray_exe.to_string_lossy();
        let tray_desktop = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=spanpaper tray\n\
             Comment=Tray applet for spanpaper — panel icon + layout palette\n\
             Exec={tray_str}\n\
             Icon=preferences-desktop-wallpaper\n\
             Terminal=false\n\
             Categories=Utility;\n\
             X-GNOME-Autostart-enabled=true\n\
             NoDisplay=true\n"
        );
        let tray_dst = autostart_dir.join("spanpaper-tray.desktop");
        write_autostart(&tray_dst, &tray_desktop)?;
        println!("wrote {}", tray_dst.display());
    } else {
        println!(
            "note: spanpaper-tray not found alongside this binary — skipping tray autostart.\n\
             (Tray is shipped with the pacman package and via `setup.sh --with-tray`.)"
        );
    }

    if a.start {
        // Daemon: skip start if one's already running. Otherwise pick
        // the start path matching the autostart method we just wrote:
        //   * systemd → `systemctl --user start spanpaper.service`
        //     so the service is alive *under* its supervisor, matching
        //     the restart-on-failure semantics the user opted into.
        //   * xdg / xdg+systemd: spawn detached the old way. (On the
        //     "both" path, the systemd unit also got enabled and will
        //     take over from the next login.)
        match daemon::current_pid() {
            Ok(pid) => println!("daemon already running (pid {pid}) — skipping start"),
            Err(_) => {
                if want_systemd && !want_xdg {
                    run_systemctl(&["start", "spanpaper.service"])?;
                    println!("started spanpaper.service (systemctl --user)");
                } else if let Err(e) = daemon::spawn_background() {
                    tracing::warn!("spawn daemon: {e:#}");
                }
            }
        }

        if tray_present {
            // Same idea for the tray. ksni registers a unique D-Bus
            // name per process; spawning a second one gives the user
            // two icons in the panel. Cheapest reliable check:
            // pgrep -x spanpaper-tray.
            let tray_running = Command::new("pgrep")
                .args(["-x", "spanpaper-tray"])
                .stdout(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if tray_running {
                println!("spanpaper-tray already running — skipping start");
            } else {
                let res = unsafe {
                    Command::new(&tray_exe)
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .pre_exec(|| {
                            nix::unistd::setsid().ok();
                            Ok(())
                        })
                        .spawn()
                };
                match res {
                    Ok(child) => println!("started spanpaper-tray (pid {})", child.id()),
                    Err(e) => tracing::warn!("spawn tray: {e}"),
                }
            }
        }
    } else {
        println!();
        println!("autostart will run at next login.");
        println!("to start now without re-logging in, run:");
        println!("  spanpaper install --start");
    }
    Ok(())
}

fn write_autostart(path: &std::path::Path, content: &str) -> Result<()> {
    // tmp-extension chosen to be benign for both .desktop and .service.
    let tmp = path.with_extension(
        format!("{}.tmp", path.extension().and_then(|e| e.to_str()).unwrap_or("tmp")),
    );
    std::fs::write(&tmp, content)
        .with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("rename to {}", path.display()))?;
    Ok(())
}

/// Run `systemctl --user <args>`, surfacing the exit status as an error
/// when it fails so the caller knows the operation didn't take.
fn run_systemctl(args: &[&str]) -> Result<()> {
    use std::process::Command;
    let status = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .status()
        .with_context(|| format!("invoke systemctl --user {args:?}"))?;
    if !status.success() {
        anyhow::bail!("systemctl --user {:?} exited {}", args, status);
    }
    Ok(())
}

fn cmd_set(a: SetArgs) -> Result<()> {
    let mut cfg = Config::load_or_default().context("loading config")?;
    // Track whether anything preset-relevant changed; if so, the
    // active config no longer matches whatever preset was loaded, so
    // we clear `active_preset` (strict mode — see docs/v0.4-plan.md).
    let mut preset_relevant_changed = false;

    if let Some(v) = a.span {
        cfg.span = Some(validated_media_path(&v, "--span")?);
        preset_relevant_changed = true;
    }
    if let Some(i) = a.side {
        cfg.side = Some(validated_media_path(&i, "--side")?);
        preset_relevant_changed = true;
    }
    if a.audio   { cfg.audio = true;  preset_relevant_changed = true; }
    if a.no_audio{ cfg.audio = false; preset_relevant_changed = true; }
    if let Some(s) = a.span_outputs { cfg.span_outputs = s; }
    if let Some(o) = a.side_output  { cfg.side_output = Some(o); }
    if let Some(f) = a.span_fit     { cfg.span_fit = f; preset_relevant_changed = true; }
    if let Some(f) = a.side_fit     { cfg.side_fit = f; preset_relevant_changed = true; }

    if preset_relevant_changed {
        if let Some(name) = cfg.active_preset.take() {
            tracing::info!(
                "active config diverged from preset {name:?} — clearing active_preset"
            );
        }
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
    println!(
        "daemon: {}",
        match pid {
            Some(p) => format!("running (pid {p})"),
            None    => "not running".into(),
        }
    );

    match Config::load() {
        Ok(cfg) => {
            println!("config: {}", Config::path()?.display());
            println!("  span         = {:?}", cfg.span);
            println!("  side         = {:?}", cfg.side);
            println!("  audio        = {}", cfg.audio);
            println!("  span_outputs = {:?}", cfg.span_outputs);
            println!("  side_output  = {:?}", cfg.side_output);
            println!("  span_fit     = {}", cfg.span_fit);
            println!("  side_fit     = {}", cfg.side_fit);
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

/// Validate, expand ~ / env vars, and canonicalize a user-supplied media
/// path. Rejects obviously-broken input (empty / newlines / NUL bytes — no
/// real Linux filesystem path contains those, and accepting them has
/// corrupted config files in the past when callers piped multi-line shell
/// output into `--span`/`--side`). Warns but doesn't fail on a missing
/// file: the config may legitimately be written before the media is
/// placed, and the daemon validates again at load time.
fn validated_media_path(p: &std::path::Path, flag: &str) -> Result<PathBuf> {
    let raw = p.to_string_lossy();
    if raw.is_empty() {
        anyhow::bail!("{flag}: empty path");
    }
    if raw.contains('\n') || raw.contains('\0') {
        anyhow::bail!(
            "{flag}: path contains newline or NUL byte — \
             refusing to write garbage into config (got {raw:?})"
        );
    }

    let expanded = shellexpand::full(&raw)
        .map_err(|e| anyhow::anyhow!("{flag}: path expansion: {e}"))?
        .into_owned();
    let pb = PathBuf::from(expanded);
    let resolved = pb.canonicalize().unwrap_or_else(|_| pb.clone());

    if !resolved.exists() {
        tracing::warn!(
            "{flag}: path does not exist yet — writing to config anyway, \
             but the daemon will refuse to start until it appears: {}",
            resolved.display()
        );
    }
    Ok(resolved)
}
