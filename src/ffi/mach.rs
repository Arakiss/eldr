//! sysctl + mach host statistics, hand-written over libSystem (no `libc` crate).
//!
//! libSystem is linked into every macOS binary, so these symbols resolve without an
//! explicit `#[link]` attribute.

use core::ffi::{c_char, c_int, c_void};
use std::ffi::CString;
use std::ptr;

unsafe extern "C" {
    fn sysctlbyname(
        name: *const c_char,
        oldp: *mut c_void,
        oldlenp: *mut usize,
        newp: *const c_void,
        newlen: usize,
    ) -> c_int;
    fn mach_host_self() -> u32;
    fn mach_task_self() -> u32;
    fn host_statistics64(host: u32, flavor: c_int, info: *mut c_int, count: *mut u32) -> c_int;
    fn host_processor_info(
        host: u32,
        flavor: c_int,
        out_processor_count: *mut u32,
        out_processor_info: *mut *mut c_int,
        out_processor_info_count: *mut u32,
    ) -> c_int;
    fn vm_deallocate(target_task: u32, address: usize, size: usize) -> c_int;
    fn getloadavg(loadavg: *mut f64, nelem: c_int) -> c_int;
    fn statfs(path: *const c_char, buf: *mut Statfs) -> c_int;
    fn getifaddrs(ifap: *mut *mut Ifaddrs) -> c_int;
    fn freeifaddrs(ifa: *mut Ifaddrs);
}

const PROCESSOR_CPU_LOAD_INFO: c_int = 2;

// mach/host_info.h
const HOST_VM_INFO64: c_int = 4;

/// `vm_statistics64` from `<mach/vm_statistics.h>`. `natural_t` = `u32`.
/// Layout must match the kernel struct exactly (`#[repr(C)]`).
#[repr(C)]
#[derive(Default)]
struct VmStatistics64 {
    free_count: u32,
    active_count: u32,
    inactive_count: u32,
    wire_count: u32,
    zero_fill_count: u64,
    reactivations: u64,
    pageins: u64,
    pageouts: u64,
    faults: u64,
    cow_faults: u64,
    lookups: u64,
    hits: u64,
    purges: u64,
    purgeable_count: u32,
    speculative_count: u32,
    decompressions: u64,
    compressions: u64,
    swapins: u64,
    swapouts: u64,
    compressor_page_count: u32,
    throttled_count: u32,
    external_page_count: u32,
    internal_page_count: u32,
    total_uncompressed_pages_in_compressor: u64,
}

/// `xsw_usage` from `<sys/sysctl.h>` (`vm.swapusage`).
#[repr(C)]
#[derive(Default)]
struct XswUsage {
    total: u64,
    avail: u64,
    used: u64,
    pagesize: u32,
    encrypted: i32,
}

// MARK: sysctl typed readers

/// Read a string sysctl by name (e.g. `machdep.cpu.brand_string`).
pub fn sysctl_string(name: &str) -> Option<String> {
    let cname = CString::new(name).ok()?;
    let mut size: usize = 0;
    unsafe {
        if sysctlbyname(cname.as_ptr(), ptr::null_mut(), &mut size, ptr::null(), 0) != 0
            || size == 0
        {
            return None;
        }
        let mut buf = vec![0u8; size];
        if sysctlbyname(
            cname.as_ptr(),
            buf.as_mut_ptr() as *mut c_void,
            &mut size,
            ptr::null(),
            0,
        ) != 0
        {
            return None;
        }
        buf.truncate(size);
        if buf.last() == Some(&0) {
            buf.pop();
        }
        String::from_utf8(buf).ok()
    }
}

