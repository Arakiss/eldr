//! macOS notifications for long-running daemon work.
//!
//! Prefer a native notification from the current process, so a guard launched from
//! `Eldr.app` is attributed to that bundle and uses the app icon. Apple deprecated
//! `NSUserNotification` in favor of UserNotifications, but this zero-dependency path is
//! still the smallest synchronous bridge for a background CLI daemon; `osascript` remains
//! the fallback.

use core::ffi::{c_char, c_void};
use std::ffi::CString;

type Id = *mut c_void;
type Sel = *const c_void;
type Class = *const c_void;
type ObjCBool = i8;

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> Class;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_msgSend();
}

// Force Foundation to load so NSUserNotification and NSString are registered.
#[link(name = "Foundation", kind = "framework")]
unsafe extern "C" {}

/// Deliver a passive notification. It intentionally has no sound: Eldr is a monitor, not
/// an alarm siren.
pub fn send(title: &str, body: &str) {
    send_coalesced(None, title, body);
}

/// Deliver a passive notification, replacing any already-delivered notification with the
/// same id on the native path.
pub fn send_coalesced(identifier: Option<&str>, title: &str, body: &str) {
    if !send_native(identifier, title, body) {
        send_osascript(title, body);
    }
}

fn send_native(identifier: Option<&str>, title: &str, body: &str) -> bool {
    unsafe {
        let Some(pool) = AutoreleasePool::new() else {
            return false;
        };
        let Some(notification) = alloc_init("NSUserNotification") else {
            return false;
        };
        let Some(center) = default_notification_center() else {
            return false;
        };
        let Some(title) = ns_string(&clean_text(title, 96)) else {
            return false;
        };
        let Some(body) = ns_string(&clean_text(body, 220)) else {
            return false;
        };

        set_object(notification, "setTitle:", title);
        set_object(notification, "setInformativeText:", body);
        set_bool(notification, "setHasActionButton:", false);
        if let Some(identifier) = identifier
            && let Some(id) = ns_string(&clean_text(identifier, 96))
        {
            set_object(notification, "setIdentifier:", id);
        }
        send_object(center, "deliverNotification:", notification);
        drop(pool);
        true
    }
}

struct AutoreleasePool(Id);

impl AutoreleasePool {
    unsafe fn new() -> Option<Self> {
        unsafe { alloc_init("NSAutoreleasePool") }.map(Self)
    }
}

impl Drop for AutoreleasePool {
    fn drop(&mut self) {
        unsafe {
            send_void(self.0, "drain");
        }
    }
}

unsafe fn default_notification_center() -> Option<Id> {
    let cls = unsafe { class("NSUserNotificationCenter") }?;
    let sel = unsafe { selector("defaultUserNotificationCenter") }?;
    let msg: extern "C" fn(Class, Sel) -> Id =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    let center = msg(cls, sel);
    (!center.is_null()).then_some(center)
}

unsafe fn alloc_init(name: &str) -> Option<Id> {
    let cls = unsafe { class(name) }?;
    let alloc = unsafe { selector("alloc") }?;
    let init = unsafe { selector("init") }?;
    let msg_class: extern "C" fn(Class, Sel) -> Id =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    let msg_id: extern "C" fn(Id, Sel) -> Id =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    let allocated = msg_class(cls, alloc);
    if allocated.is_null() {
        return None;
    }
    let obj = msg_id(allocated, init);
    (!obj.is_null()).then_some(obj)
}

unsafe fn ns_string(s: &str) -> Option<Id> {
    let cls = unsafe { class("NSString") }?;
    let sel = unsafe { selector("stringWithUTF8String:") }?;
    let c = CString::new(s).ok()?;
    let msg: extern "C" fn(Class, Sel, *const c_char) -> Id =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    let obj = msg(cls, sel, c.as_ptr());
    (!obj.is_null()).then_some(obj)
}

unsafe fn class(name: &str) -> Option<Class> {
    let c = CString::new(name).ok()?;
    let cls = unsafe { objc_getClass(c.as_ptr()) };
    (!cls.is_null()).then_some(cls)
}

unsafe fn selector(name: &str) -> Option<Sel> {
    let c = CString::new(name).ok()?;
    let sel = unsafe { sel_registerName(c.as_ptr()) };
    (!sel.is_null()).then_some(sel)
}

unsafe fn set_object(obj: Id, sel: &str, val: Id) {
    if let Some(sel) = unsafe { selector(sel) } {
        let msg: extern "C" fn(Id, Sel, Id) =
            unsafe { std::mem::transmute(objc_msgSend as *const ()) };
        msg(obj, sel, val);
    }
}

unsafe fn set_bool(obj: Id, sel: &str, val: bool) {
    if let Some(sel) = unsafe { selector(sel) } {
        let msg: extern "C" fn(Id, Sel, ObjCBool) =
            unsafe { std::mem::transmute(objc_msgSend as *const ()) };
        msg(obj, sel, if val { 1 } else { 0 });
    }
}

unsafe fn send_object(obj: Id, sel: &str, val: Id) {
    if let Some(sel) = unsafe { selector(sel) } {
        let msg: extern "C" fn(Id, Sel, Id) =
            unsafe { std::mem::transmute(objc_msgSend as *const ()) };
        msg(obj, sel, val);
    }
}

unsafe fn send_void(obj: Id, sel: &str) {
    if let Some(sel) = unsafe { selector(sel) } {
        let msg: extern "C" fn(Id, Sel) = unsafe { std::mem::transmute(objc_msgSend as *const ()) };
        msg(obj, sel);
    }
}

fn send_osascript(title: &str, body: &str) {
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        osa_escape(&clean_text(body, 220)),
        osa_escape(&clean_text(title, 96))
    );
    let _ = std::process::Command::new("osascript")
        .args(["-e", &script])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

fn clean_text(s: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut last_space = false;
    let mut len = 0;
    for c in s.chars() {
        if len >= max_chars {
            break;
        }
        let c = if c == '\0' || c.is_control() { ' ' } else { c };
        if c.is_whitespace() {
            if !last_space {
                out.push(' ');
                last_space = true;
                len += 1;
            }
        } else {
            out.push(c);
            last_space = false;
            len += 1;
        }
    }
    out.trim().to_string()
}

fn osa_escape(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\\' => "\\\\".to_string(),
            '"' => "\\\"".to_string(),
            c if (c as u32) < 0x20 => " ".to_string(),
            c => c.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_and_truncates_notification_text() {
        assert_eq!(clean_text("  hi\n\tthere\0now  ", 20), "hi there now");
        assert_eq!(clean_text("abcdef", 3), "abc");
    }

    #[test]
    fn escapes_apple_script_literals() {
        assert_eq!(
            osa_escape("a \"quoted\" path\\x"),
            "a \\\"quoted\\\" path\\\\x"
        );
    }
}
