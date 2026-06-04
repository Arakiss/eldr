//! IOHID temperature sensors, hand-written FFI (provenance in NOTICE). Reads
//! AppleVendor temperature sensors with no sudo. CPU temp is the mean of the
//! `pACC`/`eACC MTR` sensors; GPU temp the mean of `GPU MTR` sensors.

use crate::ffi::cf::{CFAllocatorRef, CFTypeRef};
use crate::ffi::cf::{
    CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef, CFDictionaryCreate, CFDictionaryRef,
    CFRelease, CFStringRef, cfnum, cfstr, from_cfstr, kCFTypeDictionaryKeyCallBacks,
    kCFTypeDictionaryValueCallBacks,
};
use core::ffi::c_void;
use std::ptr;

#[repr(C)]
struct OpaqueClient(c_void);
#[repr(C)]
struct OpaqueService(c_void);
#[repr(C)]
struct OpaqueEvent(c_void);
type IOHIDEventSystemClientRef = *const OpaqueClient;
type IOHIDServiceClientRef = *const OpaqueService;
type IOHIDEventRef = *const OpaqueEvent;

const K_HID_PAGE_APPLE_VENDOR: i32 = 0xff00;
const K_HID_USAGE_APPLE_VENDOR_TEMPERATURE_SENSOR: i32 = 0x0005;
const K_IOHID_EVENT_TYPE_TEMPERATURE: i64 = 15;

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOHIDEventSystemClientCreate(allocator: CFAllocatorRef) -> IOHIDEventSystemClientRef;
    fn IOHIDEventSystemClientSetMatching(
        client: IOHIDEventSystemClientRef,
        m: CFDictionaryRef,
    ) -> i32;
    fn IOHIDEventSystemClientCopyServices(client: IOHIDEventSystemClientRef) -> CFArrayRef;
    fn IOHIDServiceClientCopyProperty(s: IOHIDServiceClientRef, key: CFStringRef) -> CFStringRef;
    fn IOHIDServiceClientCopyEvent(
        s: IOHIDServiceClientRef,
        ty: i64,
        a: i32,
        b: i64,
    ) -> IOHIDEventRef;
    fn IOHIDEventGetFloatValue(event: IOHIDEventRef, field: i64) -> f64;
}

/// Holds the (long-lived) sensor matching dictionary.
pub struct HidTemps {
    matching: CFDictionaryRef,
}

impl HidTemps {
    pub fn new() -> Option<Self> {
        let keys = [cfstr("PrimaryUsagePage"), cfstr("PrimaryUsage")];
        let nums = [
            cfnum(K_HID_PAGE_APPLE_VENDOR),
            cfnum(K_HID_USAGE_APPLE_VENDOR_TEMPERATURE_SENSOR),
        ];
        let dict = unsafe {
            CFDictionaryCreate(
                ptr::null(),
                keys.as_ptr(),
                nums.as_ptr(),
                2,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            )
        };
        for k in keys {
            unsafe { CFRelease(k) };
        }
        for n in nums {
            unsafe { CFRelease(n) };
        }
        if dict.is_null() {
            return None;
        }
        Some(HidTemps { matching: dict })
    }

    /// Read all matching temperature sensors as `(name, celsius)`.
    pub fn read(&self) -> Vec<(String, f32)> {
        let mut out = Vec::new();
        unsafe {
            let client = IOHIDEventSystemClientCreate(ptr::null());
            if client.is_null() {
                return out;
            }
            IOHIDEventSystemClientSetMatching(client, self.matching);
            let services = IOHIDEventSystemClientCopyServices(client);
            if services.is_null() {
                CFRelease(client as CFTypeRef);
                return out;
            }
            let product_key = cfstr("Product");
            let count = CFArrayGetCount(services);
            for i in 0..count {
                let sc = CFArrayGetValueAtIndex(services, i) as IOHIDServiceClientRef;
                if sc.is_null() {
                    continue;
                }
                let name_ref = IOHIDServiceClientCopyProperty(sc, product_key);
                if name_ref.is_null() {
                    continue;
                }
                let name = from_cfstr(name_ref);
                CFRelease(name_ref as CFTypeRef);

                let event = IOHIDServiceClientCopyEvent(sc, K_IOHID_EVENT_TYPE_TEMPERATURE, 0, 0);
                if event.is_null() {
                    continue;
                }
                let temp =
                    IOHIDEventGetFloatValue(event, K_IOHID_EVENT_TYPE_TEMPERATURE << 16) as f32;
                CFRelease(event as CFTypeRef);
                if temp > 0.0 && temp <= 150.0 {
                    out.push((name, temp));
                }
            }
            CFRelease(product_key as CFTypeRef);
            CFRelease(services as CFTypeRef);
            CFRelease(client as CFTypeRef);
        }
        out
    }
}

impl Drop for HidTemps {
    fn drop(&mut self) {
        unsafe { CFRelease(self.matching) };
    }
}

/// CPU/GPU temperature averages in Celsius. Returns `(0,0)` if sensors are absent.
pub fn temps() -> (f32, f32) {
    let Some(hid) = HidTemps::new() else {
        return (0.0, 0.0);
    };
    let metrics = hid.read();
    let mut cpu = Vec::new();
    let mut gpu = Vec::new();
    for (name, val) in &metrics {
        if name.starts_with("pACC MTR Temp Sensor") || name.starts_with("eACC MTR Temp Sensor") {
            cpu.push(*val);
        } else if name.starts_with("GPU MTR Temp Sensor") {
            gpu.push(*val);
        }
    }
    let avg = |v: &[f32]| {
        if v.is_empty() {
            0.0
        } else {
            v.iter().sum::<f32>() / v.len() as f32
        }
    };
    (avg(&cpu), avg(&gpu))
}
