//! NVMe SMART / health log via the IONVMeSMARTInterface CFPlugIn (no sudo). Reads the
//! firmware's own wear and thermal telemetry — composite temperature, percentage used,
//! available spare, total bytes written, power-on hours, the critical-warning bits — for
//! NVMe disks that expose it, including external Thunderbolt-NVMe enclosures.
//!
//! The interface is an undocumented COM-style vtable; the UUIDs and method order are the
//! ones smartmontools uses (os_darwin). Every step is fail-soft: any null or non-zero
//! status yields `None`, and the plugin simply fails to open on a disk that doesn't
//! expose SMART — so a non-NVMe disk costs nothing and can never crash the caller.

use core::ffi::c_void;
use std::ptr;

type CFUUIDRef = *const c_void;
type CFAllocatorRef = *const c_void;

/// `CFUUIDBytes` — 16 raw bytes, passed by value as the QueryInterface `REFIID`.
#[repr(C)]
#[derive(Clone, Copy)]
struct CFUUIDBytes {
    bytes: [u8; 16],
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    #[allow(clippy::too_many_arguments)]
    fn CFUUIDGetConstantUUIDWithBytes(
        alloc: CFAllocatorRef,
        b0: u8,
        b1: u8,
        b2: u8,
        b3: u8,
        b4: u8,
        b5: u8,
        b6: u8,
        b7: u8,
        b8: u8,
        b9: u8,
        b10: u8,
        b11: u8,
        b12: u8,
        b13: u8,
        b14: u8,
        b15: u8,
    ) -> CFUUIDRef;
    fn CFUUIDGetUUIDBytes(uuid: CFUUIDRef) -> CFUUIDBytes;
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOCreatePlugInInterfaceForService(
        service: u32,
        plugin_type: CFUUIDRef,
        interface_type: CFUUIDRef,
        the_interface: *mut *mut *mut IOCFPlugInInterface,
        the_score: *mut i32,
    ) -> i32;
    fn IODestroyPlugInInterface(interface: *mut *mut IOCFPlugInInterface) -> i32;
}

// IOCFPlugInInterface vtable — we only call QueryInterface (the IUnknown member). The
// trailing Probe/Start/Stop pointers are present for correct struct size but unused.
#[repr(C)]
struct IOCFPlugInInterface {
    _reserved: *mut c_void,
    query_interface: extern "C" fn(*mut c_void, CFUUIDBytes, *mut *mut c_void) -> i32,
    add_ref: extern "C" fn(*mut c_void) -> u32,
    release: extern "C" fn(*mut c_void) -> u32,
    version: u16,
    revision: u16,
    probe: *mut c_void,
    start: *mut c_void,
    stop: *mut c_void,
}

// IONVMeSMARTInterface vtable. SMARTReadData is the first method after version/revision;
// the methods that follow (GetIdentifyData, GetLogPage, reserved slots) are unused, so we
// declare only up to the one we call — we never read past it.
#[repr(C)]
struct IONVMeSMARTInterface {
    _reserved: *mut c_void,
    query_interface: extern "C" fn(*mut c_void, CFUUIDBytes, *mut *mut c_void) -> i32,
    add_ref: extern "C" fn(*mut c_void) -> u32,
    release: extern "C" fn(*mut c_void) -> u32,
    version: u16,
    revision: u16,
    smart_read_data: extern "C" fn(*mut c_void, *mut NvmeSmartLog) -> i32,
}

/// The 512-byte NVMe SMART / Health Information log page (spec figure). Parsed by offset.
#[repr(C)]
struct NvmeSmartLog {
    data: [u8; 512],
}

// UUID bytes (smartmontools os_darwin.h).
const NVME_SMART_USER_CLIENT_TYPE_ID: [u8; 16] = [
    0xAA, 0x0F, 0xA6, 0xF9, 0xC2, 0xD6, 0x45, 0x7F, 0xB1, 0x0B, 0x59, 0xA1, 0x32, 0x53, 0x29, 0x2F,
];
const NVME_SMART_INTERFACE_ID: [u8; 16] = [
    0xCC, 0xD1, 0xDB, 0x19, 0xFD, 0x9A, 0x4D, 0xAF, 0xBF, 0x95, 0x12, 0x45, 0x4B, 0x23, 0x0A, 0xB6,
];
const CF_PLUGIN_INTERFACE_ID: [u8; 16] = [
    0xC2, 0x44, 0xE8, 0x58, 0x10, 0x9C, 0x11, 0xD4, 0x91, 0xD4, 0x00, 0x50, 0xE4, 0xC6, 0x42, 0x6F,
];