/// Read an integer sysctl by name into `u64`. Handles 4- or 8-byte sysctls.
pub fn sysctl_u64(name: &str) -> Option<u64> {
    let cname = CString::new(name).ok()?;
    let mut size: usize = 0;
    unsafe {
        if sysctlbyname(cname.as_ptr(), ptr::null_mut(), &mut size, ptr::null(), 0) != 0 {
            return None;
        }
        match size {
            4 => {
                let mut v: u32 = 0;
                let mut s = 4usize;
                if sysctlbyname(
                    cname.as_ptr(),
                    &mut v as *mut _ as *mut c_void,
                    &mut s,
                    ptr::null(),
                    0,
                ) != 0
                {
                    return None;
                }
                Some(v as u64)
            }
            8 => {
                let mut v: u64 = 0;
                let mut s = 8usize;
                if sysctlbyname(
                    cname.as_ptr(),
                    &mut v as *mut _ as *mut c_void,
                    &mut s,
                    ptr::null(),
                    0,
                ) != 0
                {
                    return None;
                }
                Some(v)
            }
            _ => None,
        }
    }
}

// MARK: memory

/// Total physical RAM in bytes (`hw.memsize`).
pub fn ram_total() -> u64 {
    sysctl_u64("hw.memsize").unwrap_or(0)
}

/// Page size in bytes (`hw.pagesize`).
pub fn page_size() -> u64 {
    sysctl_u64("hw.pagesize").unwrap_or(16384)
}

/// Used RAM in bytes, matching the reference accounting (provenance in NOTICE):
/// active + inactive + wired + speculative + compressed, minus purgeable and
/// file-backed (external) pages.
pub fn ram_used() -> u64 {
    let mut stats = VmStatistics64::default();
    let mut count = (size_of::<VmStatistics64>() / size_of::<c_int>()) as u32;
    let rc = unsafe {
        host_statistics64(
            mach_host_self(),
            HOST_VM_INFO64,
            &mut stats as *mut _ as *mut c_int,
            &mut count,
        )
    };
    if rc != 0 {
        return 0;
    }
    let page = page_size();
    let used_pages = (stats.active_count as i64
        + stats.inactive_count as i64
        + stats.wire_count as i64
        + stats.speculative_count as i64
        + stats.compressor_page_count as i64
        - stats.purgeable_count as i64
        - stats.external_page_count as i64)
        .max(0) as u64;
    used_pages * page
}

/// A human-meaningful memory breakdown, the way Activity Monitor frames it: what is
/// genuinely occupied (`used` = app + wired + compressed) versus what can be handed
/// back to apps without swapping (`available` = free + reclaimable cache). A raw
/// "% used" is misleading on macOS because file cache counts as "used" but is free
/// for the taking.
#[derive(Default, Clone, Copy)]
pub struct MemInfo {
    pub total: u64,
    pub used: u64,       // app + wired + compressed (Activity Monitor "Memory Used")
    pub available: u64,  // free + reclaimable cache
    pub free: u64,       // truly free right now
    pub cached: u64,     // file-backed + purgeable + speculative (reclaimable)
    pub wired: u64,      // can't be paged out
    pub compressed: u64, // in the memory compressor
    pub app: u64,        // anonymous app memory
}

impl MemInfo {
    /// Plain-language pressure from how much is reclaimable, not from raw "% used".
    pub fn pressure(&self) -> &'static str {
        if self.total == 0 {
            return "unknown";
        }
        let avail = self.available as f64 / self.total as f64;
        if avail >= 0.30 {
            "low"
        } else if avail >= 0.10 {
            "medium"
        } else {
            "high"
        }
    }
}

/// Detailed memory accounting from `vm_statistics64`.
pub fn mem_info() -> MemInfo {
    let total = ram_total();
    let mut stats = VmStatistics64::default();
    let mut count = (size_of::<VmStatistics64>() / size_of::<c_int>()) as u32;
    let rc = unsafe {
        host_statistics64(
            mach_host_self(),
            HOST_VM_INFO64,
            &mut stats as *mut _ as *mut c_int,
            &mut count,
        )
    };
    if rc != 0 {
        return MemInfo {
            total,
            ..Default::default()
        };
    }
    let page = page_size();
    let p = |n: u32| n as u64 * page;
    let free = p(stats.free_count);
    let wired = p(stats.wire_count);
    let compressed = p(stats.compressor_page_count);
    // Anonymous app pages minus the purgeable subset (which is reclaimable).
    let app = p(stats
        .internal_page_count
        .saturating_sub(stats.purgeable_count));
    // File-backed + purgeable + read-ahead: all reclaimable under pressure.
    let cached = p(stats.external_page_count + stats.purgeable_count + stats.speculative_count);
    let used = wired + compressed + app;
    let available = free + cached;
    MemInfo {
        total,
        used,
        available,
        free,
        cached,
        wired,
        compressed,
        app,
    }
}

