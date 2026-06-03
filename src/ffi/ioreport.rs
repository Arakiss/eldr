//! IOReport — the private power/performance telemetry framework. Hand-written FFI,
//! reimplemented from macmon's proven map (MIT). This is the heart of Eldr's no-sudo
//! readings: package/CPU/GPU/ANE power and per-cluster frequency residencies.
//!
//! Flow: build a subscription over channel groups -> sample t0, sleep, sample t1 ->
//! delta -> iterate channels, reading integer values (energy in mJ over the window)
//! and state residencies (active-frequency per cluster).

use crate::ffi::cf::{
    CFDictionaryCreateMutableCopy, CFDictionaryGetCount, CFDictionaryRef, CFMutableDictionaryRef,
    CFRelease, CFStringRef, CFTypeRef, cfdict_get_val, cfstr, from_cfstr,
};
use crate::ffi::cf::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
use core::ffi::c_void;
use std::ptr;

type CVoidRef = *const c_void;

#[repr(C)]
struct IOReportSubscription {
    _data: [u8; 0],
}
type IOReportSubscriptionRef = *const IOReportSubscription;

#[link(name = "IOReport", kind = "dylib")]
unsafe extern "C" {
    fn IOReportCopyChannelsInGroup(
        group: CFStringRef,
        subgroup: CFStringRef,
        c: u64,
        d: u64,
        e: u64,
    ) -> CFDictionaryRef;
    fn IOReportMergeChannels(a: CFDictionaryRef, b: CFDictionaryRef, nil: CFTypeRef);
    fn IOReportCreateSubscription(
        a: CVoidRef,
        b: CFMutableDictionaryRef,
        c: *mut CFMutableDictionaryRef,
        d: u64,
        e: CFTypeRef,
    ) -> IOReportSubscriptionRef;
    fn IOReportCreateSamples(
        a: IOReportSubscriptionRef,
        b: CFMutableDictionaryRef,
        c: CFTypeRef,
    ) -> CFDictionaryRef;
    fn IOReportCreateSamplesDelta(
        a: CFDictionaryRef,
        b: CFDictionaryRef,
        c: CFTypeRef,
    ) -> CFDictionaryRef;
    fn IOReportChannelGetGroup(a: CFDictionaryRef) -> CFStringRef;
    fn IOReportChannelGetSubGroup(a: CFDictionaryRef) -> CFStringRef;
    fn IOReportChannelGetChannelName(a: CFDictionaryRef) -> CFStringRef;
    fn IOReportSimpleGetIntegerValue(a: CFDictionaryRef, b: i32) -> i64;
    fn IOReportChannelGetUnitLabel(a: CFDictionaryRef) -> CFStringRef;
    fn IOReportStateGetCount(a: CFDictionaryRef) -> i32;
    fn IOReportStateGetNameForIndex(a: CFDictionaryRef, b: i32) -> CFStringRef;
    fn IOReportStateGetResidency(a: CFDictionaryRef, b: i32) -> i64;
}

// MARK: channel building

/// Build a merged, mutable channel dict for the given `(group, subgroup)` pairs.
fn build_channels(items: &[(&str, Option<&str>)]) -> Option<CFMutableDictionaryRef> {
    if items.is_empty() {
        return None;
    }
    let mut channels: Vec<CFDictionaryRef> = Vec::with_capacity(items.len());
    for (group, subgroup) in items {
        let gname = cfstr(group);
        let sname = subgroup.map_or(ptr::null(), cfstr);
        let chan = unsafe { IOReportCopyChannelsInGroup(gname, sname, 0, 0, 0) };
        unsafe {
            CFRelease(gname);
            if !sname.is_null() {
                CFRelease(sname);
            }
        }
        if chan.is_null() {
            // release what we have and bail
            for c in &channels {
                unsafe { CFRelease(*c) };
            }
            return None;
        }
        channels.push(chan);
    }

    let base = channels[0];
    for c in channels.iter().skip(1) {
        unsafe { IOReportMergeChannels(base, *c, ptr::null()) };
    }

    let size = unsafe { CFDictionaryGetCount(base) };
    let merged = unsafe { CFDictionaryCreateMutableCopy(ptr::null(), size, base) };

    for c in &channels {
        unsafe { CFRelease(*c) };
    }

    if merged.is_null() || cfdict_get_val(merged, "IOReportChannels").is_none() {
        if !merged.is_null() {
            unsafe { CFRelease(merged as CFDictionaryRef) };
        }
        return None;
    }
    Some(merged)
}

// MARK: IOReport handle

pub struct IOReport {
    subs: IOReportSubscriptionRef,
    chan: CFMutableDictionaryRef,
}

