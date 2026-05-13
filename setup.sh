#!/usr/bin/env bash
# setup.sh — one-shot installer for spanpaper.
#
# What it does (in order):
#   1. Verifies Wayland + wlr-layer-shell are available
#   2. Installs runtime deps (mpvpaper, swaybg) via pacman if missing
#   3. Verifies Rust toolchain
#   4. Builds spanpaper in release mode
#   5. Installs the binary to ~/.local/bin/spanpaper
#   6. Ensures ~/.local/bin is on PATH (prints a hint if not)
#   7. Optionally installs a systemd --user unit OR an XDG autostart entry
#   8. Optionally seeds the config if --span / --side are passed
#
# Idempotent: re-runs only do work that's still needed.

set -euo pipefail

# ---- paths -------------------------------------------------------------------

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_SRC="$REPO_DIR/target/release/spanpaper"
BIN_DST="$HOME/.local/bin/spanpaper"
SYSTEMD_DST="$HOME/.config/systemd/user/spanpaper.service"
AUTOSTART_DST="$HOME/.config/autostart/spanpaper.desktop"
APPS_DIR="$HOME/.local/share/applications"
OPENWITH_SPAN_DST="$APPS_DIR/spanpaper-set-span.desktop"
OPENWITH_SIDE_DST="$APPS_DIR/spanpaper-set-side.desktop"

# ---- flags -------------------------------------------------------------------

AUTOSTART_MODE="ask"   # ask | systemd | xdg | none
SKIP_PACMAN=0
SPAN=""
SIDE=""
SPAN_OUTPUTS=""
SIDE_OUTPUT=""
AUDIO=0
START_NOW=0

usage() {
    cat <<EOF
Usage: $0 [options]

Options:
  --autostart=systemd|xdg|none   How to autostart on login (default: ask)
  --skip-pacman                  Don't try to install system packages
  --span PATH                    Pre-seed config: spanning media (image or video)
  --side PATH                    Pre-seed config: side-monitor media (image or video)
  --span-outputs CSV             Override span outputs (e.g. HDMI-A-4,DP-6)
  --side-output NAME             Override side output (e.g. DP-5)
  --audio                        Unmute video
  --start                        Start the daemon at the end
  -h, --help                     This help

Examples:
  $0                                              # interactive
  $0 --autostart=systemd --start                  # install + enable + start
  $0 --span ~/wall.mp4 --side ~/side.jpg \\
     --autostart=systemd --start                  # full setup in one shot
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --autostart=*)      AUTOSTART_MODE="${1#*=}"; shift ;;
        --skip-pacman)      SKIP_PACMAN=1; shift ;;
        --span)                    SPAN="$2"; shift 2 ;;
        --side)                    SIDE="$2"; shift 2 ;;
        --span-outputs)            SPAN_OUTPUTS="$2"; shift 2 ;;
        --side-output)             SIDE_OUTPUT="$2"; shift 2 ;;
        --audio)            AUDIO=1; shift ;;
        --start)            START_NOW=1; shift ;;
        -h|--help)          usage; exit 0 ;;
        *)                  echo "unknown arg: $1" >&2; usage; exit 2 ;;
    esac
done

# ---- pretty printing ---------------------------------------------------------

if [[ -t 1 ]]; then
    C_OK=$'\033[1;32m'; C_WARN=$'\033[1;33m'; C_ERR=$'\033[1;31m'
    C_DIM=$'\033[2m';   C_BOLD=$'\033[1m';    C_RST=$'\033[0m'
else
    C_OK=""; C_WARN=""; C_ERR=""; C_DIM=""; C_BOLD=""; C_RST=""