/// NVMe firmware health telemetry, decoded from the SMART log page.
#[derive(Clone, Copy, Debug, Default)]
pub struct NvmeSmart {
    pub temp_c: f32,
    pub available_spare: u8,      // percent remaining
    pub spare_threshold: u8,      // percent at which the firmware warns
    pub percentage_used: u8,      // endurance consumed (can exceed 100)
    pub critical_warning: u8,     // bitfield; non-zero = the firmware is worried
    pub data_units_written: u128, // 1000 * 512-byte units
    pub power_on_hours: u128,
    pub media_errors: u128,
}

impl NvmeSmart {
    /// Total bytes ever written, in terabytes (a data unit is 1000 * 512 bytes).
    pub fn tbw(&self) -> f64 {
        self.data_units_written as f64 * 512_000.0 / 1.0e12
    }
}

/// Read NVMe SMART for an `IOBlockStorageDevice` registry entry. `None` if the disk is
/// not NVMe-SMART-capable or any IOKit step fails.
pub fn read(service: u32) -> Option<NvmeSmart> {
    unsafe {
        let plugin_type = const_uuid(NVME_SMART_USER_CLIENT_TYPE_ID);
        let cfplugin_id = const_uuid(CF_PLUGIN_INTERFACE_ID);
        let smart_id = const_uuid(NVME_SMART_INTERFACE_ID);
        if plugin_type.is_null() || cfplugin_id.is_null() || smart_id.is_null() {
            return None;
        }

        let mut plugin: *mut *mut IOCFPlugInInterface = ptr::null_mut();
        let mut score: i32 = 0;
        if IOCreatePlugInInterfaceForService(
            service,
            plugin_type,
            cfplugin_id,
            &mut plugin,
            &mut score,
        ) != 0
            || plugin.is_null()
        {
            return None;
        }

        // Ask the plugin for the NVMe SMART interface (double-pointer, COM style).
        let mut smart_raw: *mut c_void = ptr::null_mut();
        let qi = ((**plugin).query_interface)(
            plugin as *mut c_void,
            CFUUIDGetUUIDBytes(smart_id),
            &mut smart_raw,
        );
        if qi != 0 || smart_raw.is_null() {
            IODestroyPlugInInterface(plugin);
            return None;
        }
        let smart = smart_raw as *mut *mut IONVMeSMARTInterface;

        let mut log = NvmeSmartLog { data: [0u8; 512] };
        let rc = ((**smart).smart_read_data)(smart as *mut c_void, &mut log);
        ((**smart).release)(smart as *mut c_void);
        IODestroyPlugInInterface(plugin);
        if rc != 0 {
            return None;
        }
        Some(parse(&log.data))
    }
}

fn const_uuid(b: [u8; 16]) -> CFUUIDRef {
    unsafe {
        CFUUIDGetConstantUUIDWithBytes(
            ptr::null(),
            b[0],
            b[1],
            b[2],
            b[3],
            b[4],
            b[5],
            b[6],
            b[7],
            b[8],
            b[9],
            b[10],
            b[11],
            b[12],
            b[13],
            b[14],
            b[15],
        )
    }
}

fn parse(d: &[u8; 512]) -> NvmeSmart {
    let u128le = |o: usize| u128::from_le_bytes(d[o..o + 16].try_into().unwrap());
    // Composite temperature is Kelvin in the first two bytes after the warning byte.
    let temp_k = u16::from_le_bytes([d[1], d[2]]);
    NvmeSmart {
        temp_c: if temp_k == 0 {
            0.0
        } else {
            temp_k as f32 - 273.15
        },
        available_spare: d[3],
        spare_threshold: d[4],
        percentage_used: d[5],
        critical_warning: d[0],
        data_units_written: u128le(48),
        power_on_hours: u128le(128),
        media_errors: u128le(160),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_decodes_log_page_by_offset() {
        let mut d = [0u8; 512];
        d[0] = 0; // no critical warning
        d[1..3].copy_from_slice(&320u16.to_le_bytes()); // 320 K = 46.85 °C
        d[3] = 100; // available spare %
        d[4] = 10; // spare threshold %
        d[5] = 7; // percentage used
        d[48..64].copy_from_slice(&2_000_000u128.to_le_bytes()); // data units written
        d[128..144].copy_from_slice(&1500u128.to_le_bytes()); // power-on hours
        d[160..176].copy_from_slice(&3u128.to_le_bytes()); // media errors

        let s = parse(&d);
        assert!((s.temp_c - 46.85).abs() < 0.1);
        assert_eq!(s.available_spare, 100);
        assert_eq!(s.spare_threshold, 10);
        assert_eq!(s.percentage_used, 7);
        assert_eq!(s.power_on_hours, 1500);
        assert_eq!(s.media_errors, 3);
        // 2e6 units * 1000 * 512 bytes = 1.024e12 bytes = 1.024 TB.
        assert!((s.tbw() - 1.024).abs() < 1e-3);
    }

    #[test]
    fn zero_temperature_reads_as_zero_not_negative_kelvin() {
        let s = parse(&[0u8; 512]);
        assert_eq!(s.temp_c, 0.0);
    }
}
