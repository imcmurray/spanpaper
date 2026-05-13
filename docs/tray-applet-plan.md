# Tray applet + file-manager integration — implementation plan

A panel-resident control surface for spanpaper, plus right-click "Set as
span / side" everywhere XDG mime handlers reach. Replaces the
icon-click-then-file-picker flow with **one-gesture drag-and-drop** onto a
to-scale layout, and lets the user assign wallpaper directly from the file
manager they already have open.

Two independent shippable pieces:

| Piece                       | Tracks the user's hands when…                       |
|----------------------------|-----------------------------------------------------|
| **A. XDG mime "Open With"** | They're already browsing files                      |
| **B. Tray + layout palette**| They want at-a-glance status + drag-and-drop        |

Piece A is ~30 lines of `.desktop` files. Piece B is the new binary. Ship
A first — it's free value and de-risks the daemon contract before we add
GUI surface area.

---

## Goal

The user can:

1. Right-click any image/video in **any XDG-respecting file manager**
   (Nautilus, Nemo, Dolphin, Thunar, Files, PCManFM-Qt) → **"Open With
   → Set as spanpaper span"** or **"… side"**. One click, no terminal.
2. See a small icon in the top panel showing spanpaper's running state.
3. Left-click that icon → a small floating window appears showing the
   user's actual monitor layout, **drawn to scale**, with each output
   rendered as a rectangle containing a thumbnail of its current content.
4. **Drag a file from any file manager** onto one of those rectangles to
   assign it to that slot. Click a rectangle to open a file picker as a
   fallback for keyboard users.
5. Right-click the icon → menu with non-visual actions (pause/resume,
   swap span fit mode, toggle audio, open config dir, restart daemon,
   quit).

No new IPC surface. Everything is expressed as `spanpaper set …` writes
followed by `SIGHUP` — the exact contract `set` already uses. The tray is
a UI veneer on existing primitives.

---

## UX sketch — layout palette

```
   ┌──────────────────────────────────────────────┐
   │   spanpaper                          ⚙  ✕    │
   ├──────────────────────────────────────────────┤
   │  ┌─────────────┐  ┌──────────┐               │
   │  │             │  │          │               │
   │  │   HDMI-A-4  │  │          │               │
   │  │   ┌──────┐  │  │   DP-5   │               │
   │  │   │ THMB │  │  │ ┌──────┐ │               │
   │  │   └──────┘  │  │ │ THMB │ │               │
   │  ├─────────────┤  │ └──────┘ │               │
   │  │             │  │          │               │
   │  │    DP-6     │  │          │               │
   │  │   ┌──────┐  │  │  (side)  │               │
   │  │   │ THMB │  │  │          │               │
   │  │   └──────┘  │  │          │               │
   │  └─────────────┘  └──────────┘               │
   │   spanned pair       independent             │
   │                                              │
   │   span: spring-2160.mp4         🔄  Change  │
   │   side: forest.jpg              🔄  Change  │
   │                                              │
   │   Tip: drop any image/video onto a box.     │
   └──────────────────────────────────────────────┘
```

* Outputs are drawn proportional to their pixel geometry from
  `outputs.rs:1`, preserving stacking. The spanned pair renders as a
  single tall rectangle internally divided by a hairline — visually
  reinforcing that it's *one* wallpaper, not two.
* Each rectangle is a drop target; a label below states what's currently
  loaded with a `Change…` button that opens the file-portal picker.
* Hovering a rectangle highlights it and shows the output name +
  resolution as a tooltip.
* The window is **not** modal — it's a small wlr-layer-shell-less GTK4
  toplevel that auto-closes on focus-out.

---

## Architecture

```
                                    ~/.config/spanpaper/config.toml
                                              ▲
                                              │ atomic write
       ┌────────────────────────┐             │
   ┌──▶│ spanpaper-tray (new)   │─────────────┘
   │   │  GTK4 popover          │
   │   │  ksni StatusNotifier   │──────► SIGHUP ─────┐
   │   └────────────────────────┘                    ▼
   │            ▲                            ┌──────────────┐
   │ left-click │                            │ spanpaper    │
   │            │                            │ daemon       │
   │   ┌────────┴──────┐                     │ (unchanged)  │
   │   │ panel tray    │                     └──────────────┘
   │   └───────────────┘
   │
   │   ┌─────────────────────────────────────────┐
   └──▶│ "Open With" .desktop entries (Piece A)  │
       │  NoDisplay=true, MimeType=image/*;video/* │
       │  Exec=spanpaper set --span %f           │
       └─────────────────────────────────────────┘
```

