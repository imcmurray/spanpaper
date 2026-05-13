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
   other DE before we spend time on the popover. **~1 day.**
3. **M3 — Popover with static layout.** GTK4 window opens on left-click,
   shows monitor rectangles to scale with placeholder thumbnails. No DnD
   yet, no file picker. **~1 day.**
4. **M4 — Real thumbnails.** Reads current config, renders cached image
   thumbnails per slot. ffmpeg video-frame extraction with graceful
   fallback. **~½ day.**
5. **M5 — Drop targets.** Drag-and-drop wired to `spanpaper set`. The
   moment this lands, the feature is the killer feature. **~½ day.**
6. **M6 — File picker fallback + full menu.** `ashpd` file picker for
   click-to-open, full right-click menu with pause/fit/audio/etc.
   **~1 day.**
7. **M7 — Polish.** Icon states (playing/paused/error), focus-out
   auto-close, accessibility labels, autostart `.desktop`,
   documentation pass. **~½ day.**

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
