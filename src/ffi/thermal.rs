//! macOS thermal pressure via the Objective-C runtime:
//! `[[NSProcessInfo processInfo] thermalState]`.
//!
//! This is the clean throttle signal the bash prototype gates on (the SMC die temp
//! reads high on healthy hardware, so it informs but never triggers). We call the
//! bare objc runtime — `objc_msgSend` is transmuted to the exact signature per call,
//! the standard Rust approach. Linking Foundation registers `NSProcessInfo`.

use core::ffi::{c_char, c_void};
use std::ffi::CString;

type Id = *const c_void;
type Sel = *const c_void;
type Class = *const c_void;

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> Class;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_msgSend();
}

// Force the Foundation framework to load so `NSProcessInfo` is registered.
#[link(name = "Foundation", kind = "framework")]
unsafe extern "C" {}

/// Read `NSProcessInfoThermalState` (0=nominal..3=critical). Returns `None` if the
/// runtime call fails (e.g. Foundation unavailable), which callers map to Unknown.
pub fn thermal_state_raw() -> Option<i64> {
    unsafe {
        let cls_name = CString::new("NSProcessInfo").ok()?;
        let cls = objc_getClass(cls_name.as_ptr());
        if cls.is_null() {
            return None;
        }
        let sel_pi = CString::new("processInfo").ok()?;
        let sel_ts = CString::new("thermalState").ok()?;
        let process_info = sel_registerName(sel_pi.as_ptr());
        let thermal_state = sel_registerName(sel_ts.as_ptr());

        // [NSProcessInfo processInfo] -> id
        let msg_send_id: extern "C" fn(Id, Sel) -> Id =
            std::mem::transmute(objc_msgSend as *const ());
        let pi = msg_send_id(cls, process_info);
        if pi.is_null() {
            return None;
        }

        // [processInfo thermalState] -> NSInteger (i64 on LP64)
        let msg_send_int: extern "C" fn(Id, Sel) -> i64 =
            std::mem::transmute(objc_msgSend as *const ());
        Some(msg_send_int(pi, thermal_state))
    }
}
