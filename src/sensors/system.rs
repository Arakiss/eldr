//! Static machine identity for `eldr system`: marketing model, model id, serial,
//! macOS version/build, CPU brand + P/E topology, RAM and the internal SSD. All
//! read-only and no-sudo — sysctl (via `mach`) plus the IOKit/CoreFoundation readers
//! eldr already ships. `SystemInfo::get()` is infallible: every field defaults to
//! empty/0 so a missing read never breaks the view.

use crate::ffi::cf;
use crate::ffi::iokit::{IOObjectRelease, IOServiceIterator, entry_properties};
use crate::ffi::mach;
use crate::ui::style::{Style, gib};

#[derive(Default, Debug)]
pub struct SystemInfo {
    pub marketing: String, // "Mac mini"
    pub model_id: String,  // "Mac16,11"
    pub serial: String,
    pub os_version: String, // "26.4.1"
    pub os_build: String,   // "25E253"
    pub arch: String,       // "arm64"
    pub chip: String,       // "Apple M4 Pro"
    pub p_cores: u64,
    pub e_cores: u64,
    pub logical_cpu: u64,
    pub ram_bytes: u64,
    pub uptime_secs: u64,
    pub ssd_model: String,  // "APPLE SSD AP0512Z"
    pub ssd_bytes: u64,     // raw capacity
    pub ssd_medium: String, // "NVMe · Internal"
}

impl SystemInfo {
    pub fn get() -> Self {
        let mut s = SystemInfo::default();
        s.model_id = mach::sysctl_string("hw.model").unwrap_or_default();
        s.marketing = marketing_name(&s.model_id);
        s.chip = mach::sysctl_string("machdep.cpu.brand_string").unwrap_or_default();
        s.os_version = mach::sysctl_string("kern.osproductversion").unwrap_or_default();
        s.os_build = mach::sysctl_string("kern.osversion").unwrap_or_default();
        s.arch = mach::sysctl_string("hw.machine").unwrap_or_default();
        s.ram_bytes = mach::ram_total();
        s.uptime_secs = mach::uptime_secs();
        s.logical_cpu = mach::sysctl_u64("hw.logicalcpu").unwrap_or(0);

        // Performance/efficiency split (perflevel0 = Performance, 1 = Efficiency).
        if mach::sysctl_u64("hw.nperflevels").unwrap_or(1) >= 2 {
            s.p_cores = mach::sysctl_u64("hw.perflevel0.logicalcpu").unwrap_or(0);
            s.e_cores = mach::sysctl_u64("hw.perflevel1.logicalcpu").unwrap_or(0);
        } else {
            s.p_cores = s.logical_cpu;
        }

        s.serial = read_serial();
        let (model, bytes, medium) = read_ssd();
        s.ssd_model = model;
        s.ssd_bytes = bytes;
        s.ssd_medium = medium;
        s
    }

    /// `eldr system` — a labeled identity panel.
    pub fn render(&self) {
        let st = Style::detect();
        let d = st.dim;
        let z = st.reset;
        let b = st.bold;
        let row = |label: &str, value: String| {
            if !value.trim().is_empty() {
                println!("  {d}{label:<8}{z}{b}{value}{z}");
            }
        };

        println!();
        let model = if self.marketing.is_empty() {
            self.model_id.clone()
        } else if self.model_id.is_empty() {
            self.marketing.clone()
        } else {
            format!("{} {d}({}){z}", self.marketing, self.model_id)
        };
        row("Model", model);

        let cores = if self.e_cores > 0 {
            format!("{}P+{}E", self.p_cores, self.e_cores)
        } else if self.p_cores > 0 {
            format!("{} cores", self.p_cores)
        } else {
            String::new()
        };
        let chip = if cores.is_empty() {
            self.chip.clone()
        } else {
            format!("{} {d}·{z} {cores}", self.chip)
        };
        row("Chip", chip);

        let os = if self.os_version.is_empty() {
            String::new()
        } else if self.os_build.is_empty() {
            self.os_version.clone()
        } else {
            format!("{} {d}({}){z}", self.os_version, self.os_build)
        };
        row("macOS", os);

        if self.ram_bytes > 0 {
            row("Memory", format!("{:.0} GB", gib(self.ram_bytes)));
        }

        if !self.ssd_model.is_empty() {
            let cap = if self.ssd_bytes > 0 {
                format!(" {d}·{z} {} GB", self.ssd_bytes / 1_000_000_000)
            } else {
                String::new()
            };
            let medium = if self.ssd_medium.is_empty() {
                String::new()
            } else {
                format!(" {d}·{z} {}", self.ssd_medium)
            };
            row("Storage", format!("{}{cap}{medium}", self.ssd_model.trim()));
        }

        row("Arch", self.arch.clone());
        row("Up", fmt_uptime(self.uptime_secs));
        if !self.serial.is_empty() {
            row("Serial", self.serial.clone());
        }
        println!();
    }
}

