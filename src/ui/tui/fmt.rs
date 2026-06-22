//! Text, number, and colour helpers shared by the TUI chrome and the tab bodies:
//! byte/frequency/uptime humanizing, level/thermal/pressure colour mapping, the
//! plain-language status lines, the segmented memory bar, and the process/storage
//! one-liners. Pure functions — no I/O, no terminal state.

use crate::sensors::snapshot::{HOG_CPU_PCT, HOG_RAM_FRAC, Level, Snapshot, Thermal};
use crate::ui::style::Style;

pub(super) fn gib(b: u64) -> f64 {
    b as f64 / 1_073_741_824.0
}
pub(super) fn gb_dec(b: u64) -> u64 {
    b / 1_000_000_000
}
pub(super) fn ghz(mhz: u32) -> String {
    if mhz >= 1000 {
        format!("{:.1} GHz", mhz as f64 / 1000.0)
    } else {
        format!("{mhz} MHz")
    }
}
pub(super) fn human_mem(b: u64) -> String {
    let g = gib(b);
    if g >= 1.0 {
        format!("{g:.1} GB")
    } else {
        format!("{:.0} MB", b as f64 / 1_048_576.0)
    }
}
/// Compact byte-rate for the NET lane: `1.2M/s` · `340K/s` · `12B/s` (decimal units).
pub(super) fn fmt_rate(bps: f64) -> String {
    if bps >= 1_000_000.0 {
        format!("{:.1}M/s", bps / 1_000_000.0)
    } else if bps >= 1_000.0 {
        format!("{:.0}K/s", bps / 1_000.0)
    } else {
        format!("{:.0}B/s", bps)
    }
}
pub(super) fn fmt_uptime(s: u64) -> String {
    let (d, h, m) = (s / 86400, (s % 86400) / 3600, (s % 3600) / 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}
pub(super) fn clean_proc(name: &str) -> String {
    let base = name.rsplit('/').next().unwrap_or(name);
    let short = base
        .strip_prefix("com.apple.")
        .and_then(|r| r.split('.').next())
        .unwrap_or(base);
    trunc(short, 18)
}
fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n.saturating_sub(1)).collect::<String>() + "…"
    }
}

pub(super) fn min_max(h: &[f64]) -> (f64, f64) {
    if h.is_empty() {
        return (0.0, 0.0);
    }
    let mn = h.iter().cloned().fold(f64::INFINITY, f64::min);
    let mx = h.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    (mn, mx)
}

pub(super) fn level_color(st: &Style, lvl: Level) -> &'static str {
    match lvl {
        Level::Ok => st.green,
        Level::Warn => st.yellow,
        Level::Alert => st.red,
    }
}
pub(super) fn thermal_color(st: &Style, t: Thermal) -> &'static str {
    match t {
        Thermal::Nominal => st.green,
        Thermal::Fair => st.yellow,
        Thermal::Serious | Thermal::Critical => st.red,
        Thermal::Unknown => st.dim,
    }
}
pub(super) fn pressure_color(st: &Style, p: &str) -> &'static str {
    match p {
        "low" => st.green,
        "medium" => st.yellow,
        "high" => st.red,
        _ => st.dim,
    }
}
pub(super) fn battery_color(pct: u8, st: &Style) -> &'static str {
    if pct < 20 {
        st.red
    } else if pct < 50 {
        st.yellow
    } else {
        st.green
    }
}

pub(super) fn human_status(s: &Snapshot) -> (&'static str, &'static str) {
    if s.fan_failed() {
        return (
            "Check the fan",
            "Cooling is calling for the fan, but it reads stopped.",
        );
    }
    match s.thermal {
        Thermal::Critical | Thermal::Serious => {
            ("Running hot", "macOS is easing off to cool things down.")
        }
        Thermal::Fair => (
            "Warming up",
            "A little thermal pressure, but handling it fine.",
        ),
        Thermal::Nominal => ("All good", "Cool and fast. Nothing is throttling."),
        Thermal::Unknown => ("Running", "Live readings below."),
    }
}

pub(super) fn pressure_words(p: &str) -> &'static str {
    match p {
        "low" => "the Mac has memory to spare",
        "medium" => "starting to compress — still ok",
        "high" => "little headroom; apps may slow",
        _ => "",
    }
}

pub(super) fn thermal_words(t: Thermal) -> &'static str {
    match t {
        Thermal::Nominal => "~90° is normal under load — nothing is throttling",
        Thermal::Fair => "a little thermal pressure, handling it fine",
        Thermal::Serious => "easing off to cool things down",
        Thermal::Critical => "throttling hard to protect the chip",
        Thermal::Unknown => "thermal state unknown",
    }
}