/// Swap usage as `(used, total)` bytes (`vm.swapusage`).
pub fn swap() -> (u64, u64) {
    let cname = match CString::new("vm.swapusage") {
        Ok(c) => c,
        Err(_) => return (0, 0),
    };
    let mut xsw = XswUsage::default();
    let mut size = size_of::<XswUsage>();
    let rc = unsafe {
        sysctlbyname(
            cname.as_ptr(),
            &mut xsw as *mut _ as *mut c_void,
            &mut size,
            ptr::null(),
            0,
        )
    };
    if rc != 0 {
        return (0, 0);
    }
    (xsw.used, xsw.total)
}

// MARK: per-core CPU load

/// Per-core cumulative CPU ticks `[user, system, idle, nice]` (CPU_STATE order).
/// Take two snapshots over an interval and diff them for per-core utilization.
pub fn cpu_ticks() -> Vec<[u64; 4]> {
    let mut count: u32 = 0;
    let mut info: *mut c_int = ptr::null_mut();
    let mut info_count: u32 = 0;
    let rc = unsafe {
        host_processor_info(
            mach_host_self(),
            PROCESSOR_CPU_LOAD_INFO,
            &mut count,
            &mut info,
            &mut info_count,
        )
    };
    if rc != 0 || info.is_null() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(count as usize);
    unsafe {
        for i in 0..count as usize {
            let base = i * 4;
            out.push([
                *info.add(base) as u32 as u64,
                *info.add(base + 1) as u32 as u64,
                *info.add(base + 2) as u32 as u64,
                *info.add(base + 3) as u32 as u64,
            ]);
        }
        vm_deallocate(
            mach_task_self(),
            info as usize,
            info_count as usize * size_of::<c_int>(),
        );
    }
    out
}

/// Per-core busy fraction `0..1` from two ticks snapshots. Busy = user+system+nice.
pub fn cpu_usage(t0: &[[u64; 4]], t1: &[[u64; 4]]) -> Vec<f32> {
    let n = t0.len().min(t1.len());
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let busy0 = t0[i][0] + t0[i][1] + t0[i][3];
        let busy1 = t1[i][0] + t1[i][1] + t1[i][3];
        let total0 = busy0 + t0[i][2];
        let total1 = busy1 + t1[i][2];
        let dbusy = busy1.saturating_sub(busy0) as f32;
        let dtotal = total1.saturating_sub(total0) as f32;
        out.push(if dtotal > 0.0 {
            (dbusy / dtotal).clamp(0.0, 1.0)
        } else {
            0.0
        });
    }
    out
}

/// 1/5/15-minute load averages.
pub fn load_avg() -> (f32, f32, f32) {
    let mut la = [0f64; 3];
    let n = unsafe { getloadavg(la.as_mut_ptr(), 3) };
    if n < 3 {
        return (0.0, 0.0, 0.0);
    }
    (la[0] as f32, la[1] as f32, la[2] as f32)
}

