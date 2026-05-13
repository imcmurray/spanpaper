# spanpaper — outstanding work

## ✅ Shipped in v0.3.1

- **`spanpaper install [--method=xdg|systemd|both] [--start]`** —
  one-command autostart wiring. Generates the autostart entries
  inline from `current_exe()` (works for both pacman installs at
  `/usr/bin` and source installs at `~/.local/bin`). `--method=xdg`
  writes `~/.config/autostart/spanpaper.desktop`; `--method=systemd`
  writes `~/.config/systemd/user/spanpaper.service` and runs
  `systemctl --user enable`; `--method=both` does both. `--start`
  additionally launches the daemon and tray now, gated on existence
  checks so re-runs don't spawn duplicates. The tray always uses XDG
  regardless of method.
- **`release.sh` pre-seeds the local tarball into makepkg's working
  dir** — the v0.3.0 release blew up at the makepkg step because
  `PKGBUILD-bin`'s `source=` URL pointed at the GitHub release asset
  that the *next* script step uploads (chicken-and-egg). Fix is one
  cp; v0.3.0 was finished by hand, v0.3.1+ run end-to-end.

## ✅ Shipped in v0.3.0

- **Native Wayland output enumeration** (wl_output + xdg-output;
  `spanpaper outputs` parses correctly across hot-plug).
- **Span vf chain**: scale-to-canvas + per-monitor crop, with
  `hwdec=auto-copy-safe` so libavfilter sees CPU frames and
  `keepaspect=no` so mpv doesn't second-guess the chain.
- **Daemon supervision**: crash-restart with linear backoff, SIGHUP
  hot reload, SIGTERM graceful shutdown.
- **Active span sync via mpv IPC**: span workers spawn paused with
  `--input-ipc-server=…`; the daemon broadcasts a synchronous
  unpause once every socket is up. 0 ms drift verified across cold
  start, SIGHUP-reload (incl. side-swap with a third mpv in the
  spawn batch), and loop-wrap boundaries. Side video also gets a
  socket so the tray can pause it alongside.
- **`spanpaper set` hardening**: rejects paths containing newlines /
  NUL bytes before writing config; warns (doesn't fail) when the
  file doesn't yet exist so configure-before-place still works.
- **`side_fit` independent of `span_fit`** — same three values
  (crop / fit / stretch), applied separately to side image (swaybg
  mode) and side video (mpv panscan/keepaspect). `side_mode`
  removed.
- **Right-click "Open With → Set as spanpaper span / side"** —
  MimeType-only `.desktop` entries shipped in
  `/usr/share/applications/`, `setup.sh` snapshots and re-pins
  existing MIME defaults so installing the entries never displaces
  the user's image viewer.
- **Tray applet** (`spanpaper-tray`, GTK4 + ksni + gtk-layer-shell):
  - Panel icon with state-driven glyph (playing / paused / stopped)
  - Left-click anchors a popover near the icon via wlr-layer-shell
  - To-scale monitor rectangles with cached ffmpeg thumbnails
    (generated off the GTK thread; spinners while loading)
  - Drag-and-drop file assignment from any file manager
  - Portal-backed `gtk4::FileDialog` behind each rectangle's
    "Change…" button
  - Right-click menu: Pause/Resume, Span fit, Side fit, Audio,
    Open config folder (via `FileManager1` D-Bus, not `xdg-open`),
    Reload config, Start/Stop daemon, Quit
  - In-place refresh after drop (no close-and-reopen)
  - Focus-out auto-close (with suppression while the file picker
    is open)
  - Single-instance — clicking the tray icon while the palette is
    open just raises it
- **Packaging**: 0.3.0 ships both binaries + both autostart samples
  + the "Open With" entries in one pacman package
  (`spanpaper-bin-0.3.0-1-x86_64.pkg.tar.zst`). `setup.sh --with-tray`
  is the equivalent for source installs.

## 💡 Possible follow-ups (not required)

- **Periodic resync seek**: span IPC sync at every reload covers
  every case observed so far, but a multi-hour session could
  drift sub-frame. A heartbeat (`time-pos` read from worker 0 →
  `seek absolute exact` broadcast to others, every N seconds)
  bounds drift to a single frame even under extreme conditions.
  ~30 LoC in `daemon.rs`'s supervisor loop.
- **Auto-detect span groups**: scan `wl_output` positions for any
  two outputs that are vertically contiguous (`y2 == y1 + h1`) and
  span them automatically, instead of requiring `span_outputs` in
  config. Useful for portable / multi-rig users.
- **Native libmpv render API**: swap the mpvpaper subprocess for an
  in-process EGL + libmpv render context. Drops one process per
  output and ~5 MB of overhead per worker, but a big code
  investment for marginal gain.
- **Split pacman package**: `spanpaper` (daemon-only, no GTK deps)
  + `spanpaper-tray` (depends on `spanpaper`, adds GTK4 /
  gtk4-layer-shell). Today's single-package install pulls ~100 MB
  of GTK runtime even for users who only want the daemon. Skip
  unless that becomes a real complaint.
- **GNOME-Shell tray support**: documenting the AppIndicator
  extension users need is fine for now; investigating a native
  GNOME extension is out of scope.