fi
step() { printf "%s==>%s %s\n" "$C_BOLD" "$C_RST" "$*"; }
ok()   { printf "  %sok%s   %s\n" "$C_OK"   "$C_RST" "$*"; }
warn() { printf "  %swarn%s %s\n" "$C_WARN" "$C_RST" "$*"; }
err()  { printf "  %serr%s  %s\n" "$C_ERR"  "$C_RST" "$*" >&2; }
ask() {
    # ask "prompt" [default y|n]; returns 0 for yes
    local prompt="$1" def="${2:-n}" ans
    local hint="[y/N]"; [[ "$def" == "y" ]] && hint="[Y/n]"
    if [[ ! -t 0 ]]; then return $([[ "$def" == "y" ]] && echo 0 || echo 1); fi
    read -r -p "  $prompt $hint " ans || ans=""
    ans="${ans:-$def}"
    [[ "$ans" =~ ^[Yy]$ ]]
}

# ---- 1. Wayland / layer-shell check ------------------------------------------

step "Checking Wayland session"
if [[ -z "${WAYLAND_DISPLAY:-}" ]]; then
    err "WAYLAND_DISPLAY is not set — you must run this from a Wayland session."
    err "(X11 sessions cannot use wlr-layer-shell.)"
    exit 1
fi
ok "WAYLAND_DISPLAY=$WAYLAND_DISPLAY"

if command -v wayland-info >/dev/null 2>&1; then
    if wayland-info 2>/dev/null | grep -q zwlr_layer_shell; then
        ok "compositor exposes zwlr_layer_shell_v1"
    else
        err "compositor does not expose zwlr_layer_shell_v1."
        err "spanpaper, mpvpaper, and swaybg all require it (wlroots-based session)."
        err "If you are on Budgie+Mutter or GNOME, switch sessions before continuing."
        ask "continue anyway?" n || exit 1
    fi
else
    warn "wayland-info not installed — skipping layer-shell probe."
    warn "(install: sudo pacman -S wayland-utils  — recommended)"
fi

# ---- 2. Runtime deps ---------------------------------------------------------

step "Checking runtime dependencies"
NEED_PKGS=()
command -v mpvpaper >/dev/null || NEED_PKGS+=(mpvpaper)
command -v swaybg   >/dev/null || NEED_PKGS+=(swaybg)

