// Native Wayland output enumeration.
//
// We bind to the compositor's wl_registry, advertise interest in wl_output and
// zxdg_output_manager_v1, then do a single roundtrip to collect everything.
// xdg-output gives us the human-readable connector name (e.g. "HDMI-A-4");
// wl_output gives us position, mode (resolution), and scale.
//
// We don't keep this connection open — `detect()` is a one-shot snapshot.
// The daemon uses it at startup and on SIGHUP/reload to verify outputs exist.

use anyhow::{Context, Result};
use std::collections::HashMap;
use wayland_client::{
    globals::{registry_queue_init, GlobalList, GlobalListContents},
    protocol::{wl_output, wl_registry},
    Connection, Dispatch, Proxy, QueueHandle,
};
use wayland_protocols::xdg::xdg_output::zv1::client::{
    zxdg_output_manager_v1::ZxdgOutputManagerV1,
    zxdg_output_v1::{self, ZxdgOutputV1},
};

#[derive(Debug, Clone)]
pub struct Output {
    pub name: String,       // e.g. "HDMI-A-4"
    #[allow(dead_code)]     // reserved for richer diagnostics
    pub description: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub scale: i32,
}

pub fn detect() -> Result<Vec<Output>> {
    let conn = Connection::connect_to_env()
        .context("connect to Wayland (is WAYLAND_DISPLAY set?)")?;
    let (globals, mut queue) =
        registry_queue_init::<State>(&conn).context("init wl_registry")?;
    let qh = queue.handle();

    let mut state = State::new(&globals, &qh)?;

    // Two roundtrips: one to instantiate xdg-outputs against each wl_output,
    // a second to flush all the per-output events (name/logical_position/size).
    queue.roundtrip(&mut state).context("wayland roundtrip 1")?;
    queue.roundtrip(&mut state).context("wayland roundtrip 2")?;

    let mut out: Vec<Output> = state.outputs.into_values().collect();
    out.sort_by(|a, b| (a.y, a.x, a.name.clone()).cmp(&(b.y, b.x, b.name.clone())));
    Ok(out)
}

#[derive(Default)]
struct PendingOutput {
    name: Option<String>,
    description: Option<String>,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    scale: i32,
}

impl PendingOutput {
    fn build(self) -> Option<Output> {
        Some(Output {
            name: self.name?,
            description: self.description.unwrap_or_default(),
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
            scale: if self.scale == 0 { 1 } else { self.scale },
        })
    }
}

struct State {
    xdg_mgr: ZxdgOutputManagerV1,
    /// keyed by the protocol id of the underlying wl_output, so xdg + wl events
    /// dispatched separately can write into the same bucket.
    pending: HashMap<u32, PendingOutput>,
    /// finalized snapshot built from pending after roundtrips
    outputs: HashMap<u32, Output>,
}

impl State {
    fn new(globals: &GlobalList, qh: &QueueHandle<Self>) -> Result<Self> {
        let xdg_mgr: ZxdgOutputManagerV1 = globals
            .bind(qh, 2..=3, ())
            .context("compositor lacks zxdg_output_manager_v1 (need wlroots-based compositor)")?;

        let mut state = State {
            xdg_mgr,
            pending: HashMap::new(),
            outputs: HashMap::new(),
        };

        for g in globals.contents().clone_list() {
            if g.interface == "wl_output" {
                // Bind the wl_output and immediately request an xdg_output for it.
                let wl_out: wl_output::WlOutput = globals
                    .registry()
                    .bind(g.name, g.version.min(4), qh, ());
                let id = wl_out.id().protocol_id();
                state.pending.entry(id).or_default();
                let _xdg: ZxdgOutputV1 = state.xdg_mgr.get_xdg_output(&wl_out, qh, id);
                // We keep the wl_output around long enough for events; dropping it
                // here is fine because Wayland keeps the resource alive until the
                // queue is destroyed.
            }
        }
        Ok(state)
    }
}

// We don't care about new globals after startup — this snapshot is one-shot.
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {}
}

impl Dispatch<wl_output::WlOutput, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &wl_output::WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let id = proxy.id().protocol_id();
        let entry = state.pending.entry(id).or_default();
        match event {
            wl_output::Event::Geometry { x, y, .. } => {
                entry.x = x;
                entry.y = y;
            }
            wl_output::Event::Mode { flags, width, height, .. } => {
                if flags
                    .into_result()
                    .map(|f| f.contains(wl_output::Mode::Current))
                    .unwrap_or(false)
                {
                    entry.width = width;
                    entry.height = height;
                }
            }
            wl_output::Event::Scale { factor } => entry.scale = factor,
            wl_output::Event::Name { name } => {
                // wl_output v4 also carries the connector name. Prefer xdg_output's
                // name if both arrive (they should match), but populate as fallback.
                if entry.name.is_none() {
                    entry.name = Some(name);
                }
            }
            wl_output::Event::Description { description } => {
                entry.description = Some(description);
            }
            wl_output::Event::Done => {
                if let Some(o) = std::mem::take(entry).build() {
                    state.outputs.insert(id, o);
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<ZxdgOutputV1, u32> for State {
    fn event(
        state: &mut Self,
        _: &ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        wl_out_id: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let entry = state.pending.entry(*wl_out_id).or_default();
        match event {
            zxdg_output_v1::Event::LogicalPosition { x, y } => {
                entry.x = x;
                entry.y = y;
            }
            zxdg_output_v1::Event::LogicalSize { width, height } => {
                entry.width = width;
                entry.height = height;
            }
            zxdg_output_v1::Event::Name { name } => {
                entry.name = Some(name);
            }
            zxdg_output_v1::Event::Description { description } => {
                entry.description = Some(description);
            }
            zxdg_output_v1::Event::Done => {
                // Deprecated in v3 (wl_output::Done is preferred), but flush anyway.
                if let Some(o) = std::mem::take(entry).build() {
                    state.outputs.insert(*wl_out_id, o);
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<ZxdgOutputManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZxdgOutputManagerV1,
        _: <ZxdgOutputManagerV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {}
}
