//! AppleSMC over IOKit, hand-written (reimplemented from macmon, MIT). Eldr uses it
//! for fan telemetry: current RPM (`F0Ac`) and the min/max envelope (`F0Mn`/`F0Mx`).
//! Sensor keys on Apple Silicon are 4-byte floats (`flt `).

use crate::ffi::iokit::IOServiceIterator;
use core::ffi::c_void;
use std::collections::HashMap;

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
    pub fn temp_avgs(&mut self) -> (f32, f32) {
        const FLOAT_TYPE: u32 = 1_718_383_648; // FourCC "flt "
        let names = self.all_keys();
        let mut cpu = Vec::new();
        let mut gpu = Vec::new();
        for name in names {
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
            let Some(v) = self.read_f32(&name) else {
                continue;
            };
            if v > 0.0 && v <= 150.0 {
                if is_cpu {
                    cpu.push(v);
                } else {
                    gpu.push(v);
                }
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
    /// System total power in Watts (`PSTR`), or `None` if the key is absent.
    pub sys_power: Option<f32>,
    pub cpu_temp: f32,
    pub gpu_temp: f32,
    /// Whether SMC temp sensors were found (drives the IOHID fallback).
    pub has_temps: bool,
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
    let sys_power = smc.read_f32("PSTR").filter(|p| p.is_finite() && *p > 0.0);
    let (cpu_temp, gpu_temp) = smc.temp_avgs();
    SmcReadout {
        fan_rpm: rpm as u32,
        fan_min: min as u32,
        fan_max: max as u32,
        sys_power,
        cpu_temp,
        gpu_temp,
        has_temps: cpu_temp > 0.0 || gpu_temp > 0.0,
    }
}