if (( ${#NEED_PKGS[@]} == 0 )); then
    ok "mpvpaper and swaybg already installed"
elif (( SKIP_PACMAN )); then
    warn "missing: ${NEED_PKGS[*]} (--skip-pacman set; install manually)"
else
    warn "missing: ${NEED_PKGS[*]}"
    if ask "install via sudo pacman -S ${NEED_PKGS[*]}?" y; then
        sudo pacman -S --needed --noconfirm "${NEED_PKGS[@]}"
        ok "installed ${NEED_PKGS[*]}"
    else
        err "cannot proceed without mpvpaper and swaybg"
        exit 1
    fi
fi

# ---- 3. Rust toolchain -------------------------------------------------------

step "Checking Rust toolchain"
if ! command -v cargo >/dev/null; then
    err "cargo not on PATH. Install via: sudo pacman -S rust"
    err "(or use rustup if you prefer)"
    exit 1
fi
ok "$(cargo --version)"

# ---- 4. Build ----------------------------------------------------------------

step "Building spanpaper (release)"
cd "$REPO_DIR"
cargo build --release
[[ -x "$BIN_SRC" ]] || { err "build succeeded but $BIN_SRC missing"; exit 1; }
ok "built $BIN_SRC ($(du -h "$BIN_SRC" | cut -f1))"

# ---- 5. Install binary -------------------------------------------------------

step "Installing binary"
install -Dm755 "$BIN_SRC" "$BIN_DST"
ok "installed -> $BIN_DST"

# ---- 5b. "Open With" entries -------------------------------------------------
# Two MimeType-only .desktop files that let file managers offer
# "Open With → Set as spanpaper span / side" on any image or video.
# NoDisplay=true keeps them out of the app menu.
#
# CAREFUL: declaring MimeType= on a .desktop file makes it a candidate
# for "default app for that MIME type". If the user has no explicit
# default set for (say) image/jpeg, gio's fallback picks the
# first-registered associated app — and alphabetically ours often wins,
# which would silently turn every JPEG double-click into a wallpaper
# change. To prevent this, we snapshot the current default for every
# MIME type we claim BEFORE installing, then re-pin those defaults
# AFTER — restoring whatever was there, including the implicit fallback,
# so we only ever appear in "Open With" submenus, never as default.

# Source of truth for this list: contrib/spanpaper-set-*.desktop MimeType=
SPANPAPER_MIMES=(
    image/jpeg image/png image/webp image/bmp image/gif image/tiff
    image/avif image/heif image/jxl
    video/mp4 video/x-matroska video/webm video/quicktime
    video/x-msvideo video/x-ms-wmv video/x-flv video/mp2t video/mpeg
    video/ogg video/3gpp video/3gpp2
)

step "Installing right-click 'Open With' entries"

# 1. Snapshot prior defaults.
declare -A PRIOR_DEFAULTS=()
for mime in "${SPANPAPER_MIMES[@]}"; do
    prior="$(xdg-mime query default "$mime" 2>/dev/null || true)"
    # Skip if no real prior or it was already one of ours (re-run case).
    case "$prior" in
        ""|spanpaper-set-*) ;;
        *) PRIOR_DEFAULTS["$mime"]="$prior" ;;
    esac
done

install -d "$APPS_DIR"
sed "s|@SPANPAPER_BIN@|$BIN_DST|g" \
    "$REPO_DIR/contrib/spanpaper-set-span.desktop" > "$OPENWITH_SPAN_DST"
sed "s|@SPANPAPER_BIN@|$BIN_DST|g" \
    "$REPO_DIR/contrib/spanpaper-set-side.desktop" > "$OPENWITH_SIDE_DST"
chmod 644 "$OPENWITH_SPAN_DST" "$OPENWITH_SIDE_DST"
ok "installed $OPENWITH_SPAN_DST"
ok "installed $OPENWITH_SIDE_DST"
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database -q "$APPS_DIR" 2>/dev/null || true
    ok "refreshed MIME cache (update-desktop-database)"
else
    warn "update-desktop-database not found — entries may take effect after re-login"
fi

# 2. Re-pin captured defaults. If we displaced a real prior default,
# put it back. If the MIME type had no prior default and we became the
# default, warn but don't guess a replacement — the user will set their
# own viewer the first time they double-click.
RESTORED=()
ORPHANED=()
for mime in "${SPANPAPER_MIMES[@]}"; do
    current="$(xdg-mime query default "$mime" 2>/dev/null || true)"
    case "$current" in
        spanpaper-set-*)
            prior="${PRIOR_DEFAULTS[$mime]:-}"
            if [[ -n "$prior" ]]; then
                xdg-mime default "$prior" "$mime" 2>/dev/null && \
                    RESTORED+=("$mime → $prior")
            else
                ORPHANED+=("$mime")
            fi
            ;;
    esac
