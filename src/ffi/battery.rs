//! Battery telemetry from the IORegistry `AppleSmartBattery` service — the same no-sudo
//! source `pmset` reads. One service holds the live state (charge, AC, charging, time,
//! amperage/voltage) and the health figures (cycles, current vs design capacity), so a
//! single `entry_properties` read covers everything. Returns `None` on a desktop Mac
//! with no internal battery.

use crate::ffi::cf::{CFRelease, cfbool, cfnum_i64};
use crate::ffi::iokit::{IOServiceIterator, entry_properties};

/// A coherent battery reading. `None` from [`read`] means no internal battery.
#[derive(Default, Clone, Copy, Debug)]
pub struct Battery {
    /// Charge percentage, 0–100.
    pub percent: u8,
    pub charging: bool,
    pub on_ac: bool,
    pub fully_charged: bool,
    /// Minutes to empty (on battery) or to full (charging); `None` while macOS is still
    /// estimating (it reports a sentinel until the rate settles).
    pub time_min: Option<u32>,
    /// Instantaneous power flow in watts: negative draining the battery, positive charging.
    pub power_w: f32,
    pub temp_c: f32,
    /// Charge cycles, if reported.
    pub cycles: Option<u32>,
    /// Current full capacity as a percentage of the original design capacity — the honest
    /// "battery health" figure. `None` if the capacities aren't reported.
    pub health_pct: Option<u8>,
}

/// Read the internal battery, or `None` if there isn't one (Mac mini/Studio, or SMC
/// unavailable).
pub fn read() -> Option<Battery> {
    for (device, _name) in IOServiceIterator::new("AppleSmartBattery")? {
        let props = entry_properties(device);
        unsafe { crate::ffi::iokit::IOObjectRelease(device) };
        let Some(props) = props else { continue };

        let geti = |k: &str| {
            crate::ffi::cf::cfdict_get_val(props, k)
                .and_then(|p| cfnum_i64(p as crate::ffi::cf::CFNumberRef))
        };
        let getb = |k: &str| {
            crate::ffi::cf::cfdict_get_val(props, k)
                .map(cfbool)
                .unwrap_or(false)
        };

        if !getb("BatteryInstalled") {
            unsafe { CFRelease(props) };
            return None;
        }

        // CurrentCapacity is already a 0–100 percentage on Apple Silicon (MaxCapacity = 100).
        let percent = geti("CurrentCapacity").unwrap_or(0).clamp(0, 100) as u8;
        let charging = getb("IsCharging");
        let on_ac = getb("ExternalConnected");
        let fully_charged = getb("FullyCharged");

        // Amperage is signed milliamps (negative = discharging); Voltage is millivolts.
        let amperage = geti("InstantAmperage")
            .or_else(|| geti("Amperage"))
            .unwrap_or(0);
        let voltage = geti("Voltage").unwrap_or(0);
        let power_w = (amperage as f32 / 1000.0) * (voltage as f32 / 1000.0);

        // The relevant time field depends on direction; macOS reports 65535 while estimating.
        let raw_time = if charging {
            geti("AvgTimeToFull")
        } else {
            geti("TimeRemaining").or_else(|| geti("AvgTimeToEmpty"))
        }
        .unwrap_or(65535);
        let time_min = if (0..1440).contains(&raw_time) {
            Some(raw_time as u32)
        } else {
            None
        };

        let cycles = geti("CycleCount").filter(|&c| c > 0).map(|c| c as u32);

        // Health = current full capacity vs the original design capacity.
        let design = geti("DesignCapacity").unwrap_or(0);
        let max_now = geti("AppleRawMaxCapacity")
            .or_else(|| geti("NominalChargeCapacity"))
            .unwrap_or(0);
        let health_pct = if design > 0 && max_now > 0 {
            Some(
                ((max_now as f64 / design as f64) * 100.0)
                    .round()
                    .clamp(0.0, 100.0) as u8,
            )
        } else {
            None
        };

        let temp_c = geti("Temperature").map(|t| t as f32 / 100.0).unwrap_or(0.0);

        unsafe { CFRelease(props) };
        return Some(Battery {
            percent,
            charging,
            on_ac,
            fully_charged,
            time_min,
            power_w,
            temp_c,
            cycles,
            health_pct,
        });
    }
    None
}
