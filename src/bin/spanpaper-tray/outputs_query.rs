//! Output topology for the layout palette.
//!
//! Used to shell out to `spanpaper outputs` and parse the tab-separated
//! lines. Since v0.4.0's lib split, the tray uses
//! `spanpaper::outputs::detect` directly — same Wayland enumeration
//! the daemon does, no subprocess.

use anyhow::Result;
use spanpaper::outputs::{detect, Output};

pub type OutputInfo = Output;

pub fn list() -> Result<Vec<OutputInfo>> {
    detect()
}
