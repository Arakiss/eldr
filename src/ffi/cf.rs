//! CoreFoundation, hand-written (no `core-foundation` crate). Opaque ref types plus
//! the handful of CFString/CFDictionary/CFArray/CFData/CFNumber entry points Eldr
//! needs. All CF "Create"/"Copy" results are owned: release them with [`CFRelease`].

use core::ffi::{c_char, c_void};
use std::ptr;

// CoreFoundation ref types are opaque pointers.
pub type CFTypeRef = *const c_void;
pub type CFAllocatorRef = *const c_void;
pub type CFStringRef = *const c_void;
pub type CFDictionaryRef = *const c_void;
pub type CFMutableDictionaryRef = *mut c_void;
pub type CFArrayRef = *const c_void;
pub type CFDataRef = *const c_void;
pub type CFNumberRef = *const c_void;

/// `CFIndex` is `long` (isize on LP64).
pub type CFIndex = isize;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CFRange {
    pub location: CFIndex,
    pub length: CFIndex,
}

impl CFRange {
    pub fn new(location: CFIndex, length: CFIndex) -> Self {
        CFRange { location, length }
    }
}

// Encodings / number types.
pub const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
pub const K_CF_NUMBER_SINT32_TYPE: CFIndex = 3;
pub const K_CF_NUMBER_SINT64_TYPE: CFIndex = 4;

// CoreFoundation dictionary callback tables (only the addresses are used, passed to
// CFDictionaryCreate). Declared as opaque; we never read their fields.
#[repr(C)]
pub struct CFDictionaryKeyCallBacks {
    _opaque: [u8; 0],
}
#[repr(C)]
pub struct CFDictionaryValueCallBacks {
    _opaque: [u8; 0],
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    pub static kCFTypeDictionaryKeyCallBacks: CFDictionaryKeyCallBacks;
    pub static kCFTypeDictionaryValueCallBacks: CFDictionaryValueCallBacks;

    pub fn CFRelease(cf: CFTypeRef);

    pub fn CFStringCreateWithBytes(
        alloc: CFAllocatorRef,
        bytes: *const u8,
        num_bytes: CFIndex,
        encoding: u32,
        is_external_representation: u8,
    ) -> CFStringRef;
    pub fn CFStringGetCString(
        the_string: CFStringRef,
        buffer: *mut c_char,
        buffer_size: CFIndex,
        encoding: u32,
    ) -> u8;

    pub fn CFNumberCreate(
        alloc: CFAllocatorRef,
        the_type: CFIndex,
        value_ptr: *const c_void,
    ) -> CFNumberRef;
    pub fn CFNumberGetValue(number: CFNumberRef, the_type: CFIndex, value_ptr: *mut c_void) -> u8;

    pub fn CFDictionaryCreate(
        alloc: CFAllocatorRef,
        keys: *const *const c_void,
        values: *const *const c_void,
        num_values: CFIndex,
        key_callbacks: *const CFDictionaryKeyCallBacks,
        value_callbacks: *const CFDictionaryValueCallBacks,
    ) -> CFDictionaryRef;
    pub fn CFDictionaryGetValue(dict: CFDictionaryRef, key: *const c_void) -> *const c_void;
    pub fn CFDictionaryGetCount(dict: CFDictionaryRef) -> CFIndex;
    pub fn CFDictionaryCreateMutableCopy(
        alloc: CFAllocatorRef,
        capacity: CFIndex,
        dict: CFDictionaryRef,
    ) -> CFMutableDictionaryRef;

    pub fn CFArrayGetCount(array: CFArrayRef) -> CFIndex;
    pub fn CFArrayGetValueAtIndex(array: CFArrayRef, idx: CFIndex) -> *const c_void;

    pub fn CFDataGetLength(data: CFDataRef) -> CFIndex;
    pub fn CFDataGetBytes(data: CFDataRef, range: CFRange, buffer: *mut u8);
}

// MARK: helpers

/// Create a CFString by COPYING `s` (so the Rust buffer need not outlive it). The
/// returned ref is owned — release it with [`CFRelease`].
pub fn cfstr(s: &str) -> CFStringRef {
    unsafe {
        CFStringCreateWithBytes(
            ptr::null(),
            s.as_ptr(),
            s.len() as CFIndex,
            K_CF_STRING_ENCODING_UTF8,
            0,
        )
    }
}

/// Decode a CFString into a Rust `String`. Returns empty on null/failure.
pub fn from_cfstr(s: CFStringRef) -> String {
    if s.is_null() {
        return String::new();
    }
    let mut buf = [0i8; 512];
    unsafe {
        if CFStringGetCString(s, buf.as_mut_ptr(), 512, K_CF_STRING_ENCODING_UTF8) == 0 {
            return String::new();
        }
        std::ffi::CStr::from_ptr(buf.as_ptr())
            .to_string_lossy()
            .into_owned()
    }
}

/// Create a signed-32 CFNumber. Owned — release with [`CFRelease`].
pub fn cfnum(val: i32) -> CFNumberRef {
    unsafe {
        CFNumberCreate(
            ptr::null(),
            K_CF_NUMBER_SINT32_TYPE,
            &val as *const i32 as *const c_void,
        )
    }
}

/// Look up `key` in a dictionary. The returned ref is BORROWED (do not release).
pub fn cfdict_get_val(dict: CFDictionaryRef, key: &str) -> Option<*const c_void> {
    if dict.is_null() {
        return None;
    }
    unsafe {
        let k = cfstr(key);
        let v = CFDictionaryGetValue(dict, k);
        CFRelease(k);
        if v.is_null() { None } else { Some(v) }
    }
}

/// Read a CFNumber as `i64`. Returns `None` on null/failure.
pub fn cfnum_i64(number: CFNumberRef) -> Option<i64> {
    if number.is_null() {
        return None;
    }
    let mut v: i64 = 0;
    let ok = unsafe {
        CFNumberGetValue(
            number,
            K_CF_NUMBER_SINT64_TYPE,
            &mut v as *mut i64 as *mut c_void,
        )
    };
    if ok == 0 { None } else { Some(v) }
}

/// Copy the raw bytes of a CFData object into a `Vec<u8>`.
pub fn cfdata_bytes(data: CFDataRef) -> Vec<u8> {
    if data.is_null() {
        return Vec::new();
    }
    unsafe {
        let len = CFDataGetLength(data);
        if len <= 0 {
            return Vec::new();
        }
        let mut buf = vec![0u8; len as usize];
        CFDataGetBytes(data, CFRange::new(0, len), buf.as_mut_ptr());
        buf
    }
}