/// Marketing name from the model identifier (no IOKit needed). Falls back to "" for
/// models not in the table, in which case the renderer shows the identifier alone.
fn marketing_name(model_id: &str) -> String {
    let name = match model_id {
        "Mac16,10" | "Mac16,11" | "Mac14,3" | "Mac14,12" => "Mac mini",
        "Mac16,1" | "Mac16,5" | "Mac16,6" | "Mac16,7" | "Mac16,8" => "MacBook Pro",
        "Mac16,12" | "Mac16,13" | "Mac14,2" | "Mac14,15" => "MacBook Air",
        "Mac16,2" | "Mac16,3" | "Mac15,4" | "Mac15,5" => "iMac",
        "Mac15,3" | "Mac15,6" | "Mac15,7" | "Mac15,8" | "Mac15,9" | "Mac15,10" | "Mac15,11" => {
            "MacBook Pro"
        }
        "Mac15,12" | "Mac15,13" => "MacBook Air",
        "Mac14,13" | "Mac14,14" => "Mac Studio",
        "Mac14,8" => "Mac Pro",
        s if s.starts_with("Macmini") => "Mac mini",
        s if s.starts_with("MacBookPro") => "MacBook Pro",
        s if s.starts_with("MacBookAir") => "MacBook Air",
        s if s.starts_with("iMac") => "iMac",
        s if s.starts_with("MacPro") => "Mac Pro",
        s if s.starts_with("MacStudio") || s.starts_with("Mac13,") => "Mac Studio",
        _ => "",
    };
    name.to_string()
}

/// Platform serial number from `IOPlatformExpertDevice` (CFString property).
fn read_serial() -> String {
    let Some(it) = IOServiceIterator::new("IOPlatformExpertDevice") else {
        return String::new();
    };
    for (entry, _name) in it {
        let serial = entry_properties(entry)
            .map(|props| {
                let s = cf::cfdict_get_val(props, "IOPlatformSerialNumber")
                    .map(cf::from_cfstr)
                    .unwrap_or_default();
                unsafe { cf::CFRelease(props) };
                s
            })
            .unwrap_or_default();
        unsafe { IOObjectRelease(entry) };
        if !serial.is_empty() {
            return serial;
        }
    }
    String::new()
}

/// Internal SSD model, raw capacity (bytes) and medium, from the NVMe controller node.
/// Tries the concrete Apple controller classes seen on Apple Silicon, then a generic
/// match; returns empties when none is found.
fn read_ssd() -> (String, u64, String) {
    for class in [
        "AppleANS3CGv2Controller",
        "AppleANS3NVMeController",
        "AppleANS2Controller",
        "IONVMeController",
    ] {
        let Some(it) = IOServiceIterator::new(class) else {
            continue;
        };
        for (entry, _name) in it {
            let found = entry_properties(entry).and_then(|props| {
                let model = cf::cfdict_get_val(props, "Model Number")
                    .map(cf::from_cfstr)
                    .unwrap_or_default();
                let interconnect = cf::cfdict_get_val(props, "Physical Interconnect")
                    .map(cf::from_cfstr)
                    .unwrap_or_default();
                let location = cf::cfdict_get_val(props, "Physical Interconnect Location")
                    .map(cf::from_cfstr)
                    .unwrap_or_default();
                // capacity is nested inside "Controller Characteristics" -> "capacity".
                let bytes = cf::cfdict_get_val(props, "Controller Characteristics")
                    .and_then(|cc| cf::cfdict_get_val(cc, "capacity"))
                    .and_then(cf::cfnum_i64)
                    .unwrap_or(0)
                    .max(0) as u64;
                unsafe { cf::CFRelease(props) };
                if model.trim().is_empty() {
                    None
                } else {
                    let medium = match (interconnect.is_empty(), location.is_empty()) {
                        (true, _) => String::new(),
                        (false, true) => "NVMe".to_string(),
                        (false, false) => format!("NVMe · {location}"),
                    };
                    Some((model.trim().to_string(), bytes, medium))
                }
            });
            unsafe { IOObjectRelease(entry) };
            if let Some(r) = found {
                return r;
            }
        }
    }
    (String::new(), 0, String::new())
}

/// Short uptime like `30d 9h`, `4h 12m`, `7m`.
fn fmt_uptime(s: u64) -> String {
    let (d, h, m) = (s / 86400, (s % 86400) / 3600, (s % 3600) / 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marketing_table_knows_common_models() {
        assert_eq!(marketing_name("Mac16,11"), "Mac mini");
        assert_eq!(marketing_name("Mac14,3"), "Mac mini");
        assert_eq!(marketing_name("MacBookPro18,1"), "MacBook Pro");
        assert_eq!(marketing_name("Macmini9,1"), "Mac mini");
        assert_eq!(marketing_name("Totally,Unknown"), "");
    }

    #[test]
    fn get_is_infallible_and_populated() {
        // Runs on the host; identity reads must not panic and basics must be present.
        let s = SystemInfo::get();
        assert!(!s.model_id.is_empty());
        assert!(s.ram_bytes > 0);
        assert!(s.logical_cpu > 0);
    }
}