impl IOReport {
    /// Subscribe to the given channel groups. `subgroup = None` selects the whole
    /// group. Returns `None` if the private framework is unavailable or channels
    /// cannot be built.
    pub fn new(channels: &[(&str, Option<&str>)]) -> Option<Self> {
        let chan = build_channels(channels)?;
        let mut subbed: CFMutableDictionaryRef = ptr::null_mut();
        let subs =
            unsafe { IOReportCreateSubscription(ptr::null(), chan, &mut subbed, 0, ptr::null()) };
        if subs.is_null() {
            unsafe { CFRelease(chan as CFDictionaryRef) };
            return None;
        }
        Some(IOReport { subs, chan })
    }

    /// Sample over `duration_ms`: snapshot, sleep, snapshot, delta. The returned
    /// iterator owns the delta dict and yields one item per channel.
    pub fn sample(&self, duration_ms: u64) -> IOReportIterator {
        unsafe {
            let s1 = IOReportCreateSamples(self.subs, self.chan, ptr::null());
            std::thread::sleep(std::time::Duration::from_millis(duration_ms));
            let s2 = IOReportCreateSamples(self.subs, self.chan, ptr::null());
            let delta = IOReportCreateSamplesDelta(s1, s2, ptr::null());
            CFRelease(s1);
            CFRelease(s2);
            IOReportIterator::new(delta)
        }
    }
}

impl Drop for IOReport {
    fn drop(&mut self) {
        unsafe {
            CFRelease(self.chan as CFDictionaryRef);
            CFRelease(self.subs as CFTypeRef);
        }
    }
}

// MARK: channel iterator

pub struct IOReportIterator {
    sample: CFDictionaryRef,
    items: CFArrayRef,
    items_size: isize,
    index: isize,
}

impl IOReportIterator {
    fn new(sample: CFDictionaryRef) -> Self {
        let items = cfdict_get_val(sample, "IOReportChannels").unwrap_or(ptr::null()) as CFArrayRef;
        let items_size = if items.is_null() {
            0
        } else {
            unsafe { CFArrayGetCount(items) }
        };
        IOReportIterator { sample, items, items_size, index: 0 }
    }
}

impl Drop for IOReportIterator {
    fn drop(&mut self) {
        if !self.sample.is_null() {
            unsafe { CFRelease(self.sample) };
        }
    }
}

/// One IOReport channel within a delta sample.
pub struct Channel {
    pub group: String,
    pub subgroup: String,
    pub channel: String,
    pub unit: String,
    /// Borrowed dict pointer into the sample; valid until the iterator drops.
    pub item: CFDictionaryRef,
}

impl Iterator for IOReportIterator {
    type Item = Channel;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.items_size {
            return None;
        }
        let item =
            unsafe { CFArrayGetValueAtIndex(self.items, self.index) } as CFDictionaryRef;
        self.index += 1;
        let group = cfstr_field(unsafe { IOReportChannelGetGroup(item) });
        let subgroup = cfstr_field(unsafe { IOReportChannelGetSubGroup(item) });
        let channel = cfstr_field(unsafe { IOReportChannelGetChannelName(item) });
        let unit = cfstr_field(unsafe { IOReportChannelGetUnitLabel(item) })
            .trim()
            .to_string();
        Some(Channel { group, subgroup, channel, unit, item })
    }
}

fn cfstr_field(s: CFStringRef) -> String {
    if s.is_null() { String::new() } else { from_cfstr(s) }
}

// MARK: per-channel readers

/// State residencies for a channel: `(state_name, residency)` pairs.
pub fn residencies(item: CFDictionaryRef) -> Vec<(String, i64)> {
    let count = unsafe { IOReportStateGetCount(item) };
    let mut out = Vec::with_capacity(count.max(0) as usize);
    for i in 0..count {
        let name = from_cfstr(unsafe { IOReportStateGetNameForIndex(item, i) });
        let val = unsafe { IOReportStateGetResidency(item, i) };
        out.push((name, val));
    }
    out
}

/// Convert an energy channel to average Watts over `duration_ms`. The integer value
/// is accumulated energy in the channel's unit (mJ/uJ/nJ).
pub fn watts(item: CFDictionaryRef, unit: &str, duration_ms: u64) -> f32 {
    let energy = unsafe { IOReportSimpleGetIntegerValue(item, 0) } as f32;
    let secs = (duration_ms as f32 / 1000.0).max(f32::MIN_POSITIVE);
    let power = energy / secs;
    match unit {
        "mJ" => power / 1e3,
        "uJ" => power / 1e6,
        "nJ" => power / 1e9,
        _ => 0.0,
    }
}
