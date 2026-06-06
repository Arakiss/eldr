//! IOKit service matching + registry property reads, hand-written (no `io-kit-sys`).
//! Shared infrastructure: SoC frequency discovery reads the `pmgr` entry's
//! voltage-states; the SMC client (M2) opens `AppleSMCKeysEndpoint`.

use crate::ffi::cf::{
    self, CFAllocatorRef, CFDictionaryRef, CFMutableDictionaryRef, CFStringRef, CFTypeRef,
};
use core::ffi::c_char;
use std::ptr;

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOServiceMatching(name: *const c_char) -> CFMutableDictionaryRef;
    fn IOServiceGetMatchingServices(
        main_port: u32,
        matching: CFDictionaryRef,
        existing: *mut u32,
    ) -> i32;
    fn IOIteratorNext(iterator: u32) -> u32;
    fn IORegistryEntryGetName(entry: u32, name: *mut c_char) -> i32;
    fn IORegistryEntryCreateCFProperties(
        entry: u32,
        properties: *mut CFMutableDictionaryRef,
        allocator: CFAllocatorRef,
        options: u32,
    ) -> i32;
    fn IORegistryEntrySearchCFProperty(
        entry: u32,
        plane: *const c_char,
        key: CFStringRef,
        allocator: CFAllocatorRef,
        options: u32,
    ) -> CFTypeRef;
    pub fn IOObjectRelease(obj: u32) -> u32;
}

/// Iterator over IOKit services matching a class name (e.g. `AppleARMIODevice`).
/// Yields `(entry, name)`; the caller owns each `entry` and must
/// [`IOObjectRelease`] it after use.
pub struct IOServiceIterator {
    existing: u32,
}

impl IOServiceIterator {
    pub fn new(service_name: &str) -> Option<Self> {
        let cname = std::ffi::CString::new(service_name).ok()?;
        unsafe {
            let matching = IOServiceMatching(cname.as_ptr());
            if matching.is_null() {
                return None;
            }
            let mut existing = 0u32;
            // mainPort 0 = kIOMainPortDefault. Consumes `matching`.
            if IOServiceGetMatchingServices(0, matching, &mut existing) != 0 {
                return None;
            }
            Some(IOServiceIterator { existing })
        }
    }
}

impl Drop for IOServiceIterator {
    fn drop(&mut self) {
        unsafe {
            IOObjectRelease(self.existing);
        }
    }
}

impl Iterator for IOServiceIterator {
    type Item = (u32, String);

    fn next(&mut self) -> Option<Self::Item> {
        let next = unsafe { IOIteratorNext(self.existing) };
        if next == 0 {
            return None;
        }
        let mut name = [0 as c_char; 128];
        if unsafe { IORegistryEntryGetName(next, name.as_mut_ptr()) } != 0 {
            unsafe { IOObjectRelease(next) };
            return None;
        }
        let name = unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        Some((next, name))
    }
}

/// Search a registry entry and its children (IOService plane) for `key`, returning the
/// first match. Owned — release with `CFRelease`. Lets a parent device node reach a
/// property that lives in a child, e.g. an IOMedia "BSD Name" or a driver's "Statistics".
pub fn entry_search_property(entry: u32, key: &str) -> Option<CFTypeRef> {
    const K_IO_REGISTRY_ITERATE_RECURSIVELY: u32 = 1;
    let k = cf::cfstr(key);
    let plane = c"IOService";
    let v = unsafe {
        IORegistryEntrySearchCFProperty(
            entry,
            plane.as_ptr(),
            k,
            ptr::null(),
            K_IO_REGISTRY_ITERATE_RECURSIVELY,
        )
    };
    unsafe { cf::CFRelease(k) };
    if v.is_null() { None } else { Some(v) }
}

/// Read a registry entry's CF properties dictionary. Owned — release with
/// `CFRelease`. Returns `None` on failure.
pub fn entry_properties(entry: u32) -> Option<CFDictionaryRef> {
    unsafe {
        let mut props: CFMutableDictionaryRef = ptr::null_mut();
        if IORegistryEntryCreateCFProperties(entry, &mut props, ptr::null(), 0) != 0
            || props.is_null()
        {
            return None;
        }
        Some(props as CFDictionaryRef)
    }
}
