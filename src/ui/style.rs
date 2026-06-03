//! Shared presentation helpers: ANSI styling (tty-gated), bar gauges, byte/number
//! humanizing. No `ratatui`, no color crates — just bytes we emit ourselves.

use std::io::IsTerminal;

/// ANSI palette, blanked when stdout is not a terminal (so pipes stay clean).
#[derive(Clone, Copy)]
pub struct Style {
    pub bold: &'static str,
    pub dim: &'static str,
    pub red: &'static str,
    pub green: &'static str,
    pub yellow: &'static str,
    pub blue: &'static str,
    pub reset: &'static str,
}

impl Style {
    pub fn detect() -> Self {
        if std::io::stdout().is_terminal() {
            Style::color()
        } else {
            Style::plain()
        }
    }
    pub const fn color() -> Self {
        Style {
            bold: "\x1b[1m",
            dim: "\x1b[2m",
            red: "\x1b[31m",
            green: "\x1b[32m",
            yellow: "\x1b[33m",
            blue: "\x1b[34m",
            reset: "\x1b[0m",
        }
    }
    pub const fn plain() -> Self {
        Style {
            bold: "",
            dim: "",
            red: "",
            green: "",
            yellow: "",
            blue: "",
            reset: "",
        }
    }
}

/// A filled/empty block bar of `width` cells for `v` in `[lo, hi]`.
pub fn bar(v: f64, lo: f64, hi: f64, width: usize) -> String {
    let span = (hi - lo).max(f64::MIN_POSITIVE);
    let frac = ((v - lo) / span).clamp(0.0, 1.0);
    let filled = (frac * width as f64).round() as usize;
    let mut s = String::with_capacity(width * 3);
    for _ in 0..filled {
        s.push('█');
    }
    for _ in filled..width {
        s.push('░');
    }
    s
}

const SPARK: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Unicode sparkline of `vals` scaled to `[lo, hi]`.
pub fn sparkline(vals: &[f64], lo: f64, hi: f64) -> String {
    let span = (hi - lo).max(f64::MIN_POSITIVE);
    vals.iter()
        .map(|&v| {
            let idx = (((v - lo) / span) * 7.0).round().clamp(0.0, 7.0) as usize;
            SPARK[idx]
        })
        .collect()
}

/// Humanize a byte count as GiB/MiB with one decimal.
pub fn human_bytes(b: u64) -> String {
    let gib = 1024.0 * 1024.0 * 1024.0;
    let mib = 1024.0 * 1024.0;
    let f = b as f64;
    if f >= gib {
        format!("{:.1} GiB", f / gib)
    } else if f >= mib {
        format!("{:.0} MiB", f / mib)
    } else {
        format!("{} B", b)
    }
}

/// Just the numeric GiB value (for aligned "used / total" pairs).
pub fn gib(b: u64) -> f64 {
    b as f64 / (1024.0 * 1024.0 * 1024.0)
}
