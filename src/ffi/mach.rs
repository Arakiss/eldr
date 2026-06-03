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
    fn host_statistics64(host: u32, flavor: c_int, info: *mut c_int, count: *mut u32) -> c_int;
}

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
        if sysctlbyname(cname.as_ptr(), ptr::null_mut(), &mut size, ptr::null(), 0) != 0 || size == 0
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

/// Used RAM in bytes, matching macmon's accounting (so `eldr now` agrees with
/// `macmon pipe`): active + inactive + wired + speculative + compressed, minus
/// purgeable and file-backed (external) pages.
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