The tray binary is **a client of the daemon's existing CLI**. It shells
out to `spanpaper set` (same atomic-write + SIGHUP path) and reads
`spanpaper status` / `spanpaper outputs` for state. No socket, no D-Bus
service of our own, no shared library. If the daemon is rewritten, the
tray keeps working as long as the CLI contract holds.

---

## Piece A — XDG "Open With" entries

Ship two `.desktop` files in `contrib/` and install them to
`~/.local/share/applications/` (or `/usr/share/applications/` for the
pacman package).

`contrib/spanpaper-set-span.desktop`:
```ini
[Desktop Entry]
Type=Application
Name=Set as spanpaper span
Comment=Use this file as the spanned wallpaper across stacked monitors
Exec=spanpaper set --span %f
Icon=spanpaper
MimeType=image/jpeg;image/png;image/webp;image/avif;image/heic;image/gif;video/mp4;video/x-matroska;video/webm;
NoDisplay=true
Categories=Graphics;Video;
```

`contrib/spanpaper-set-side.desktop`: identical with `--side` and a
different `Name`.

* `NoDisplay=true` keeps them out of the application menu — they appear
  *only* in "Open With" lists, which is the entire goal.
* `MimeType` lists the exact types `media.rs:1`'s auto-detection accepts
  today. Anything `file(1)`-classified that's not in this list still
  works via terminal but won't show in the right-click menu — acceptable
  because XDG `image/*` and `video/*` wildcards aren't reliably honored
  across file managers.
* `setup.sh` and the PKGBUILDs install these alongside the existing
  autostart entry; no per-FM integration required.

### Per-file-manager polish (optional)

For file managers that don't honor "Open With" for default-action chords
(historically Nemo and Thunar), ship small native snippets:

* **Nemo**: `contrib/fm-integration/spanpaper-span.nemo_action` →
  installs to `~/.local/share/nemo/actions/`
* **Dolphin/KDE**: ServiceMenu `.desktop` in
  `~/.local/share/kio/servicemenus/` (uses `X-KDE-Submenu=spanpaper`)
* **Thunar**: documented snippet for `~/.config/Thunar/uca.xml` (can't
  ship as a file — uca.xml is user-owned and shared with other UCAs)

All four are tiny and all delegate to the same `spanpaper set` CLI, so
they can't drift from the canonical behavior.

---

## Piece B — Tray applet

### Process model

A new long-running user-session binary `spanpaper-tray`, autostarted via
its own XDG entry. Lifecycle is independent of the daemon — the tray
shows a "daemon stopped" state and offers a Start button if the daemon
isn't running. Killing the tray doesn't touch wallpapers.

### Crate layout

Keep the single-crate repo; add a feature-gated second binary so default
`cargo build` stays GTK-free:

```toml
# Cargo.toml additions
[features]
default = []
tray    = ["dep:gtk4", "dep:ksni", "dep:image", "dep:ashpd"]

[dependencies]
# … existing …
gtk4   = { version = "0.9", optional = true, features = ["v4_12"] }
ksni   = { version = "0.3", optional = true }
image  = { version = "0.25", optional = true, default-features = false,
           features = ["jpeg", "png", "webp"] }
ashpd  = { version = "0.10", optional = true, features = ["gtk4"] }

[[bin]]
name              = "spanpaper"
path              = "src/main.rs"

[[bin]]
name              = "spanpaper-tray"
path              = "src/bin/tray/main.rs"
required-features = ["tray"]
```

Source tree gains:

```
src/bin/tray/
├── main.rs              # gtk main, ksni service, glue
├── icon.rs              # tray icon states (idle / playing / paused / error)
├── menu.rs              # right-click menu items → daemon actions
├── palette.rs           # the popover GTK Window
├── layout.rs            # geometry → DrawingArea coordinates
├── thumbnail.rs         # image load + ffmpeg single-frame extract + cache
└── daemon_client.rs     # `spanpaper status` / `set` / SIGHUP shellouts
```

