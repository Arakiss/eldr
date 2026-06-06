//! Per-physical-disk I/O statistics and the firmware SMART verdict. No sudo.
//!
//! Counters come from IOKit: each `IOBlockStorageDevice` carries device/protocol
//! characteristics, and its child driver/media nodes carry the `Statistics` dict and the
//! `BSD Name`. Rising `Errors`/`Retries` are the earliest honest sign of a failing SSD
//! or a flaky cable/enclosure. The pass/fail SMART verdict comes from `diskutil` (a
//! system tool, same lane as the osascript/lsof/git the daemon already shells out to),
//! kept off the hot snapshot path because it spawns a process.

use crate::ffi::cf::{self, CFDictionaryRef};
use crate::ffi::iokit::{
    IOObjectRelease, IOServiceIterator, entry_properties, entry_search_property,
};
use std::process::Command;

/// Raw per-disk counters and identity, straight from IOKit (no SMART verdict — that is
/// read separately via [`smart_status`]).
pub struct DiskStat {
    pub bsd_name: String,     // "disk4"
    pub model: String,        // "Samsung SSD 990 PRO 4TB"
    pub external: bool,       // Physical Interconnect Location == External
    pub interconnect: String, // "PCI-Express" | "USB" | "SATA" | "Apple Fabric"
    pub solid_state: bool,
    pub read_errors: u64,
    pub write_errors: u64,
    pub read_retries: u64,
    pub write_retries: u64,
    pub read_ops: u64,
    pub write_ops: u64,
    pub read_bytes: u64,
    pub write_bytes: u64,
    pub read_time_ns: u64, // cumulative service time; /ops gives mean latency
    pub write_time_ns: u64,
    /// Firmware NVMe SMART telemetry (temp, wear, TBW), when the disk exposes it.
    pub nvme: Option<crate::ffi::nvme::NvmeSmart>,
}

/// Every physical block storage device with its I/O counters. Cheap (pure IOKit reads,
/// no shell-out), so it is safe on the per-snapshot path.
pub fn disks() -> Vec<DiskStat> {
    let Some(it) = IOServiceIterator::new("IOBlockStorageDevice") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (entry, _name) in it {
        if let Some(d) = read_disk(entry) {
            out.push(d);
        }
        unsafe { IOObjectRelease(entry) };
    }
    out
}

fn read_disk(entry: u32) -> Option<DiskStat> {
    let props = entry_properties(entry)?;
    // Identity lives on this node (Device/Protocol Characteristics sub-dicts).
    let dev = cf::cfdict_get_val(props, "Device Characteristics");
    let model = dev
        .and_then(|d| cf::cfdict_get_val(d, "Product Name"))
        .map(cf::from_cfstr)
        .unwrap_or_default();
    let medium = dev
        .and_then(|d| cf::cfdict_get_val(d, "Medium Type"))
        .map(cf::from_cfstr)
        .unwrap_or_default();
    let proto = cf::cfdict_get_val(props, "Protocol Characteristics");
    let location = proto
        .and_then(|p| cf::cfdict_get_val(p, "Physical Interconnect Location"))
        .map(cf::from_cfstr)
        .unwrap_or_default();
    let interconnect = proto
        .and_then(|p| cf::cfdict_get_val(p, "Physical Interconnect"))
        .map(cf::from_cfstr)
        .unwrap_or_default();
    unsafe { cf::CFRelease(props) };

    // BSD name + Statistics live in child nodes; search down the IOService plane.
    let bsd_name = entry_search_property(entry, "BSD Name")
        .map(|v| {
            let s = cf::from_cfstr(v);
            unsafe { cf::CFRelease(v) };
            s
        })
        .unwrap_or_default();
    if bsd_name.is_empty() {
        return None;
    }
    let stats = entry_search_property(entry, "Statistics")?;
    let g = |k: &str| {
        cf::cfdict_get_val(stats as CFDictionaryRef, k)
            .and_then(cf::cfnum_i64)
            .unwrap_or(0)
            .max(0) as u64
    };
    let d = DiskStat {
        bsd_name,
        model: model.trim().to_string(),
        external: location.eq_ignore_ascii_case("external"),
        interconnect: interconnect.trim().to_string(),
        solid_state: medium.eq_ignore_ascii_case("solid state"),
        read_errors: g("Errors (Read)"),
        write_errors: g("Errors (Write)"),
        read_retries: g("Retries (Read)"),
        write_retries: g("Retries (Write)"),
        read_ops: g("Operations (Read)"),
        write_ops: g("Operations (Write)"),
        read_bytes: g("Bytes (Read)"),
        write_bytes: g("Bytes (Write)"),
        read_time_ns: g("Total Time (Read)"),
        write_time_ns: g("Total Time (Write)"),
        // Try the NVMe SMART plugin on this device node; None for non-NVMe disks.
        nvme: crate::ffi::nvme::read(entry),
    };
    unsafe { cf::CFRelease(stats) };
    Some(d)
}

/// Firmware SMART verdict for a BSD disk, lower-cased: "verified", "failing", "not
/// supported", or "" when unknown. Shells out to `diskutil`, so keep it off the
/// per-frame path (one-shot views and the guard call it; the TUI refresh does not).
pub fn smart_status(bsd: &str) -> String {
    let Ok(out) = Command::new("diskutil").args(["info", bsd]).output() else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        if let Some(rest) = line.trim().strip_prefix("SMART Status:") {
            return rest.trim().to_lowercase();
        }
    }
    String::new()
}
