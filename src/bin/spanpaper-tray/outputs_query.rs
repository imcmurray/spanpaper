//! Query the daemon for the current Wayland output topology.
//!
//! Shells out to `spanpaper outputs` and parses its tab-separated lines:
//!
//!     HDMI-A-4    1920x1080    +2870+750    scale=1
//!     DP-5        1080x1920    +1790+990    scale=1
//!     DP-6        1920x1080    +2870+1830   scale=1
//!
//! This is the same data `crate::outputs::detect()` produces inside the
//! daemon, accessed via the public CLI contract so the tray stays a
//! pure CLI client.

use anyhow::{Context, Result};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct OutputInfo {
    pub name: String,
    pub width: i32,
    pub height: i32,
    pub x: i32,
    #[allow(dead_code)] // M3 only uses x for sort order; full positional layout in a later milestone.
    pub y: i32,
    #[allow(dead_code)] // Surfaced in M4+ when thumbnails care about hidpi.
    pub scale: i32,
}

pub fn list() -> Result<Vec<OutputInfo>> {
    let out = Command::new("spanpaper")
        .arg("outputs")
        .output()
        .context("spawn `spanpaper outputs`")?;
    if !out.status.success() {
        anyhow::bail!(
            "`spanpaper outputs` exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    parse(&String::from_utf8_lossy(&out.stdout))
}

fn parse(stdout: &str) -> Result<Vec<OutputInfo>> {
    let mut v = Vec::new();
    for (i, line) in stdout.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        v.push(parse_line(line).with_context(|| format!("line {}: {line:?}", i + 1))?);
    }
    Ok(v)
}

fn parse_line(line: &str) -> Result<OutputInfo> {
    // Tab-separated: name, WxH, +X+Y, scale=N
    let mut parts = line.split('\t');
    let name = parts.next().context("missing name")?.trim().to_string();
    let res = parts.next().context("missing resolution")?.trim();
    let pos = parts.next().context("missing position")?.trim();
    let scale_kv = parts.next().context("missing scale")?.trim();

    let (w, h) = res.split_once('x').context("resolution missing 'x'")?;
    let width: i32 = w.parse().context("width")?;
    let height: i32 = h.parse().context("height")?;

    let pos = pos
        .strip_prefix('+')
        .context("position missing leading '+'")?;
    let (xs, ys) = pos.split_once('+').context("position missing second '+'")?;
    let x: i32 = xs.parse().context("x")?;
    let y: i32 = ys.parse().context("y")?;

    let scale: i32 = scale_kv
        .strip_prefix("scale=")
        .context("scale missing 'scale=' prefix")?
        .parse()
        .context("scale")?;

    Ok(OutputInfo { name, width, height, x, y, scale })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_three_output_layout() {
        let sample = "\
HDMI-A-4\t1920x1080\t+2870+750\tscale=1
DP-5\t1080x1920\t+1790+990\tscale=1
DP-6\t1920x1080\t+2870+1830\tscale=1
";
        let outs = parse(sample).expect("parse ok");
        assert_eq!(outs.len(), 3);
        assert_eq!(outs[0].name, "HDMI-A-4");
        assert_eq!((outs[0].width, outs[0].height), (1920, 1080));
        assert_eq!((outs[0].x, outs[0].y), (2870, 750));
        assert_eq!(outs[0].scale, 1);
        assert_eq!(outs[1].name, "DP-5");
        assert_eq!((outs[1].width, outs[1].height), (1080, 1920));
    }
}
