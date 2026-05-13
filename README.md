# spanpaper

Single-source wallpaper that spans stacked monitors on Wayland — give it
**any image or video** and one file gets sliced top/bottom (or left/right)
across two outputs, no pre-splitting, hardware-accelerated, proper
`wlr-layer-shell` background. A third monitor gets its own independent
image or video.

## Status

Built and **validated** on EndeavourOS + Budgie (wlroots Wayland) with:

| Output     | Mode       | Role                                       |
|------------|------------|--------------------------------------------|
| `HDMI-A-4` | 1920×1080  | top half of the spanned content            |
| `DP-6`     | 1920×1080  | bottom half of the spanned content         |
| `DP-5`     | 1080×1920  | independent side content (image or video)  |

Frame-level sync between the two `mpvpaper` instances was verified live via
on-screen timestamps matching to the millisecond; a diagonal + ring
calibration confirmed the seam crop lines up to the pixel; and a hot-reload
swap from MP4 → PNG → MP4 confirmed image/video auto-routing.

## Quick start

### Arch Linux — install the prebuilt pacman package

Each tagged release ships a `.pkg.tar.zst` as a GitHub release asset. No
clone, no Rust toolchain, no compile:

```bash
# Substitute the latest version — check the Releases page.
VERSION=0.3.0 PKGREL=1
curl -LO "https://github.com/imcmurray/spanpaper/releases/download/v$VERSION/spanpaper-bin-$VERSION-$PKGREL-x86_64.pkg.tar.zst"
sudo pacman -U "spanpaper-bin-$VERSION-$PKGREL-x86_64.pkg.tar.zst"
```

Pacman pulls every runtime dep (`mpvpaper`, `swaybg`, `gtk4`,
`gtk4-layer-shell`) and installs both binaries:

* `/usr/bin/spanpaper` — the daemon
* `/usr/bin/spanpaper-tray` — the optional panel applet (see
  [Tray applet](#tray-applet-optional))

Plus a (not-enabled) systemd `--user` unit at
`/usr/lib/systemd/user/spanpaper.service` and sample XDG autostart
entries at `/usr/share/spanpaper/autostart/spanpaper{,-tray}.desktop`
that you can copy into `~/.config/autostart/` to start at login.
Uninstall is `sudo pacman -R spanpaper-bin`.

### Any wlroots Wayland distro — build from source

```bash
git clone https://github.com/imcmurray/spanpaper && cd spanpaper
./setup.sh --autostart=xdg --start             # build + install + enable + start
```

(Use `--autostart=systemd` instead on Sway/Hyprland/river/etc. — see
[Autostart](#autostart) for which path your session needs.)

Then point it at content:

```bash
spanpaper set \
  --span ~/Wallpapers/anything.mp4-or-png \
  --side ~/Wallpapers/anything.jpg-or-mp4
```

Both `--span` and `--side` accept **either an image or a video** — content
type is auto-detected from extension (with `file(1)` MIME fallback) and the
right backend is chosen for you:

| You provide | Span outputs (stacked) | Side output |
|---|---|---|
| Video (`.mp4`, `.mkv`, `.webm`, …) | mpvpaper × N, top/bottom crop | mpvpaper × 1, no crop |
| Image (`.jpg`, `.png`, `.webp`, …) | mpvpaper × N, held as still frame, top/bottom crop | swaybg (lighter than libmpv for a still) |

`setup.sh --help` lists every flag. The sections below explain what it
automates.

## How it works

`spanpaper` is a small Rust daemon that:

1. Enumerates Wayland outputs natively via `wl_output` + `xdg-output`.
2. **Auto-detects content type** for each slot from the file extension (with
   `file(1)` MIME fallback), then routes to the right backend:
   * `span` slot (always mpvpaper, per-monitor crop):
     - Video → libmpv decodes and renders its cropped slice
     - Image → libmpv holds the single frame via `image-display-duration=inf`
       and renders the cropped slice as a still
   * `side` slot:
     - Image → swaybg (lighter than libmpv for a still)
     - Video → mpvpaper with no crop
3. Supervises children: restart-on-crash with linear backoff (caps at 5
   rapid failures), `SIGTERM` → graceful shutdown, `SIGHUP` → hot reload
   of config without dropping workers longer than necessary.

Every `mpvpaper` uses libmpv's render API into its own `wlr-layer-shell`
background surface. Video playback is hardware-decoded (VA-API on Intel/AMD,
NVDEC on NVIDIA): span workers run `hwdec=auto-copy-safe` so libavfilter
sees CPU-resident frames for the scale/crop chain; the solo side worker
runs the slightly faster `hwdec=auto-safe` because it doesn't apply
software filters.

**Span sync.** Two independent `mpvpaper` decoders looping the same file
will drift in and out of phase over time — most visibly when a SIGHUP
reload kicks off three new `mpvpaper`s (span pair + side video) in one
spawn batch, where hwdec init contends and each instance reaches "first
frame ready" at slightly different wall-clock times. spanpaper handles
this by spawning each span worker with `--input-ipc-server=$XDG_RUNTIME_DIR/spanpaper/mpv-<output>.sock`
and `pause=yes start=0`; once every socket is connectable, the daemon
broadcasts a synchronous unpause to all span workers within a few hundred
microseconds. Measured drift between the two span instances is 0 ms
across reloads, side-swap SIGHUPs, and loop-boundary wraparounds.

## Requirements

* A **wlroots-based Wayland session** (Sway, Hyprland, river, Wayfire,
  labwc, wlroots-based Budgie). Plain GNOME / Mutter does **not** expose
  `wlr-layer-shell` and no wallpaper daemon — this one, `swaybg`, `swww`,
  `mpvpaper` — will work there. Verify with `wayland-info | grep
  zwlr_layer_shell`.
* Arch / EndeavourOS packages: `rust mpvpaper swaybg` (`setup.sh` installs
  the runtime two for you).

## Build & install (manual path)

```bash
sudo pacman -S rust mpvpaper swaybg
cd spanpaper
cargo build --release
install -Dm755 target/release/spanpaper ~/.local/bin/spanpaper
```

Ensure `~/.local/bin` is on `PATH`:

```bash
case ":$PATH:" in *":$HOME/.local/bin:"*) ;; *)
  echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bash_profile
esac
```

## Configure

Config lives at `~/.config/spanpaper/config.toml`. Edit it directly, or use
the `set` subcommand (validates paths, auto-detects media type, atomically
rewrites the file, SIGHUPs a running daemon for hot reload):

```bash
spanpaper set \
  --span         ~/Wallpapers/anything.mp4-or-jpg \
  --side         ~/Wallpapers/anything.jpg-or-mp4 \
  --span-outputs HDMI-A-4,DP-6 \
  --side-output  DP-5 \
  --span-fit     crop \
  --side-fit     crop
```

Example written config:

```toml
span         = "/home/you/Wallpapers/sky-1920x2160.mp4"   # image or video
side         = "/home/you/Wallpapers/forest.jpg"          # image or video
audio        = false
span_outputs = ["HDMI-A-4", "DP-6"]
side_output  = "DP-5"
span_direction = "vertical"        # "vertical" stacks | "horizontal" side-by-side
span_fit     = "crop"              # crop (zoom-fill) | fit (letterbox) | stretch
side_fit     = "crop"              # same three values; independent of span_fit
extra_mpv_options = []             # raw mpv opts appended to every video worker
```

## Right-click in your file manager

Both `setup.sh` and the pacman packages install two MIME-only `.desktop`
entries — `spanpaper-set-span` and `spanpaper-set-side`. They don't appear
in the application menu, but any file manager that respects XDG MIME
associations (Nautilus / Files, Nemo, Dolphin, Thunar, PCManFM-Qt, …)
offers them under **Open With → Set as spanpaper span / Set as spanpaper
side** for any image or video.

Picking one is exactly equivalent to running:

```bash
spanpaper set --span /path/to/that/file        # or --side
```

— same atomic config rewrite, same SIGHUP hot-reload, no terminal. If the
file's "Open With" list doesn't show them right away, run
`update-desktop-database ~/.local/share/applications` (source install) or
log out and back in (pacman install) to refresh the MIME cache.

## Test / calibrate

Before pointing it at the wallpaper you actually want, run the included
generator to produce a span-continuity test pattern:

```bash
./gen-test-assets.sh                       # writes test-assets/*.{png,mp4}
spanpaper set \
  --span ./test-assets/test-span-1920x2160.mp4 \
  --side ./test-assets/test-side-1080x1920.png
spanpaper start --background

# Try the image path too — same hot reload, no daemon restart needed:
spanpaper set --span ./test-assets/test-span-1920x2160.png
```

What to look for on the screens:

* **Yellow diagonal** from `(0,0)` to `(1920,2160)` must form **one straight
  line** across the seam. Any kink or duplication = misconfigured outputs.
* **White concentric circles** are centered exactly on `y=1080`; they
  render as full circles only if the two halves are spanned, not
  duplicated.
* **Row labels** `row 0…row 11` flow continuously across the seam (rows
  0–5 on the top monitor, 6–11 on the bottom).
* **Two timestamps** appear straddling the seam (in the MP4 only) — they
  should always match digit-for-digit, proving frame-level sync between
  the two mpvpaper instances. The PNG version has no animated elements
  but every static calibration mark (diagonal, circles, rows) still
  applies.

## Autostart

Three options, install whichever you prefer (`setup.sh` automates this).
The two paths are safe to coexist — `spanpaper start` checks the pid file
and exits cleanly if a daemon is already running.

**A. XDG autostart `.desktop`** (required on Budgie; works on most XDG sessions):

```bash
install -Dm644 contrib/spanpaper.desktop ~/.config/autostart/spanpaper.desktop
```

> **Budgie note**: Budgie's session does *not* activate
> `graphical-session.target`, so any systemd `--user` unit gated on that
> target will stay inactive across logins. XDG autostart is the reliable
> path here. Verify with `systemctl --user list-units --state=active --type=target`
> — if `graphical-session.target` isn't listed, install the `.desktop` above.

**B. systemd `--user` unit** (preferred on Sway/Hyprland/river/etc. — anything
that *does* activate `graphical-session.target`; gives restart-on-failure
plus journald logs):

```bash
install -Dm644 contrib/spanpaper.service ~/.config/systemd/user/spanpaper.service
systemctl --user daemon-reload
systemctl --user enable --now spanpaper.service
journalctl --user -u spanpaper -f       # live logs
```

**C. Budgie Menu → Startup Applications** → command `spanpaper start --background`.

## Tray applet (optional)

A second, optional binary — `spanpaper-tray` — adds a panel
StatusNotifierItem with a layout palette. Left-click the icon to open
a small window that draws your monitor topology to scale; each output
shows a thumbnail of its current content, and you can:

* **Drag any image/video** from your file manager onto a rectangle to
  assign it.
* Click **Change…** for a portal-backed file picker (works in any
  session that has `xdg-desktop-portal` — every modern desktop).
* Right-click the panel icon for span-fit / side-fit / audio /
  pause-resume / open-config-folder / reload-config.

The icon updates state: full-colour wallpaper glyph when the daemon is
running, pause glyph when paused, stop glyph when the daemon is down.
Closing the palette (clicking outside, or the X) only closes the
window — the tray keeps running in the panel.

The tray is **feature-gated** (`cargo build --features tray`) so a
default `cargo build` stays GTK-free for power users who only want
the daemon. The pacman package and `setup.sh --with-tray` both ship
the tray:

```bash
# Source path:
./setup.sh --with-tray --autostart=xdg --start
```

That builds both binaries, installs the autostart entry
(`~/.config/autostart/spanpaper-tray.desktop`), and starts the
daemon. The pacman install path bundles `/usr/bin/spanpaper-tray`
and the sample autostart at
`/usr/share/spanpaper/autostart/spanpaper-tray.desktop` — copy that
into `~/.config/autostart/` to auto-launch the tray at login, or run
it on demand:

```bash
spanpaper-tray &
```

> **GNOME note**: tray icons need an AppIndicator extension on GNOME
> Shell (built in on Budgie, KDE Plasma, Cinnamon, MATE; waybar
> renders them natively on Sway/Hyprland).

## CLI cheatsheet

```bash
spanpaper outputs              # list detected Wayland outputs (one per line)
spanpaper status               # daemon state + active config + outputs

spanpaper start                # foreground (ctrl-c to stop)
spanpaper start --background   # detached
spanpaper stop
spanpaper restart

spanpaper set --span ~/foo.mp4    # rewrite config + SIGHUP running daemon
spanpaper set --span ~/foo.png    # auto-detected as image, holds as still
spanpaper set --side ~/bar.mp4    # video on DP-5; mpvpaper instead of swaybg
spanpaper set --span ~/foo.mp4 --no-reload    # rewrite only
```

## Picking / encoding source content

The ideal source for a 1920×1080 + 1920×1080 vertical stack is a single
**1920 × 2160** file — video *or* image. Anything else gets cropped, fit,
or stretched per the `span_fit` setting (`crop` is the default — zoom-fill,
may clip sides; `fit` letterboxes; `stretch` ignores aspect).

### Video — re-encode to hardware-decode-friendly H.264 8-bit `yuv420p`:

```bash
ffmpeg -i source.mp4 \
  -c:v libx264 -preset slow -crf 20 \
  -pix_fmt yuv420p -movflags +faststart -an \
  -vf "scale=1920:2160:flags=lanczos" \
  ~/Wallpapers/span-1920x2160.mp4
```

Hardware decoding (VA-API/NVDEC) is on by default — see [How it works](#how-it-works)
for the exact `hwdec` mode chosen per worker.

### Image — anything common works (JPG, PNG, WebP, AVIF, HEIC, GIF…):

```bash
# Resize to the combined-stack resolution for a perfect fit, no
# crop/letterbox.
magick source.jpg -resize 1920x2160^ -gravity center -extent 1920x2160 \
                  ~/Wallpapers/span-1920x2160.jpg
```

Then just `spanpaper set --span ~/Wallpapers/span-1920x2160.jpg`. The image
is held as a single frame; CPU drops to ~0% after the first paint.

## Troubleshooting

| Symptom | Likely cause / fix |
|---|---|
| `mpvpaper not found` | `sudo pacman -S mpvpaper` |
| `compositor does not expose zwlr_layer_shell_v1` | You're on Mutter/GNOME or X11; switch to a wlroots session |
| Diagonal breaks at the seam in the test image | The two outputs aren't actually contiguous in the compositor's layout; check `spanpaper outputs` against your display configuration |
| Two diagonals (one per monitor) instead of one | `span_outputs` lists only one output, or the daemon found one to be missing |
| `unrecognised media type` on `spanpaper set` | File has an unknown extension *and* `file(1)` doesn't classify it as `image/*` or `video/*`. Re-encode to a common format, or install `file`: `sudo pacman -S file` |
| Audio duplicated | spanpaper already restricts audio to the first span output; `spanpaper restart` resyncs |
| High CPU on playback | Hardware decode failed to engage. Check with: `mpv --hwdec=auto-safe --vo=null --frames=1 yourfile.mp4 2>&1 \| grep -i hwdec` |
| Daemon "not running" but `pgrep -f spanpaper` shows it | Stale pid file; `rm "$XDG_RUNTIME_DIR/spanpaper/spanpaper.pid"` then `spanpaper start` |

## Releasing

`release.sh` cuts a new tagged release end-to-end. From a clean `main`:

```bash
./release.sh 0.3.0          # interactive — shows the plan, prompts y/N
./release.sh 0.3.0 -y       # non-interactive
./release.sh 0.3.0 2        # same version, pkgrel bump (e.g. repackaged)
./release.sh -h             # full usage
```

What it does, in order:

1. **Preflight** — refuses unless the tree is clean, you're on `main`, in
   sync with `origin`, the tag and GitHub release don't already exist, and
   all required tools are on PATH (`cargo`, `makepkg`, `fakeroot`, `curl`,
   `sha256sum`, `gh`, `git`).
2. **Bump** — rewrites the `[package]` version in `Cargo.toml` and the
   `pkgver`/`pkgrel` in both `contrib/PKGBUILD*`.
3. **Smoke build** — `cargo build --release` before doing anything
   irreversible.
4. **Commit + tag + push** — `Release vX.Y.Z`, annotated tag, push main +
   tag.
5. **Stage binary tarball** — `dist/spanpaper-X.Y.Z-x86_64.tar.gz`
   containing the release binary, `contrib/`, README, LICENSE.
6. **Compute checksums** — fetches the GitHub source tarball at the new
   tag, writes its sha256 into `contrib/PKGBUILD` and the binary tarball's
   sha256 into `contrib/PKGBUILD-bin`.
7. **Build the pacman package** — runs `makepkg` against `PKGBUILD-bin` to
   produce `spanpaper-bin-X.Y.Z-N-x86_64.pkg.tar.zst`.
8. **Create GitHub release** — uploads both artifacts with install
   instructions in the release body.
9. **Commit the checksum updates** to `main` and push.

If anything fails before step 4, the tree is dirty but no external state has
changed — fix, `git restore .`, retry. Failures after step 4 leave the tag
and release in a partial state; check the release page and clean up by hand
if needed.

The PKGBUILDs in `contrib/` stay valid as standalone build recipes too:
`cd contrib && makepkg -si` (source) or
`cp PKGBUILD-bin /tmp/x/PKGBUILD && cd /tmp/x && makepkg` (binary).

## Layout

```
spanpaper/
├── Cargo.toml
├── LICENSE                    MIT
├── README.md                  (this file)
├── TODO.md                    follow-ups
├── setup.sh                   one-shot installer (source build path)
├── release.sh                 cut a new tagged release end-to-end
├── gen-test-assets.sh         span-continuity calibration generator
├── docs/
│   └── tray-applet-plan.md    design + milestone notes for the tray binary
├── contrib/
│   ├── spanpaper.service                systemd --user unit (uses @SPANPAPER_BIN@)
│   ├── spanpaper.desktop                XDG autostart entry (uses @SPANPAPER_BIN@)
│   ├── spanpaper-set-{span,side}.desktop  right-click "Open With" entries
│   ├── spanpaper-tray.desktop           tray autostart entry (uses @SPANPAPER_TRAY_BIN@)
│   ├── PKGBUILD                         source-build pacman package recipe
│   └── PKGBUILD-bin                     prebuilt-binary pacman package recipe
├── dist/                      (generated; gitignored — release artifacts)
├── test-assets/               (generated; gitignored)
└── src/
    ├── main.rs                tracing init + CLI dispatch (daemon)
    ├── cli.rs                 clap definitions
    ├── config.rs              TOML load/save (atomic write)
    ├── media.rs               image-vs-video content-type detection
    ├── outputs.rs             wl_output + xdg-output enumeration
    ├── workers.rs             mpvpaper / swaybg subprocess plan & supervisors
    ├── ipc.rs                 mpv JSON IPC client for span sync + pause/resume
    ├── daemon.rs              pid file, signal handling, supervisor loop
    └── bin/spanpaper-tray/    optional tray applet (feature = "tray")
        ├── main.rs            tokio + ksni service, GTK4 application
        ├── daemon_client.rs   CLI/IPC client of the daemon
        ├── outputs_query.rs   `spanpaper outputs` parser
        ├── palette.rs         layout-palette popover window
        └── thumbnail.rs       ffmpeg-backed thumbnail cache
```

## License

MIT — see [LICENSE](LICENSE).