/// Plain-language reason for the current memory pressure: how much is still reclaimable
/// and whether anything has spilled to swap. Turns a mute "● medium" into something that
/// says *why* — the biggest holder is printed on the line below it in the Memory view.
pub(super) fn why_pressure(s: &Snapshot) -> String {
    let avail = gib(s.ram_available);
    match s.mem_pressure() {
        "low" => format!("{avail:.1} GB reclaimable — use it freely"),
        "medium" if s.swap_used > 0 => format!(
            "{avail:.1} GB still reclaimable; {:.1} GB spilled to swap earlier",
            gib(s.swap_used)
        ),
        "medium" => format!("{avail:.1} GB still reclaimable, nothing swapped"),
        "high" => format!("only {avail:.1} GB left to reclaim — macOS will swap to cope"),
        _ => "memory state unknown".to_string(),
    }
}

/// Three-zone gauge: used (▓) · cached/reclaimable (▒) · free (░).
pub(super) fn seg_bar(used: u64, cached: u64, total: u64, width: usize, st: &Style) -> String {
    if total == 0 {
        return "░".repeat(width);
    }
    let cell = |b: u64| ((b as f64 / total as f64) * width as f64).round() as usize;
    let u = cell(used).min(width);
    let c = cell(cached).min(width - u);
    let fr = width - u - c;
    format!(
        "{}{}{}{}{}{}",
        st.reset,
        "▓".repeat(u),
        st.dim,
        "▒".repeat(c),
        "░".repeat(fr),
        st.reset,
    )
}

pub(super) fn storage_color(st: &Style, free: u64, total: u64) -> (&'static str, &'static str) {
    let gb = gb_dec(free);
    let pct_free = if total > 0 {
        free as f64 / total as f64
    } else {
        1.0
    };
    // macOS cares about absolute free GB; the % keeps "plenty" honest on big disks.
    if gb < 10 || pct_free < 0.03 {
        (st.red, "almost full")
    } else if gb < 25 || pct_free < 0.10 {
        (st.yellow, "getting full")
    } else {
        (st.green, "plenty free")
    }
}

/// One-line summary of every mounted volume, for the Overview DISK row.
pub(super) fn storage_summary(s: &Snapshot, st: &Style) -> String {
    let d = st.dim;
    let z = st.reset;
    let one = |label: &str, free: u64, total: u64| {
        let (sc, _) = storage_color(st, free, total);
        let used = total.saturating_sub(free);
        format!("{label} {sc}{}{z}{d}/{}{z} GB", gb_dec(used), gb_dec(total))
    };
    let parts: Vec<String> = if !s.volumes.is_empty() {
        s.volumes
            .iter()
            .map(|v| {
                let label = if v.mount_point == "/" {
                    "/"
                } else {
                    v.name.as_str()
                };
                one(label, v.free, v.total)
            })
            .collect()
    } else if let Some(disk) = &s.disk {
        vec![one("/", disk.free, disk.total)]
    } else {
        vec![]
    };
    if parts.is_empty() {
        format!("{d}disk info unavailable{z}")
    } else {
        parts.join(&format!(" {d}·{z} "))
    }
}

pub(super) fn procs_cpu(s: &Snapshot, st: &Style, n: usize) -> String {
    let d = st.dim;
    let z = st.reset;
    s.top_procs
        .iter()
        .take(n)
        .map(|p| {
            // Flag a real hog in red, a merely busy process (≥ one core) in fire.
            let col = if p.cpu >= HOG_CPU_PCT {
                st.red
            } else if p.cpu >= 100.0 {
                st.fire
            } else {
                z
            };
            format!("{col}{}{z} {d}{:.0}%{z}", clean_proc(&p.name), p.cpu)
        })
        .collect::<Vec<_>>()
        .join(&format!(" {d}·{z} "))
}
pub(super) fn procs_mem(s: &Snapshot, st: &Style, n: usize) -> String {
    let d = st.dim;
    let z = st.reset;
    let total = s.ram_total.max(1) as f64;
    s.top_mem
        .iter()
        .take(n)
        .map(|p| {
            let col = if p.mem as f64 / total >= HOG_RAM_FRAC {
                st.red
            } else {
                z
            };
            format!("{col}{}{z} {d}{}{z}", clean_proc(&p.name), human_mem(p.mem))
        })
        .collect::<Vec<_>>()
        .join(&format!(" {d}·{z} "))
}
