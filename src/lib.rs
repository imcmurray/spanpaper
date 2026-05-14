//! Shared library for the spanpaper daemon and tray applet.
//!
//! Both binaries (`spanpaper`, `spanpaper-tray`) depend on this crate
//! for the modules below. The daemon also has its own server-side
//! modules (`daemon`, `workers`) that live alongside `src/main.rs` and
//! are not exposed here — the tray has no business spawning workers.
//!
//! The split landed in v0.4.0 to kill the duplication that crept in
//! during the M2–M7 tray work, where the tray binary reimplemented
//! its own pid-file probe, mpv IPC client, and partial Config struct.

pub mod config;
pub mod ipc;
pub mod media;
pub mod outputs;
pub mod state;
