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

```bash
git clone <this repo> && cd spanpaper
./setup.sh --autostart=systemd --start         # build + install + enable + start
```

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

Old `--video` / `--left-image` flags and old config field names (`video`,
`left_image`, `image_output`, `image_mode`, `video_fit`) are still
accepted as aliases and silently migrated on the next save.

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
background surface with `hwdec=auto-safe`, so video playback is
hardware-decoded (VA-API on Intel/AMD, NVDEC on NVIDIA). Two decoders
opening the same file start in lockstep and stay in sync at the millisecond
level for the lifetime of a session.

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
  --side-mode    fill
```

Example written config:

```toml
span         = "/home/you/Wallpapers/sky-1920x2160.mp4"   # image or video
side         = "/home/you/Wallpapers/forest.jpg"          # image or video
audio        = false
span_outputs = ["HDMI-A-4", "DP-6"]
side_output  = "DP-5"
side_mode    = "fill"              # swaybg: fill | fit | stretch | center | tile
span_direction = "vertical"        # "vertical" stacks | "horizontal" side-by-side
span_fit     = "crop"              # crop (zoom-fill) | fit (letterbox) | stretch
extra_mpv_options = []             # raw mpv opts appended to every video worker
```

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

`hwdec=auto-safe` is on by default, so VA-API/NVDEC kicks in when present.

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

## Layout

```
spanpaper/
├── Cargo.toml
├── README.md                  (this file)
├── TODO.md                    follow-ups
├── setup.sh                   one-shot installer
├── gen-test-assets.sh         span-continuity calibration generator
├── contrib/
│   ├── spanpaper.service      systemd --user unit
│   └── spanpaper.desktop      XDG autostart entry
├── test-assets/               (generated; gitignored)
└── src/
    ├── main.rs                tracing init + CLI dispatch
    ├── cli.rs                 clap definitions
    ├── config.rs              TOML load/save (atomic write, schema migrations)
    ├── media.rs               image-vs-video content-type detection
    ├── outputs.rs             wl_output + xdg-output enumeration
    ├── workers.rs             mpvpaper / swaybg subprocess plan & supervisors
    └── daemon.rs              pid file, signal handling, supervisor loop
```

## License

MIT.
