//! AppleSMC over IOKit, hand-written FFI (provenance in NOTICE). Eldr uses it
//! for fan telemetry: current RPM (`F0Ac`), the controller's commanded target (`F0Tg`)
//! and the min/max envelope (`F0Mn`/`F0Mx`).
//! Sensor keys on Apple Silicon are 4-byte floats (`flt `).

use crate::ffi::iokit::IOServiceIterator;
use core::ffi::c_void;
use std::collections::HashMap;
use std::sync::OnceLock;

unsafe extern "C" {
    fn mach_task_self() -> u32;
    fn IOServiceOpen(device: u32, owning_task: u32, kind: u32, connect: *mut u32) -> i32;
    fn IOServiceClose(conn: u32) -> i32;
    fn IOConnectCallStructMethod(
        conn: u32,
        selector: u32,
        input: *const c_void,
        input_size: usize,
        output: *mut c_void,
        output_size: *mut usize,
    ) -> i32;
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct KeyDataVer {
    major: u8,
    minor: u8,
    build: u8,
    reserved: u8,
    release: u16,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct PLimitData {
    version: u16,
    length: u16,
    cpu_p_limit: u32,
    gpu_p_limit: u32,
    mem_p_limit: u32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct KeyInfo {
    data_size: u32,
    data_type: u32,
    data_attributes: u8,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct KeyData {
    key: u32,
    vers: KeyDataVer,
    p_limit_data: PLimitData,
    key_info: KeyInfo,
    result: u8,
    status: u8,
    data8: u8,
    data32: u32,
    bytes: [u8; 32],
}

/// A live connection to `AppleSMCKeysEndpoint`.
pub struct Smc {
    conn: u32,
    key_info_cache: HashMap<u32, KeyInfo>,
}

impl Smc {
    pub fn new() -> Option<Self> {
        let mut conn = 0u32;
        for (device, name) in IOServiceIterator::new("AppleSMC")? {
            if name == "AppleSMCKeysEndpoint" {
                let rc = unsafe { IOServiceOpen(device, mach_task_self(), 0, &mut conn) };
                unsafe { crate::ffi::iokit::IOObjectRelease(device) };
                if rc == 0 {
                    return Some(Smc {
                        conn,
                        key_info_cache: HashMap::new(),
                    });
                }
                return None;
            }
            unsafe { crate::ffi::iokit::IOObjectRelease(device) };
        }
        None
    }

    fn call(&self, input: &KeyData) -> Option<KeyData> {
        let mut output = KeyData::default();
        let mut osize = size_of::<KeyData>();
        let rc = unsafe {
            IOConnectCallStructMethod(
                self.conn,
                2, // kSMCHandleYPCEvent
                input as *const KeyData as *const c_void,
                size_of::<KeyData>(),
                &mut output as *mut KeyData as *mut c_void,
                &mut osize,
            )
        };
        if rc != 0 || output.result != 0 {
            return None;
        }
        Some(output)
    }

    fn key_to_u32(key: &str) -> u32 {
        key.bytes().fold(0u32, |acc, b| (acc << 8) | b as u32)
    }

    fn read_key_info(&mut self, key: &str) -> Option<KeyInfo> {
        if key.len() != 4 {
            return None;
        }
        let k = Self::key_to_u32(key);
        if let Some(ki) = self.key_info_cache.get(&k) {
            return Some(*ki);
        }
        let input = KeyData {
            data8: 9,
            key: k,
            ..Default::default()
        };
        let out = self.call(&input)?;
        self.key_info_cache.insert(k, out.key_info);
        Some(out.key_info)
    }

    /// Read a key's raw bytes (length per its `KeyInfo`).
    fn read_bytes(&mut self, key: &str) -> Option<(KeyInfo, [u8; 32])> {
        let key_info = self.read_key_info(key)?;
        let input = KeyData {
            data8: 5,
            key: Self::key_to_u32(key),
            key_info,
            ..Default::default()
        };
        let out = self.call(&input)?;
        Some((key_info, out.bytes))
    }

    /// Read a 4-byte float SMC key (`flt `), little-endian.
    pub fn read_f32(&mut self, key: &str) -> Option<f32> {
        let (ki, bytes) = self.read_bytes(key)?;
        if ki.data_size < 4 {
            return None;
        }
        Some(f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// Resolve the SMC key name at a given index.
    fn key_by_index(&self, index: u32) -> Option<String> {
        let input = KeyData {
            data8: 8,
            data32: index,
            ..Default::default()
        };
        let out = self.call(&input)?;
        Some(
            std::str::from_utf8(&out.key.to_be_bytes())
                .ok()?
                .trim_end_matches('\0')
                .to_string(),
        )
    }

    /// Enumerate all SMC key names (via the `#KEY` count).
    fn all_keys(&mut self) -> Vec<String> {
        let Some((_, bytes)) = self.read_bytes("#KEY") else {
            return Vec::new();
        };
        let count = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let mut keys = Vec::with_capacity(count.min(2000) as usize);
        for i in 0..count.min(2000) {
            if let Some(k) = self.key_by_index(i) {
                keys.push(k);
            }
        }
        keys
    }

    /// CPU/GPU temperature averages from SMC float sensors. CPU = `Tp`/`Te`/`Ts`
    /// (performance/efficiency/super cores), GPU = `Tg`. Returns `(0,0)` if absent.
    ///
    /// The set of temperature keys never changes for a given Mac, so it is discovered
    /// (one full `#KEY` enumeration) only on the first call and cached. Steady-state
    /// samples then read just the handful of `T*` sensors instead of re-walking every
    /// SMC key — the bulk of the old per-sample cost.
    pub fn temp_avgs(&mut self) -> (f32, f32) {
        static KEYS: OnceLock<(Vec<String>, Vec<String>)> = OnceLock::new();
        if KEYS.get().is_none() {
            let _ = KEYS.set(self.discover_temp_keys());
        }
        let (cpu_keys, gpu_keys) = KEYS.get().unwrap();
        (self.avg_of(cpu_keys), self.avg_of(gpu_keys))
    }

    /// Mean of the valid (0–150 °C) readings of the given SMC float keys.
    fn avg_of(&mut self, keys: &[String]) -> f32 {
        let (mut sum, mut n) = (0.0f32, 0u32);
        for k in keys {
            if let Some(v) = self.read_f32(k)
                && v > 0.0
                && v <= 150.0
            {
                sum += v;
                n += 1;
            }
        }
        if n == 0 { 0.0 } else { sum / n as f32 }
    }

    /// Enumerate every SMC key once and keep the float temperature sensor names,
    /// split into CPU (`Tp`/`Te`/`Ts`) and GPU (`Tg`).
    fn discover_temp_keys(&mut self) -> (Vec<String>, Vec<String>) {
        const FLOAT_TYPE: u32 = 1_718_383_648; // FourCC "flt "
        let (mut cpu, mut gpu) = (Vec::new(), Vec::new());
        for name in self.all_keys() {
            let is_cpu = name.starts_with("Tp") || name.starts_with("Te") || name.starts_with("Ts");
            let is_gpu = name.starts_with("Tg");
            if !is_cpu && !is_gpu {
                continue;
            }
            let Some(ki) = self.read_key_info(&name) else {
                continue;
            };
            if ki.data_size != 4 || ki.data_type != FLOAT_TYPE {
                continue;
            }
            if is_cpu {
                cpu.push(name);
            } else {
                gpu.push(name);
            }
        }
        (cpu, gpu)
    }

    /// Read EVERY float SMC sensor, classified by group. Enumerates all keys (one
    /// `#KEY` walk), gates strictly on `flt `/4-byte to dodge the big-endian integer /
    /// fixed-point decode footgun, classifies by the key's first letter, and keeps only
    /// values inside the group's sane range. Backs `eldr sensors`.
    pub fn read_all_sensors(&mut self) -> Vec<Sensor> {
        const FLOAT_TYPE: u32 = 1_718_383_648;
        let mut out = Vec::new();
        for name in self.all_keys() {
            let Some(group) = classify(&name) else {
                continue;
            };
            let Some(ki) = self.read_key_info(&name) else {
                continue;
            };
            if ki.data_size != 4 || ki.data_type != FLOAT_TYPE {
                continue;
            }
            let Some(v) = self.read_f32(&name) else {
                continue;
            };
            if v.is_finite() && group.in_range(v) {
                out.push(Sensor {
                    key: name,
                    group,
                    value: v,
                });
            }
        }
        out
    }
}

/// One decoded SMC sensor reading.
#[derive(Clone, Debug)]
pub struct Sensor {
    pub key: String,
    pub group: SensorGroup,
    pub value: f32,
}

/// What an SMC sensor measures, inferred from its key-name family.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SensorGroup {
    Temp,
    Fan,
    Power,
    Current,
    Voltage,
}

impl SensorGroup {
    pub fn title(self) -> &'static str {
        match self {
            SensorGroup::Temp => "Temperatures",
            SensorGroup::Fan => "Fans",
            SensorGroup::Power => "Power",
            SensorGroup::Current => "Currents",
            SensorGroup::Voltage => "Voltages",
        }
    }
    pub fn unit(self) -> &'static str {
        match self {
            SensorGroup::Temp => "°C",
            SensorGroup::Fan => "rpm",
            SensorGroup::Power => "W",
            SensorGroup::Current => "A",
            SensorGroup::Voltage => "V",
        }
    }
    /// Membership test: combined with the `flt `/4-byte gate, the range is what keeps a
    /// float-looking non-sensor key out of a group.
    fn in_range(self, v: f32) -> bool {
        match self {
            SensorGroup::Temp => v > 0.0 && v <= 150.0,
            SensorGroup::Fan => (0.0..=12000.0).contains(&v),
            SensorGroup::Power => v > 0.0 && v <= 400.0,
            SensorGroup::Current => (0.0..=50.0).contains(&v),
            SensorGroup::Voltage => (0.0..=30.0).contains(&v),
        }
    }
}

/// Classify an SMC key by its first letter (fans also need a digit: `F0Ac`, not `FNum`).
fn classify(name: &str) -> Option<SensorGroup> {
    let b = name.as_bytes();
    match b.first()? {
        b'T' => Some(SensorGroup::Temp),
        b'F' if b.get(1).is_some_and(|c| c.is_ascii_digit()) => Some(SensorGroup::Fan),
        b'P' => Some(SensorGroup::Power),
        b'I' => Some(SensorGroup::Current),
        b'V' => Some(SensorGroup::Voltage),
        _ => None,
    }
}

/// Every float SMC sensor on this machine, classified. Empty if SMC is unavailable.
pub fn all_sensors() -> Vec<Sensor> {
    match Smc::new() {
        Some(mut smc) => smc.read_all_sensors(),
        None => Vec::new(),
    }
}

impl Drop for Smc {
    fn drop(&mut self) {
        unsafe {
            IOServiceClose(self.conn);
        }
    }
}

/// Fan envelope, system power and temperatures, read over one SMC connection.
#[derive(Default, Clone, Copy)]
pub struct SmcReadout {
    pub fan_rpm: u32,
    pub fan_min: u32,
    pub fan_max: u32,
    /// Commanded target RPM (`F0Tg`). On Apple Silicon the controller drives this to 0
    /// when the machine is cool — the fan then legitimately stops. A non-zero target
    /// with a stalled `fan_rpm` is the real "fan failed" signal.
    pub fan_target: u32,
    /// System total power in Watts (`PSTR`), or `None` if the key is absent.
    pub sys_power: Option<f32>,
    pub cpu_temp: f32,
    pub gpu_temp: f32,
    /// Whether SMC temp sensors were found (drives the IOHID fallback).
    pub has_temps: bool,
}

/// One physical fan's telemetry: current RPM, the controller's commanded target, and
/// the min/max envelope.
#[derive(Default, Clone, Copy, Debug)]
pub struct FanReading {
    pub rpm: u32,
    pub min: u32,
    pub max: u32,
    pub target: u32,
}

/// Every fan the SMC reports, discovered by probing `F0*`, `F1*`, … until a fan's
/// max-RPM key is absent. (The `FNum` count key isn't a 4-byte float, so it can't be
/// read this way — probing the envelope keys is what's reliable.) A MacBook Pro has two
/// fans; an Air has none. Empty if SMC is unavailable.
pub fn read_fans() -> Vec<FanReading> {
    let Some(mut smc) = Smc::new() else {
        return Vec::new();
    };
    let mut fans = Vec::new();
    for i in 0..8u32 {
        // The envelope max is the presence test: no `F{i}Mx` means no fan i.
        let Some(max) = smc.read_f32(&format!("F{i}Mx")) else {
            break;
        };
        if max <= 0.0 {
            break;
        }
        fans.push(FanReading {
            rpm: smc.read_f32(&format!("F{i}Ac")).unwrap_or(0.0) as u32,
            min: smc.read_f32(&format!("F{i}Mn")).unwrap_or(0.0) as u32,
            max: max as u32,
            target: smc.read_f32(&format!("F{i}Tg")).unwrap_or(0.0) as u32,
        });
    }
    fans
}

/// Read fan telemetry, system power and temps in one shot. All-zero / `None` if SMC
/// is unavailable.
pub fn read() -> SmcReadout {
    let Some(mut smc) = Smc::new() else {
        return SmcReadout::default();
    };
    let rpm = smc.read_f32("F0Ac").unwrap_or(0.0);
    let min = smc.read_f32("F0Mn").unwrap_or(0.0);
    let max = smc.read_f32("F0Mx").unwrap_or(0.0);
    let target = smc.read_f32("F0Tg").unwrap_or(0.0);
    let sys_power = smc.read_f32("PSTR").filter(|p| p.is_finite() && *p > 0.0);
    let (cpu_temp, gpu_temp) = smc.temp_avgs();
    SmcReadout {
        fan_rpm: rpm as u32,
        fan_min: min as u32,
        fan_max: max as u32,
        fan_target: target as u32,
        sys_power,
        cpu_temp,
        gpu_temp,
        has_temps: cpu_temp > 0.0 || gpu_temp > 0.0,
    }
}