### Key dependencies — rationale

| Crate     | Why this one                                                                 |
|-----------|------------------------------------------------------------------------------|
| `gtk4`    | Native popover, drag-and-drop, file portal integration. GTK3 is EOL.         |
| `ksni`    | SNI is the only cross-DE tray protocol. Works on Budgie, KDE, waybar/sway, GNOME-with-AppIndicator. |
| `ashpd`   | xdg-desktop-portal client for the file picker — works in Flatpak'd FMs too.  |
| `image`   | Decode existing thumbnails. Video frame extraction shells to `ffmpeg`.       |

### Thumbnails

Generated lazily and cached at
`$XDG_CACHE_HOME/spanpaper/thumbs/<sha1-of-abs-path>.png` (256×256 max,
JPEG quality 80, written atomically). Invalidated by mtime check on
cache hit. Videos use `ffmpeg -ss 0.5 -i FILE -frames:v 1 -vf
scale=256:-1` and fall back to a generic film-strip glyph if ffmpeg
fails (we don't require ffmpeg as a hard dep — mpvpaper already
transitively provides it on every install we care about, but the tray
shouldn't crash without it).

### Drag-and-drop

GTK4's `DropTarget` configured for `gio::File` and string URIs. On drop:

```rust
fn on_drop(target: OutputTarget, file: gio::File) -> Result<()> {
    let path = file.path().context("non-local file")?;
    let arg  = match target {
        OutputTarget::Span => "--span",
        OutputTarget::Side => "--side",
    };
    Command::new("spanpaper").args(["set", arg]).arg(path).status()?;
    Ok(())
}
```

The daemon's SIGHUP hot-reload (already wired in `daemon.rs:1`) handles
the visual swap. Tray refreshes its thumbnails by re-reading config on
the next focus-in or after a short debounce.

### Right-click menu

| Item                        | Action                                                  |
|-----------------------------|---------------------------------------------------------|
| Start / Stop daemon         | `spanpaper start --background` / `spanpaper stop`       |
| Pause / Resume playback     | mpv IPC pause (needs daemon support — see Open Questions) |
| Span fit: crop / fit / stretch | `spanpaper set --span-fit X`                         |
| Audio: on / off             | `spanpaper set --audio on|off`                          |
| Open config folder          | `xdg-open ~/.config/spanpaper`                          |
| Reload (re-read config)     | `pkill -HUP spanpaper`                                  |
| Quit tray                   | exit                                                    |

Items disable themselves when the daemon isn't running.

### Icon states

Single `spanpaper` icon name with three variants installed at standard
icon-theme paths (`scalable/apps/`, `symbolic/apps/`): `spanpaper`,
`spanpaper-paused`, `spanpaper-error`. ksni selects by state.

---

## Distribution

* `setup.sh` gains a `--with-tray` flag (default off for backward
  compatibility); when set, it `cargo build --release --features tray`
  and installs both binaries plus the new autostart entry.
* `contrib/PKGBUILD` splits into two pacman packages eventually:
  `spanpaper` (daemon only, the current artifact) and `spanpaper-tray`
  (depends on `spanpaper`, gtk4, libayatana-appindicator). For the first
  release, ship as a single package with both binaries and call the tray
  optional via `--features tray` at build time.
* The two `.desktop` files for "Open With" install unconditionally —
  they're tiny and useful even without the tray binary.

---

## Milestones

Each milestone is independently shippable and user-testable. None of
them is "do everything else first."

1. **M1 — Piece A only.** Two `.desktop` files + install lines in
   `setup.sh` and the PKGBUILDs. End-to-end test: right-click an image
   in Nautilus on Budgie, "Open With → Set as spanpaper span", verify
   hot-reload. **No new Rust code. ~2 hours.** ✅ Done.
   * Gotcha discovered during M1: declaring `MimeType=image/jpeg;…` on
     a `.desktop` file makes it default-eligible. If the user has no
     explicit default for a type, gio's fallback picks the
     first-registered match — and ours often wins alphabetically. A
     naive install silently turned every JPEG double-click into a
     wallpaper change. The fix: `setup.sh` snapshots
     `xdg-mime query default` for every claimed MIME type BEFORE
     installing, then re-pins those defaults AFTER. The pacman packages
     install to `/usr/share/applications/` where the user's
     `mimeapps.list` (if any) takes precedence — but fresh-user systems
     should be documented to run `xdg-mime default <app> <type>` if
     they want a different default than what `gio` happens to fall
     back to.
2. **M2 — Tray walking skeleton.** New binary behind `tray` feature,
   ksni icon in the panel, right-click menu with Start/Stop/Quit only,
   no popover. Proves cross-DE icon rendering on Budgie + at least one
   other DE before we spend time on the popover. **~1 day.** ✅ Done.
   * Cargo gained `autobins = false` and explicit `[[bin]]` entries
     for both `spanpaper` and `spanpaper-tray`; default builds remain
     ksni/tokio-free, `cargo build --features tray` produces the
     second binary.
   * Sources under `src/bin/spanpaper-tray/` — `main.rs` (tokio
     `current_thread` runtime, ksni `TrayService`, 2 s pid-file poll
     for live menu state) and `daemon_client.rs` (pid-file +
     `kill(pid, 0)` liveness, shellout to the existing CLI for
     actions).
   * Side-fix in `daemon::run`: replaced the `isatty(stdin)` guard
     around `spawn_background()` with a `SPANPAPER_DAEMONIZED` env-var
     sentinel. The TTY guard had left non-TTY callers (tray, XDG
     autostart) running the supervisor in-process; removing it
     naively caused an infinite re-exec loop. The env-var sentinel
     does what the TTY check was really there for: tell the child
     "you are the daemon, don't re-exec." Works for every caller now.
   * M2 limitation worth knowing: menu activate callbacks are sync
     (ksni's design), and `spanpaper stop` blocks up to 5 s — so
     clicking Stop freezes the tray menu for that long. M3+ moves
     blocking actions onto a tokio task and the menu stays
     responsive.
3. **M3 — Popover with static layout.** GTK4 window opens on left-click,
   shows monitor rectangles to scale with placeholder thumbnails. No DnD
   yet, no file picker. **~1 day.** ✅ Done.
   * `gtk4 = "0.9"` and `async-channel = "2"` added under the `tray`
     feature so default daemon builds remain GTK-free.
   * Threading: GTK4 main loop on the main thread (required by GTK);
     tokio current-thread runtime + ksni service on a worker thread;
     `async_channel<UiMsg>` bridges them. ksni's `activate(x, y)`
     left-click handler does a non-blocking `try_send`; a
     `glib::spawn_future_local` task on the GTK side receives and
     calls `palette::show`. Right-click menu gained an **Open
     palette** item that uses the same channel so the right-click
     path works on DEs whose panels deliver left-click as a menu
     trigger.
   * `outputs_query::list` shells out to `spanpaper outputs` and
     parses the tab-separated rows — keeps the tray a pure CLI
     client of the daemon, no module imports across binaries.
   * `palette.rs` builds the popover: top-level horizontal GtkBox
     containing the span group (GtkBox oriented per
     `span_direction`) and the standalone side, **ordered left-to-
     right by each output's actual x-coordinate on the desktop** so
     "what you see is what you set" holds even when the side
     monitor sits to the left of the span pair. Each output renders
     as a GtkFrame sized proportionally to its real pixel
     dimensions (`MAX_EDGE_PX = 220`), with the output name,
     resolution, and current filename basename inside. A
     placeholder `Change…` button is shown disabled with a tooltip
     pointing to M6 (file-picker fallback).
   * `app.hold()` returns a guard that must be kept alive —
     `mem::forget` is the documented pattern for "hold for the
     whole app lifetime so closing the palette window doesn't
     terminate the tray."
   * One unit test covers the `spanpaper outputs` parser against
     the known three-monitor layout (HDMI-A-4 / DP-5 / DP-6).
4. **M4 — Real thumbnails.** Reads current config, renders cached image
   thumbnails per slot. ffmpeg video-frame extraction with graceful
   fallback. **~½ day.** ✅ Done.
   * New `src/bin/spanpaper-tray/thumbnail.rs`. One code path handles
     both stills and videos: `ffmpeg -ss 0.5 -i FILE -frames:v 1 -vf
     scale=256:-1:flags=lanczos -c:v png -f image2 OUT`. The 0.5 s
     pre-roll seek skips fade-in black frames on clips; for stills
     it's a no-op.
   * Cache layout: `$XDG_CACHE_HOME/spanpaper/thumbs/<hash>.png`,
     where `<hash>` is a stable `DefaultHasher` of the canonicalised
     absolute path. Atomic write via temp-then-rename. Mtime-based
     invalidation when the source is newer than the cache.
   * `palette::build_output_frame` swaps the resolution label for a
     `gtk4::Picture::for_filename` with `ContentFit::Cover` so the
     thumbnail fills the frame while preserving aspect; portrait
     frames (the side output) crop the sides of a landscape source.
     Falls back to the M3 resolution-text rendering when ffmpeg is
     absent or fails — the popover must never blank because of
     thumbnail trouble.
   * `gtk4` feature set bumped to `["v4_8"]` for `Picture::content_fit`
     (available since GTK 4.8; system GTK is 4.22+).
   * Known cost: thumbnail generation runs synchronously inside the
     GTK activate handler, so first-open of the palette blocks for
     ~1 s per uncached source. Subsequent opens are instant. M7
     polish can move generation onto a `glib::spawn_future_local`
     task and render a spinner placeholder in the meantime.
5. **M5 — Drop targets.** Drag-and-drop wired to `spanpaper set`. The
   moment this lands, the feature is the killer feature. **~½ day.** ✅ Done.
   * `daemon_client::Slot { Span, Side }` + `set_for(slot, path)`
     shells out to `spanpaper set --span PATH` / `--side PATH`.
   * Each output Frame in the palette gets a `gtk4::DropTarget`
     accepting `gio::File`. Drop pulls `.path()` (local files only —
     non-local Flatpak crossings decline cleanly; M6's portal picker
     covers those), calls `daemon_client::set_for`, then closes the
     window so the next left-click brings up a fresh palette with the
     new thumbnail. End-to-end SIGHUP-reload latency on a drop is
     ~10 ms.
   * Polish discovered during user testing:
     * **Single-instance palette.** Repeatedly left-clicking the tray
       icon was opening duplicate windows. Now the palette window is
       tagged `widget_name = "spanpaper-palette"`; `show()` iterates
       `Application::windows()` and `present()`s the existing one if
       found, only building a new window when none exists.
     * **Still-image thumbnails were blank.** The M4 `-ss 0.5`
       pre-roll seek pushes past EOF on a single-frame PNG/JPG,
       ffmpeg exits zero with no file written, the cache rename then
       fails and the popover falls back to resolution text. Fix:
       drop the seek (the fade-in skip wasn't worth the still-image
       breakage), and add `-update 1` so the image2 muxer stops
       warning about the missing sequence pattern.
6. **M6 — File picker fallback + full menu.** `ashpd` file picker for
   click-to-open, full right-click menu with pause/fit/audio/etc.
   **~1 day.** ✅ Done.
   * **File picker**: ended up using `gtk4::FileDialog` (GTK 4.10+,
     `v4_10` feature flag) instead of `ashpd`. GTK4's FileDialog
     wraps `xdg-desktop-portal` automatically on Wayland and works
     natively with `glib::spawn_future_local` — no extra dep, no
     tokio bridge required. MIME filter populated from the same
     list as `contrib/spanpaper-set-*.desktop`. The previously
     disabled "Change…" buttons on the summary rows are now live;
     flow is identical to drag-and-drop (set, then
     `populate(window)` in place).
   * **Right-click menu (full)**: Open palette, Pause/Resume, Span
     fit submenu (crop/fit/stretch), Side mode submenu (fill/fit/
     stretch/center/tile), Audio submenu, Open config folder,
     Reload config, Start/Stop daemon, Quit tray. Per-submenu
     factory helpers keep `menu()` readable.
   * **Side mode separate from Span fit**: user testing surfaced
     that the swaybg `side_mode` knob is what side images need,
     distinct from `span_fit`. Now independent menu groups. Side
     videos still use `span_fit` — pre-existing daemon quirk,
     documented in `daemon_client::set_span_fit`.
   * **Pause/resume via mpv IPC**: M2's startup-sync already wired
     sockets for span workers; M6 added them for side video too in
     `workers.rs::plan`. The tray enumerates
     `$XDG_RUNTIME_DIR/spanpaper/mpv-*.sock` and broadcasts
     `set_property pause`. Tray-side `paused: bool` flips the menu
     label between Pause and Resume; it resets on daemon-down
     because a fresh daemon always boots unpaused. The daemon's
     sync-unpause now accepts solo-socket configs (relaxed the
     old `< 2 sockets returns early` check that would have left a
     single-IPC side paused forever).
   * **Open config folder via `org.freedesktop.FileManager1`**:
     `xdg-open` was honouring a hijacked `inode/directory` MIME
     default and opening "etag" instead of a file manager. The
     tray tries the freedesktop FileManager1 D-Bus interface
     (`ShowFolders`) first — every real file manager implements
     it — and only falls back to `xdg-open` if no service answers.
   * **Stop→Start sequencing**: was shelling out to `spanpaper
     stop` which spin-waits up to 5 s; that blocked the ksni
     service loop and froze the menu. Replaced with direct
     `kill(pid, SIGTERM)` + a 5 s bounded wait for the pid file
     to disappear before returning. `start_daemon` also waits up
     to 5 s for any lingering shutdown to complete before
     spawning, belt-and-braces, so the next spanpaper start
     never sees a stale pid file.
   * **Menu refresh on every state flip**: Budgie's tray applet
     doesn't refresh items reliably on dbusmenu's
     `ItemsPropertiesUpdated` signal — only on `LayoutUpdated`.
     ksni only fires LayoutUpdated when the menu's *structure*
     (child IDs) differs, not when only enabled flags flip. So
     "show both Start and Stop, grey out the wrong one" left
     Budgie caching the cold-start layout and silently swallowing
     clicks on items it thought were disabled. Fix: conditionally
     INCLUDE/EXCLUDE daemon-dependent items based on `running`
     rather than always emitting them with `enabled: running`.
     The shrink-grow on every state transition is enough of a
     structural diff that LayoutUpdated fires and Budgie
     re-renders. Bonus UX win: no greyed-out useless menu items
     when the daemon is stopped.
7. **M7 — Polish.** Icon states (playing/paused/error), focus-out
   auto-close, accessibility labels, autostart `.desktop`,
   documentation pass. **~½ day.** ✅ Done.
   * In-place palette refresh after drop landed earlier as its own
     commit; the rest landed together as the M7 polish commit.
   * **Icon states.** `Tray::icon_name` now returns one of three
     fdo icons based on `daemon_running × paused`:
     `preferences-desktop-wallpaper` (running), `media-playback-pause`
     (paused), `media-playback-stop` (stopped). Verified live via
     dbus property reads.
   * **Focus-out auto-close.** Palette window connects
     `is_active_notify`; when the user clicks outside the window,
     it closes. A shared `Rc<PaletteState>` carries a
     `suppress_autoclose` flag the file-picker code flips while the
     modal `FileDialog` is up — without that we'd close the dialog's
     own parent mid-pick.
   * **Async thumbnails.** `thumbnail::ensure` now runs through
     `gio::spawn_blocking`, which runs ffmpeg on glib's worker thread
     pool and resolves on the GTK main loop. Each frame paints a
     `gtk4::Spinner` immediately and swaps in the picture (or a
     resolution-text fallback) when ffmpeg returns. First-open of
     the palette no longer blocks on ffmpeg.
   * **Accessibility / tooltips.** Each output frame's tooltip
     names the output, resolution, current file, and the drop role.
     "Change…" buttons have tooltips explaining the picker action.
     Bonus: removed a small duplication where the basename was
     formatted twice.
   * **Tray autostart.** New `contrib/spanpaper-tray.desktop`
     (`@SPANPAPER_TRAY_BIN@` placeholder, mirrors the existing
     `spanpaper.desktop`). `setup.sh` gained `--with-tray`: builds
     with `--features tray`, installs `spanpaper-tray` to
     `~/.local/bin`, and writes the autostart entry to
     `~/.config/autostart/`.
   * **Default size + resizable.** Palette window now opens 540×540
     and resizable (was 480 wide, no height, fixed). Without an
     explicit height the async-thumbnail spinners gave too small a
     natural size and content was clipped on first paint.
   * **Layer-shell anchoring near the tray icon.** With a plain
     `gtk4::ApplicationWindow`, Wayland compositors place the
     popover wherever they see fit — sometimes bottom-left, often
     far from the panel icon. spanpaper already requires
     `wlr-layer-shell` for the daemon's wallpapers, so the tray
     now also uses it (via `gtk4-layer-shell = "0.5"` paired with
     `gtk4 = "0.9"`): on `Tray::activate(x, y)` the click
     coordinates ride along on `UiMsg::ShowPalette { x, y }`,
     and `palette::show` initialises the window as a layer-shell
     surface anchored to top-left with margins (x+4, y+4). Menu-
     item activation sends `(-1, -1)` and skips the anchor, letting
     the compositor pick — for any caller without click coords.
   * **README pass.** New "Tray applet (optional)" section
     explaining the binary, install via `setup.sh --with-tray`,
     autostart story, GNOME AppIndicator caveat. Layout section
     updated to list `src/bin/spanpaper-tray/` and the new
     `contrib/spanpaper-tray.desktop`.

Total: ~5 working days for a complete, polished result. M1 alone (~2
hours) delivers most of the day-to-day workflow improvement; everything
after is the visual layer.

---

## Trade-offs and open questions

* **GNOME / Mutter requires AppIndicator extension** for the tray icon
  to appear. Acceptable because spanpaper's daemon already doesn't work
  on plain Mutter (no wlr-layer-shell). Document in README.
* **Pause/resume requires daemon work**, not just tray work. mpvpaper
  doesn't currently expose mpv's IPC socket externally. Either
  (a) drop pause from M6 and add it as a follow-up that wires mpv's
  `--input-ipc-server`, or (b) implement pause as "stop the workers,
  remember the state, restart on resume." Recommend (a) — pause matters
  most for video and IPC is the right long-term answer.
* **Flatpak'd file managers + drag-and-drop**: GTK4 Wayland DnD across
  sandbox boundaries occasionally drops URIs. The file-picker fallback
  (M6) covers this; not a blocker.
* **Multi-user / multi-display-config**: tray reads `spanpaper outputs`
  on every popover open so a monitor hot-plug doesn't show stale
  geometry. No caching beyond the in-popover lifetime.
* **What if the user drops a non-media file?** Use `media.rs:1`'s
  existing detection — show an inline error toast in the popover, don't
  call `spanpaper set`. The CLI already rejects unknowns so this is
  belt-and-braces.

---

## Out of scope (do not build now)

* Per-output independent wallpapers beyond the existing span/side
  abstraction. The popover shows three boxes because that's what the
  daemon supports; growing the daemon is a separate effort.
* Built-in wallpaper library / browser. The OS file manager is better
  at this than any UI we'd build.
* Scheduled wallpaper rotation. Cron + `spanpaper set` already does
  this and doesn't need GUI.
* In-popover video preview (animated thumbs). Static frame is plenty;
  animated preview would burn battery for negligible gain.
* Cross-DE applet bindings (Budgie Vala applet, KDE Plasmoid). The
  SNI icon already gives us those DEs for free with one codebase.

---

## File-level change summary

```
New:
  docs/tray-applet-plan.md                 (this file)
  contrib/spanpaper-set-span.desktop
  contrib/spanpaper-set-side.desktop
  contrib/spanpaper-tray.desktop           (autostart for the applet, M2+)
  contrib/fm-integration/                  (optional per-FM snippets)
    spanpaper-span.nemo_action
    spanpaper-side.nemo_action
    spanpaper-span.kio-servicemenu.desktop
    spanpaper-side.kio-servicemenu.desktop
    thunar-uca-snippet.xml
  src/bin/tray/main.rs                     (M2+)
  src/bin/tray/{icon,menu,palette,layout,thumbnail,daemon_client}.rs

Modified:
  Cargo.toml                  + [features] tray, + [[bin]] spanpaper-tray
  setup.sh                    + --with-tray flag, installs .desktop files
  contrib/PKGBUILD            installs the two "Open With" .desktops
  contrib/PKGBUILD-bin        same
  README.md                   short section: "Right-click in your file manager"
  TODO.md                     mark milestones as they land

Unchanged:
  src/daemon.rs, src/workers.rs, src/config.rs, src/outputs.rs,
  src/media.rs, src/cli.rs
  (the daemon stays a pure CLI; tray is a client of it)
```
