# spanpaper — outstanding work

## ⏳ Swap the test calibration content for real spanning content

`--span` and `--side` both accept images *or* videos — pick whatever you have.

Daemon is currently running the test-asset calibration video/image. When
you've picked a real spanning MP4 (ideally 1920×2160 to match the
HDMI-A-4 + DP-6 stack):

```bash
# hot-swap the span content (image or video), keep DP-5 untouched
spanpaper set --span /path/to/your-real-content.mp4
spanpaper set --span /path/to/your-real-content.png   # still image also works

# (or swap both at once)
spanpaper set \
  --span /path/to/your-real-content.mp4 \
  --side /path/to/your-real-side.jpg
```

`spanpaper set` writes the new config and sends SIGHUP to the running
daemon, which rolls the workers in place — no restart needed.

### Picking / encoding a source video

The combined span area is **1920 × 2160** (two stacked 1920×1080 panels).
Ideal source is a single MP4 at that resolution. Anything else gets cropped
or letterboxed per the `span_fit` setting in
`~/.config/spanpaper/config.toml` (`crop` = zoom-fill, default; `fit` =
letterbox; `stretch` = ignore aspect).

Re-encode to a hardware-decode-friendly H.264 8-bit yuv420p:

```bash
ffmpeg -i source.mp4 \
  -c:v libx264 -preset slow -crf 20 \
  -pix_fmt yuv420p -movflags +faststart -an \
  -vf "scale=1920:2160:flags=lanczos" \
  ~/Wallpapers/span-1920x2160.mp4
```

## ✅ Done

- Native Wayland output enumeration (HDMI-A-4, DP-5, DP-6 detected with
  correct stacked geometry).
- Two mpvpaper instances locked to the same source MP4 with per-monitor
  vertical crop filters — verified frame-level sync via on-screen
  timestamps.
- swaybg renders the DP-5 static image independently.
- Daemon supervises workers (crash-restart with backoff, SIGHUP hot reload,
  SIGTERM graceful shutdown).
- `setup.sh`, `gen-test-assets.sh`, systemd user unit, autostart desktop
  file.
- v0.2.0 auto-detection: --span / --side accept either image or video.
- vf chain pipelines scale-to-canvas then per-monitor crop, with
  hwdec=auto-copy-safe so libavfilter sees CPU frames and keepaspect=no so
  mpv doesn't pillarbox against the source aspect.
- Persistence wired: config writes atomically on every `spanpaper set`;
  ~/.config/autostart/spanpaper.desktop relaunches the daemon at login
  (required on Budgie; graphical-session.target is inert there). systemd
  --user unit also enabled for sessions that DO activate that target.
- **Active span sync via mpv IPC**: span workers spawn paused with
  `--input-ipc-server=…` and the daemon broadcasts a synchronous unpause
  once every socket is up. Eliminates the visible drift that appeared
  after a `spanpaper set --side <video>` SIGHUP-reload, where adding a
  third mpvpaper to the spawn batch widened per-worker startup variance.
  Verified at 0 ms drift across cold start, reload, side-swap SIGHUP,
  and loop-wrap boundaries.

## 💡 Possible follow-ups (not required)

- **Periodic resync seek**: span workers now sync at every cold start /
  SIGHUP-reload via mpv IPC (see "Span sync" in README). Measured drift is
  0 ms across reloads and loop wraparounds, so periodic resync isn't
  needed for normal use — but multi-hour sessions could still benefit
  from a heartbeat: every N seconds, read worker 0's `time-pos` and
  broadcast `seek $t absolute exact` to the others. Cheap to add (~30
  LoC in `daemon.rs`'s supervisor loop) if drift is ever observed in
  the wild.
- **Auto-detect span groups**: instead of hard-coding `span_outputs`,
  `spanpaper` could scan `wl_output` positions for any two outputs that are
  vertically contiguous (`y2 == y1 + h1`) and span them automatically.
- **Native libmpv render API**: swap the mpvpaper subprocess for an
  in-process EGL + libmpv render context. Drops one process per output
  and ~5 MB of overhead, but a big code investment for marginal gain.