done
if (( ${#RESTORED[@]} )); then
    ok "restored ${#RESTORED[@]} default(s) we would have displaced"
    for line in "${RESTORED[@]}"; do
        printf "       %s%s%s\n" "$C_DIM" "$line" "$C_RST"
    done
fi
if (( ${#ORPHANED[@]} )); then
    warn "no prior default existed for these types — spanpaper is now their default:"
    for m in "${ORPHANED[@]}"; do
        printf "       %s%s%s\n" "$C_DIM" "$m" "$C_RST"
    done
    warn "pick a viewer with: xdg-mime default <app>.desktop <mimetype>"
    warn "or right-click a file → Open With → set a different default."
fi

# ---- 6. PATH hint ------------------------------------------------------------

case ":$PATH:" in
    *":$HOME/.local/bin:"*)
        ok "~/.local/bin is on PATH"
        ;;
    *)
        warn "~/.local/bin is NOT on your current PATH."
        warn "add this to ~/.bash_profile (or ~/.zprofile):"
        printf "    %sexport PATH=\"\$HOME/.local/bin:\$PATH\"%s\n" "$C_DIM" "$C_RST"
        ;;
esac

# ---- 7. Seed config (optional) -----------------------------------------------

if [[ -n "$SPAN$SIDE$SPAN_OUTPUTS$SIDE_OUTPUT" ]] || (( AUDIO )); then
    step "Seeding config"
    set_args=( set --no-reload )
    [[ -n "$SPAN"          ]] && set_args+=( --span "$SPAN" )
    [[ -n "$SIDE"          ]] && set_args+=( --side "$SIDE" )
    [[ -n "$SPAN_OUTPUTS"  ]] && set_args+=( --span-outputs "$SPAN_OUTPUTS" )
    [[ -n "$SIDE_OUTPUT"   ]] && set_args+=( --side-output "$SIDE_OUTPUT" )
    (( AUDIO ))               && set_args+=( --audio )
    "$BIN_DST" "${set_args[@]}"
    ok "config saved -> ~/.config/spanpaper/config.toml"
fi

# ---- 8. Autostart ------------------------------------------------------------

if [[ "$AUTOSTART_MODE" == "ask" ]]; then
    echo
    step "Autostart"
    printf "  How should spanpaper start at login?\n"
    printf "    1) systemd --user unit (recommended; auto-restart, journald)\n"
    printf "    2) XDG autostart (~/.config/autostart/spanpaper.desktop)\n"
    printf "    3) none — start it yourself\n"
    read -r -p "  choice [1/2/3] (default 1): " choice || choice="1"
    case "${choice:-1}" in
        1) AUTOSTART_MODE="systemd" ;;
        2) AUTOSTART_MODE="xdg" ;;
        *) AUTOSTART_MODE="none" ;;
    esac
fi

# Both autostart files use @SPANPAPER_BIN@ as a placeholder for the real
# binary path; the session's PATH does NOT include ~/.local/bin, so the
# absolute form is required.
substitute_autostart() {
    local src="$1" dst="$2"
    install -Dm644 /dev/null "$dst"
    sed "s|@SPANPAPER_BIN@|$BIN_DST|g" "$src" > "$dst"
    chmod 644 "$dst"
}

case "$AUTOSTART_MODE" in
    systemd)
        step "Installing systemd --user unit"
        substitute_autostart "$REPO_DIR/contrib/spanpaper.service" "$SYSTEMD_DST"
        systemctl --user daemon-reload
        systemctl --user enable spanpaper.service >/dev/null
        ok "enabled $SYSTEMD_DST"
        ok "live logs: journalctl --user -u spanpaper -f"
        ;;
    xdg)
        step "Installing XDG autostart entry"
        substitute_autostart "$REPO_DIR/contrib/spanpaper.desktop" "$AUTOSTART_DST"
        ok "installed $AUTOSTART_DST"
        ;;
    none)
        ok "skipping autostart"
        ;;
    *)
        warn "unknown --autostart mode: $AUTOSTART_MODE (skipping)"
        ;;
esac

# ---- 9. Detected outputs (sanity check) --------------------------------------

step "Detected Wayland outputs"
if "$BIN_DST" outputs 2>/dev/null; then
    :
else
    warn "could not enumerate outputs (compositor may lack xdg-output)"
fi

# ---- 10. Start now? ----------------------------------------------------------

if (( START_NOW )); then
    step "Starting daemon"
    if [[ "$AUTOSTART_MODE" == "systemd" ]]; then
        systemctl --user restart spanpaper.service
        ok "systemctl --user restart spanpaper.service"
    else
        # Stop any existing instance first, ignore failure.
        "$BIN_DST" stop 2>/dev/null || true
        "$BIN_DST" start --background
    fi
    sleep 1
    "$BIN_DST" status || true
fi

# ---- done --------------------------------------------------------------------

echo
echo "${C_BOLD}Done.${C_RST} Next:"
if [[ -z "$SPAN" ]]; then
    echo "  spanpaper set --span /path/to/anything --side /path/to/anything"
fi
if (( ! START_NOW )); then
    if [[ "$AUTOSTART_MODE" == "systemd" ]]; then
        echo "  systemctl --user start spanpaper.service"
    else
        echo "  spanpaper start --background"
    fi
fi
echo "  spanpaper status"