/// Seconds since boot (`kern.boottime`).
pub fn uptime_secs() -> u64 {
    // kern.boottime returns a `struct timeval { i64 sec; i32 usec }` (16 bytes padded).
    #[repr(C)]
    #[derive(Default)]
    struct Timeval {
        sec: i64,
        usec: i64,
    }
    let cname = match CString::new("kern.boottime") {
        Ok(c) => c,
        Err(_) => return 0,
    };
    let mut tv = Timeval::default();
    let mut size = size_of::<Timeval>();
    let rc = unsafe {
        sysctlbyname(
            cname.as_ptr(),
            &mut tv as *mut _ as *mut c_void,
            &mut size,
            ptr::null(),
            0,
        )
    };
    if rc != 0 || tv.sec <= 0 {
        return 0;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    (now - tv.sec).max(0) as u64
}

// MARK: disk

/// macOS `struct statfs` (arm64, 64-bit inodes). We read only the leading block
/// counters; the rest must be present so the kernel write fits our buffer.
#[repr(C)]
struct Statfs {
    f_bsize: u32,
    f_iosize: i32,
    f_blocks: u64,
    f_bfree: u64,
    f_bavail: u64,
    f_files: u64,
    f_ffree: u64,
    f_fsid: [i32; 2],
    f_owner: u32,
    f_type: u32,
    f_flags: u32,
    f_fssubtype: u32,
    f_fstypename: [u8; 16],
    f_mntonname: [u8; 1024],
    f_mntfromname: [u8; 1024],
    f_flags_ext: u32,
    f_reserved: [u32; 7],
}

/// Filesystem `(total, free)` bytes for `path` (use `/` for the boot volume).
pub fn disk(path: &str) -> (u64, u64) {
    let cpath = match CString::new(path) {
        Ok(c) => c,
        Err(_) => return (0, 0),
    };
    // Zeroed buffer; only leading fields are read after the call.
    let mut sf: Statfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { statfs(cpath.as_ptr(), &mut sf) };
    if rc != 0 {
        return (0, 0);
    }
    let bs = sf.f_bsize as u64;
    (sf.f_blocks * bs, sf.f_bavail * bs)
}

// MARK: network

#[repr(C)]
struct Sockaddr {
    sa_len: u8,
    sa_family: u8,
    sa_data: [u8; 14],
}

#[repr(C)]
struct Ifaddrs {
    ifa_next: *mut Ifaddrs,
    ifa_name: *const c_char,
    ifa_flags: u32,
    ifa_addr: *mut Sockaddr,
    ifa_netmask: *mut Sockaddr,
    ifa_dstaddr: *mut Sockaddr,
    ifa_data: *mut c_void,
}

/// Leading fields of `struct if_data` up to the byte counters (u32, may wrap; we use
/// them for deltas only).
#[repr(C)]
struct IfData {
    ifi_type: u8,
    ifi_typelen: u8,
    ifi_physical: u8,
    ifi_addrlen: u8,
    ifi_hdrlen: u8,
    ifi_recvquota: u8,
    ifi_xmitquota: u8,
    ifi_unused1: u8,
    ifi_mtu: u32,
    ifi_metric: u32,
    ifi_baudrate: u32,
    ifi_ipackets: u32,
    ifi_ierrors: u32,
    ifi_opackets: u32,
    ifi_oerrors: u32,
    ifi_collisions: u32,
    ifi_ibytes: u32,
    ifi_obytes: u32,
}

const AF_LINK: u8 = 18;

/// Total `(rx_bytes, tx_bytes)` across physical interfaces (excludes `lo0`).
pub fn net_counters() -> (u64, u64) {
    let mut head: *mut Ifaddrs = ptr::null_mut();
    if unsafe { getifaddrs(&mut head) } != 0 || head.is_null() {
        return (0, 0);
    }
    let (mut rx, mut tx) = (0u64, 0u64);
    let mut cur = head;
    unsafe {
        while !cur.is_null() {
            let ifa = &*cur;
            if !ifa.ifa_addr.is_null()
                && (*ifa.ifa_addr).sa_family == AF_LINK
                && !ifa.ifa_data.is_null()
            {
                let name = if ifa.ifa_name.is_null() {
                    String::new()
                } else {
                    std::ffi::CStr::from_ptr(ifa.ifa_name)
                        .to_string_lossy()
                        .into_owned()
                };
                if name != "lo0" {
                    let d = &*(ifa.ifa_data as *const IfData);
                    rx += d.ifi_ibytes as u64;
                    tx += d.ifi_obytes as u64;
                }
            }
            cur = ifa.ifa_next;
        }
        freeifaddrs(head);
    }
    (rx, tx)
}
