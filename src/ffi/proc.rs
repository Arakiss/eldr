//! Process enumeration via libproc (no `sysinfo` crate). Per-process CPU% is the
//! delta of cumulative user+system time across two samples over the snapshot window.

use core::ffi::{c_int, c_void};
use std::collections::HashMap;

const PROC_ALL_PIDS: u32 = 1;
const RUSAGE_INFO_V2: c_int = 2;

unsafe extern "C" {
    fn proc_listpids(ty: u32, typeinfo: u32, buffer: *mut c_void, buffersize: c_int) -> c_int;
    fn proc_name(pid: c_int, buffer: *mut c_void, buffersize: u32) -> c_int;
    fn proc_pid_rusage(pid: c_int, flavor: c_int, buffer: *mut c_void) -> c_int;
}

/// Leading fields of `rusage_info_v2` (from `<sys/resource.h>`). CPU time is
/// `ri_user_time`/`ri_system_time` (cumulative ns); `ri_phys_footprint` is the memory
/// footprint Activity Monitor shows. `_rest` pads to the full struct so the kernel
/// write fits (total 18 u64 after the uuid).
#[repr(C)]
#[derive(Default)]
struct RusageInfoV2 {
    ri_uuid: [u8; 16],
    ri_user_time: u64,
    ri_system_time: u64,
    ri_pkg_idle_wkups: u64,
    ri_interrupt_wkups: u64,
    ri_pageins: u64,
    ri_wired_size: u64,
    ri_resident_size: u64,
    ri_phys_footprint: u64,
    _rest: [u64; 10],
}

/// All current PIDs.
pub fn list_pids() -> Vec<i32> {
    let needed = unsafe { proc_listpids(PROC_ALL_PIDS, 0, std::ptr::null_mut(), 0) };
    if needed <= 0 {
        return Vec::new();
    }
    let cap = (needed as usize / size_of::<i32>()) + 16;
    let mut pids = vec![0i32; cap];
    let got = unsafe {
        proc_listpids(
            PROC_ALL_PIDS,
            0,
            pids.as_mut_ptr() as *mut c_void,
            (pids.len() * size_of::<i32>()) as c_int,
        )
    };
    if got <= 0 {
        return Vec::new();
    }
    let n = got as usize / size_of::<i32>();
    pids.truncate(n);
    pids.retain(|&p| p > 0);
    pids
}

/// Cumulative CPU nanoseconds (user+system) for a PID, or `None` if inaccessible.
fn cpu_ns(pid: i32) -> Option<u64> {
    let mut ri = RusageInfoV2::default();
    let rc = unsafe { proc_pid_rusage(pid, RUSAGE_INFO_V2, &mut ri as *mut _ as *mut c_void) };
    if rc != 0 {
        return None;
    }
    Some(ri.ri_user_time + ri.ri_system_time)
}

/// All PIDs whose short name equals `target` (e.g. `claude`, `codex`).
pub fn pids_named(target: &str) -> Vec<i32> {
    list_pids()
        .into_iter()
        .filter(|&p| name_of(p) == target)
        .collect()
}

/// Short process name for a PID.
pub fn name_of(pid: i32) -> String {
    let mut buf = [0u8; 256];
    let n = unsafe { proc_name(pid, buf.as_mut_ptr() as *mut c_void, buf.len() as u32) };
    if n <= 0 {
        return format!("pid {pid}");
    }
    String::from_utf8_lossy(&buf[..n as usize]).into_owned()
}

/// Memory footprint in bytes (`ri_phys_footprint`) for a PID, or `None` if
/// inaccessible. This is the figure Activity Monitor reports under "Memory".
fn mem_footprint(pid: i32) -> Option<u64> {
    let mut ri = RusageInfoV2::default();
    let rc = unsafe { proc_pid_rusage(pid, RUSAGE_INFO_V2, &mut ri as *mut _ as *mut c_void) };
    if rc != 0 {
        return None;
    }
    Some(ri.ri_phys_footprint)
}

/// Top `n` processes by memory footprint, as `(pid, bytes, name)`.
pub fn top_mem(n: usize) -> Vec<(i32, u64, String)> {
    let mut rows: Vec<(i32, u64)> = list_pids()
        .into_iter()
        .filter_map(|pid| mem_footprint(pid).map(|m| (pid, m)))
        .filter(|&(_, m)| m > 0)
        .collect();
    rows.sort_by_key(|&(_, m)| std::cmp::Reverse(m));
    rows.truncate(n);
    rows.into_iter()
        .map(|(pid, m)| (pid, m, name_of(pid)))
        .collect()
}

/// Snapshot of every PID's cumulative CPU nanoseconds.
pub fn cpu_times() -> HashMap<i32, u64> {
    let mut m = HashMap::new();
    for pid in list_pids() {
        if let Some(ns) = cpu_ns(pid) {
            m.insert(pid, ns);
        }
    }
    m
}

/// Top `n` processes by CPU% over `dt_secs`, as `(pid, cpu_percent, name)`.
pub fn top(
    t0: &HashMap<i32, u64>,
    t1: &HashMap<i32, u64>,
    dt_secs: f64,
    n: usize,
) -> Vec<(i32, f32, String)> {
    let window_ns = (dt_secs * 1e9).max(1.0);
    let mut rows: Vec<(i32, f32)> = Vec::new();
    for (&pid, &ns1) in t1 {
        let ns0 = t0.get(&pid).copied().unwrap_or(ns1);
        let d = ns1.saturating_sub(ns0) as f64;
        let pct = (d / window_ns * 100.0) as f32;
        if pct > 0.05 {
            rows.push((pid, pct));
        }
    }
    rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    rows.truncate(n);
    rows.into_iter()
        .map(|(pid, pct)| (pid, pct, name_of(pid)))
        .collect()
}
