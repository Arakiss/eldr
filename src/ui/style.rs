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
    /// The eldr brand accent — fire orange. Used for activity bars (CPU, power).
    pub fire: &'static str,
    pub reset: &'static str,
    /// True only on a 24-bit terminal. The RGB gradients in [`crate::ui::chart`] need
    /// truecolor; on ansi16 they fall back to a solid per-zone colour.
    pub truecolor: bool,
}

impl Style {
    pub fn detect() -> Self {
        if !std::io::stdout().is_terminal() {
            return Style::plain();
        }
        if truecolor_supported() {
            Style::color()
        } else {
            Style::ansi16()
        }
    }

    /// 8/256-colour fallback for terminals that don't advertise 24-bit colour, so the
    /// truecolor `38;2;…` codes don't render as garbage. Keeps the OK/WARN/ALERT
    /// semantics; a 256-colour orange (`208`) stands in for fire.
    pub const fn ansi16() -> Self {
        Style {
            bold: "\x1b[1m",
            dim: "\x1b[2m",
            red: "\x1b[31m",
            green: "\x1b[32m",
            yellow: "\x1b[33m",
            blue: "\x1b[34m",
            fire: "\x1b[38;5;208m",
            reset: "\x1b[0m",
            truecolor: false,
        }
    }
    /// 24-bit (truecolor) brand palette: fire on charcoal, with the product's own
    /// OK/WARN/ALERT greens/ambers/reds. Modern terminals render these directly; the
    /// codes are inert when stdout is not a tty (see `plain`).
    pub const fn color() -> Self {
        Style {
            bold: "\x1b[1m",
            dim: "\x1b[2m",
            red: "\x1b[38;2;255;95;86m",     // #ff5f56 — ALERT
            green: "\x1b[38;2;39;201;63m",   // #27c93f — OK
            yellow: "\x1b[38;2;255;189;46m", // #ffbd2e — WARN
            blue: "\x1b[38;2;127;168;201m",  // #7fa8c9
            fire: "\x1b[38;2;255;106;44m",   // #ff6a2c — brand accent
            reset: "\x1b[0m",
            truecolor: true,
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
            fire: "",
            reset: "",
            truecolor: false,
        }
    }
}

/// Whether the terminal advertises 24-bit colour. macOS terminals that support it set
/// `COLORTERM=truecolor` (or `24bit`); when it's absent we fall back rather than emit
/// truecolor codes a basic terminal would mangle.
fn truecolor_supported() -> bool {
    std::env::var("COLORTERM")
        .map(|v| v.contains("truecolor") || v.contains("24bit"))
        .unwrap_or(false)
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

/// Like [`bar`], but the filled run is painted `color` and the empty run is dim — so a
/// gauge carries its meaning in color, not just length. The caller picks `color` from the
/// signal's health (fire for activity, green/amber/red for memory/thermal/disk).
pub fn bar_c(v: f64, lo: f64, hi: f64, width: usize, color: &str, st: &Style) -> String {
    let span = (hi - lo).max(f64::MIN_POSITIVE);
    let frac = ((v - lo) / span).clamp(0.0, 1.0);
    let filled = (frac * width as f64).round() as usize;
    format!(
        "{color}{}{}{}{}",
        "█".repeat(filled),
        st.dim,
        "░".repeat(width.saturating_sub(filled)),
        st.reset,
    )
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

/// Visible width of a styled string — counts characters, skips ANSI escape sequences
/// (`ESC [ … letter`), so colour codes don't count toward the column budget. Shared by
/// the TUI layout (`tui`) and the chart compositor (`chart`).
pub fn visible_len(s: &str) -> usize {
    let mut n = 0;
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\x1b' {
            for x in it.by_ref() {
                if x.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            n += 1;
        }
    }
    n
}

/// Clip a styled line to `w` visible columns, ANSI-aware: lines that fit pass through
/// untouched; longer ones are cut to `w-1` visible chars plus an ellipsis (and a reset,
/// in case the cut fell inside a coloured run). Stops content from spilling past the
/// panel edge.
pub fn fit(s: &str, w: usize) -> String {
    if visible_len(s) <= w {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut cols = 0usize;
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\x1b' {
            out.push(c);
            for x in it.by_ref() {
                out.push(x);
                if x.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        if cols >= w.saturating_sub(1) {
            break;
        }
        out.push(c);
        cols += 1;
    }
    out.push('…');
    out.push_str("\x1b[0m");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_is_ansi_aware() {
        // Colour codes don't count toward visible width.
        assert_eq!(visible_len("\x1b[2mhello world\x1b[0m"), 11);
        // A line that fits passes through untouched.
        assert_eq!(fit("\x1b[2mhi\x1b[0m", 10), "\x1b[2mhi\x1b[0m");
        // An over-wide line is clipped to exactly w visible columns (w-1 + ellipsis)...
        let clipped = fit("\x1b[2mabcdefghij\x1b[0m", 5);
        assert_eq!(visible_len(&clipped), 5);
        // ...and never spills past the panel: the bare text case too.
        assert_eq!(visible_len(&fit("abcdefghijklmnop", 8)), 8);
    }
}
