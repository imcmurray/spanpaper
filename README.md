# spanpaper

True single-MP4 video wallpaper spanning across stacked monitors on Wayland.
One source video, sliced top/bottom (or left/right) across two outputs, no
pre-splitting, hardware-accelerated, proper `wlr-layer-shell` background — and
an independent static image on a third monitor.

## Status

Built and **validated** on EndeavourOS + Budgie (wlroots Wayland) with:

| Output     | Mode       | Role                         |
|------------|------------|------------------------------|
| `HDMI-A-4` | 1920×1080  | top half of the spanned video|
| `DP-6`     | 1920×1080  | bottom half of the spanned video |
| `DP-5`     | 1080×1920  | independent static image (portrait) |

Frame-level sync between the two `mpvpaper` instances was verified live via
on-screen timestamps matching to the millisecond, and a diagonal + ring
calibration confirmed the seam crop lines up to the pixel.

## Quick start

```bash
git clone <this repo> && cd spanpaper
./setup.sh --autostart=systemd --start         # build + install + enable + start
```

Then point it at content:

```bash
spanpaper set \
  --video      ~/Wallpapers/span-1920x2160.mp4 \
  --left-image ~/Wallpapers/side.jpg
```

`setup.sh --help` lists every flag. The sections below explain what it
automates.

## How it works

`spanpaper` is a small Rust daemon that:

1. Enumerates Wayland outputs natively via `wl_output` + `xdg-output`.
2. For each output in `span_outputs`, spawns one `mpvpaper` that opens the
   same source MP4 with a per-monitor `vf=crop=iw:ih/N:0:ih*i/N` filter.
   Each instance only renders its slice; to the user it looks like one
   continuous video stretched across the stack.
3. For `image_output`, spawns one `swaybg` with the static image.
4. Supervises children: restart-on-crash with linear backoff (caps at 5
   rapid failures), `SIGTERM` → graceful shutdown, `SIGHUP` → hot reload
   of config without dropping workers longer than necessary.

Both `mpvpaper` instances use libmpv's render API into their own
`wlr-layer-shell` background surfaces with `hwdec=auto-safe`, so playback
is hardware-decoded (VA-API on Intel/AMD, NVDEC on NVIDIA). Two decoders
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
the `set` subcommand (validates paths, atomically rewrites the file,
SIGHUPs a running daemon for hot reload):

```bash
spanpaper set \
  --video       ~/Wallpapers/span-1920x2160.mp4 \
  --left-image  ~/Wallpapers/forest.jpg \
  --span-outputs HDMI-A-4,DP-6 \
  --image-output DP-5 \
  --image-mode fill
```

Example written config:

```toml
video        = "/home/you/Wallpapers/sky-1920x2160.mp4"
left_image   = "/home/you/Wallpapers/forest.jpg"
audio        = false
span_outputs = ["HDMI-A-4", "DP-6"]
image_output = "DP-5"
image_mode   = "fill"              # swaybg: fill | fit | stretch | center | tile
span_direction = "vertical"        # "vertical" stacks | "horizontal" side-by-side
video_fit    = "crop"              # crop (zoom-fill) | fit (letterbox) | stretch
extra_mpv_options = []             # raw mpv opts appended to every video worker
```

## Test / calibrate

Before pointing it at the wallpaper you actually want, run the included
generator to produce a span-continuity test pattern:

```bash
./gen-test-assets.sh
spanpaper set \
  --video      ./test-assets/test-span-1920x2160.mp4 \
  --left-image ./test-assets/test-side-1080x1920.png
spanpaper start --background
```

What to look for on the screens:

* **Yellow diagonal** from `(0,0)` to `(1920,2160)` must form **one straight
  line** across the seam. Any kink or duplication = misconfigured outputs.
* **White concentric circles** are centered exactly on `y=1080`; they
  render as full circles only if the two halves are spanned, not
  duplicated.
* **Row labels** `row 0…row 11` flow continuously across the seam (rows
  0–5 on the top monitor, 6–11 on the bottom).
* **Two timestamps** appear straddling the seam — they should always match
  digit-for-digit, proving frame-level sync between the two mpvpaper
  instances.

## Autostart

Three options, install whichever you prefer (`setup.sh` automates this).

**A. systemd `--user` unit** (recommended; restart-on-failure + journal logs):

```bash
install -Dm644 contrib/spanpaper.service ~/.config/systemd/user/spanpaper.service
systemctl --user daemon-reload
systemctl --user enable --now spanpaper.service
journalctl --user -u spanpaper -f       # live logs
```

**B. XDG autostart `.desktop`** (works in Budgie, GNOME-flavoured sessions, etc.):

```bash
install -Dm644 contrib/spanpaper.desktop ~/.config/autostart/spanpaper.desktop
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

spanpaper set --video ~/foo.mp4    # rewrite config + SIGHUP running daemon
spanpaper set --video ~/foo.mp4 --no-reload   # rewrite only
```

## Picking / encoding a source video

The ideal source for a 1920×1080 + 1920×1080 vertical stack is a single
**1920 × 2160** MP4. Anything else gets cropped/fit/stretched per the
`video_fit` setting.

Re-encode to hardware-decode-friendly H.264 8-bit `yuv420p`:

```bash
ffmpeg -i source.mp4 \
  -c:v libx264 -preset slow -crf 20 \
  -pix_fmt yuv420p -movflags +faststart -an \
  -vf "scale=1920:2160:flags=lanczos" \
  ~/Wallpapers/span-1920x2160.mp4
```

`hwdec=auto-safe` is on by default, so VA-API/NVDEC kicks in when present.

## Troubleshooting

| Symptom | Likely cause / fix |
|---|---|
| `mpvpaper not found` | `sudo pacman -S mpvpaper` |
| `compositor does not expose zwlr_layer_shell_v1` | You're on Mutter/GNOME or X11; switch to a wlroots session |
| Diagonal breaks at the seam in the test image | The two outputs aren't actually contiguous in the compositor's layout; check `spanpaper outputs` against your display configuration |
| Two diagonals (one per monitor) instead of one | `span_outputs` lists only one output, or the daemon found one to be missing |
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
    ├── config.rs              TOML load/save (atomic write)
    ├── outputs.rs             wl_output + xdg-output enumeration
    ├── workers.rs             mpvpaper / swaybg subprocess plan & supervisors
    └── daemon.rs              pid file, signal handling, supervisor loop
```

## License

MIT.
