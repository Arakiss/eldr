//! Host-level readings: RAM/swap (mach), per-core load, load average, uptime, disk,
//! network rates and top processes. The interval-based readings (per-core, net,
//! processes) use a t0/finish pair so they share the snapshot's single sample window.

use crate::ffi::{mach, proc};
use crate::sensors::snapshot::{DiskInfo, NetInfo, ProcInfo};
use std::collections::HashMap;

pub use mach::{mem_info, page_size, ram_total, ram_used, swap};

/// The "before" half of an interval reading. Pair with [`finish`].
pub struct HostT0 {
    cpu: Vec<[u64; 4]>,
    net: (u64, u64),
    procs: HashMap<i32, (u64, u64)>,
    at: std::time::Instant,
}

/// Interval-derived host metrics.
#[derive(Default)]
pub struct HostMetrics {
    pub per_core: Vec<f32>,
    pub cpu_total: f32, // mean of per-core, 0..1
    pub load: (f32, f32, f32),
    pub uptime_secs: u64,
    pub disk: DiskInfo,
    pub net: NetInfo,
    pub top: Vec<ProcInfo>,
    pub top_mem: Vec<ProcInfo>,
}

/// Capture the t0 side: per-core ticks, net counters, per-process CPU times.
pub fn start() -> HostT0 {
    HostT0 {
        cpu: mach::cpu_ticks(),
        net: mach::net_counters(),
        procs: proc::sample_all(),
        at: std::time::Instant::now(),
    }
}

/// Capture the t1 side and reduce to [`HostMetrics`]. Call after the sample window.
pub fn finish(t0: HostT0, top_n: usize) -> HostMetrics {
    let cpu1 = mach::cpu_ticks();
    let net1 = mach::net_counters();
    let procs1 = proc::sample_all();
    let dt = t0.at.elapsed().as_secs_f64().max(1e-3);

    let per_core = mach::cpu_usage(&t0.cpu, &cpu1);
    let cpu_total = if per_core.is_empty() {
        0.0
    } else {
        per_core.iter().sum::<f32>() / per_core.len() as f32
    };

    let rx_rate = (net1.0.saturating_sub(t0.net.0)) as f64 / dt;
    let tx_rate = (net1.1.saturating_sub(t0.net.1)) as f64 / dt;

    let (dtotal, dfree) = mach::disk("/");

    let top = proc::top(&t0.procs, &procs1, dt, top_n)
        .into_iter()
        .map(|(pid, cpu, name)| ProcInfo {
            pid,
            cpu,
            name,
            mem: 0,
        })
        .collect();
    let top_mem = proc::top_mem(&procs1, top_n)
        .into_iter()
        .map(|(pid, mem, name)| ProcInfo {
            pid,
            cpu: 0.0,
            name,
            mem,
        })
        .collect();

    HostMetrics {
        per_core,
        cpu_total,
        load: mach::load_avg(),
        uptime_secs: mach::uptime_secs(),
        disk: DiskInfo {
            total: dtotal,
            free: dfree,
        },
        net: NetInfo {
            rx_bytes: net1.0,
            tx_bytes: net1.1,
            rx_rate,
            tx_rate,
        },
        top,
        top_mem,
    }
}

/// Seconds since the Unix epoch, formatted as a UTC ISO-8601 timestamp.
pub fn timestamp() -> String {
    format_iso_utc(unix_time())
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
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, mo, d, hh, mm, ss)
}

/// Howard Hinnant's days-from-civil inverse: epoch-day -> (year, month, day).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_utc_known_epochs() {
        assert_eq!(format_iso_utc(0), "1970-01-01T00:00:00Z");
        // 2026-06-03T15:52:21Z
        assert_eq!(format_iso_utc(1_780_501_941), "2026-06-03T15:52:21Z");
        // leap day 2024-02-29T12:00:00Z
        assert_eq!(format_iso_utc(1_709_208_000), "2024-02-29T12:00:00Z");
    }
}
