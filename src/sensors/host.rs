//! Host-level readings: RAM/swap (mach), and — in later milestones — per-core load,
//! disk, net, uptime and top processes. M0 wires RAM/swap and a timestamp.

use crate::ffi::mach;

pub use mach::{page_size, ram_total, ram_used, swap};

/// Seconds since the Unix epoch, formatted as a local-ish ISO-8601 timestamp.
/// Hand-rolled (no `chrono`): we render UTC with a `Z` suffix for status.json.
pub fn timestamp() -> String {
    let secs = unix_time();
    format_iso_utc(secs)
}

/// Whole seconds since the Unix epoch.
pub fn unix_time() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Format a Unix timestamp as `YYYY-MM-DDTHH:MM:SSZ` (UTC), proleptic Gregorian.
pub fn format_iso_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mo, d) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y, mo, d, hh, mm, ss
    )
}

/// Howard Hinnant's days-from-civil inverse: epoch-day -> (year, month, day).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}
