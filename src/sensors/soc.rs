//! Apple Silicon identity and SoC sensors.
//!
//! - Identity (chip, model, P/E cores) from sysctl.
//! - Frequency tables (MHz) from the IORegistry `pmgr` voltage-states.
//! - [`SocSampler`] turns one IOReport delta into power (CPU/GPU/ANE/DRAM) and
//!   per-cluster active frequencies. Reimplemented from macmon (MIT).

use crate::ffi::cf::{
    CFDataRef, CFDictionaryRef, CFNumberRef, CFRelease, cfdata_bytes, cfdict_get_val, cfnum_i64,
};
use crate::ffi::iokit::{IOObjectRelease, IOServiceIterator, entry_properties};
use crate::ffi::ioreport::{IOReport, residencies, watts};
use crate::ffi::mach;

const CPU_FREQ_CORE_SUBG: &str = "CPU Core Performance States";
const GPU_FREQ_DICE_SUBG: &str = "GPU Performance States";

/// Static Apple Silicon description. Read once at startup.
#[derive(Clone, Default, Debug)]
pub struct SocInfo {
    pub chip_name: String,
    pub mac_model: String,
    /// Performance cores (sysctl `hw.perflevel0`, the top tier).
    pub p_cores: u32,
    /// Efficiency cores (sysctl `hw.perflevel1`).
    pub e_cores: u32,
    pub gpu_cores: u32,
    /// Per-cluster DVFS frequency tables (MHz).
    pub ecpu_freqs: Vec<u32>,
    pub pcpu_freqs: Vec<u32>,
    pub gpu_freqs: Vec<u32>,
}

impl SocInfo {
    /// Read identity (sysctl) and frequency tables (IORegistry). Infallible:
    /// missing values default sensibly so callers always get a usable struct.
    pub fn get() -> Self {
        let chip_name =
            mach::sysctl_string("machdep.cpu.brand_string").unwrap_or_else(|| "Unknown".into());
        let mac_model = mach::sysctl_string("hw.model").unwrap_or_else(|| "Unknown".into());
        let p_cores = mach::sysctl_u64("hw.perflevel0.physicalcpu").unwrap_or(0) as u32;
        let e_cores = mach::sysctl_u64("hw.perflevel1.physicalcpu").unwrap_or(0) as u32;

        let mut info = SocInfo {
            chip_name,
            mac_model,
            p_cores,
            e_cores,
            gpu_cores: 0,
            ecpu_freqs: Vec::new(),
            pcpu_freqs: Vec::new(),
            gpu_freqs: Vec::new(),
        };
        info.read_freqs();
        info.gpu_cores = read_gpu_cores().unwrap_or(0);
        info
    }

    pub fn total_cores(&self) -> u32 {
        self.p_cores + self.e_cores
    }

    /// Read DVFS frequency tables from the `pmgr` registry entry.
    fn read_freqs(&mut self) {
        let cpu_scale = cpu_freq_scale(&self.chip_name);
        let gpu_scale: u32 = 1_000_000; // -> MHz

        let Some(iter) = IOServiceIterator::new("AppleARMIODevice") else {
            return;
        };
        for (entry, name) in iter {
            if name == "pmgr" {
                if let Some(props) = entry_properties(entry) {
                    if let Some(f) = dvfs_freqs(props, "voltage-states1-sram", cpu_scale) {
                        self.ecpu_freqs = f;
                    }
                    if let Some(f) = dvfs_freqs(props, "voltage-states5-sram", cpu_scale) {
                        self.pcpu_freqs = f;
                    }
                    if let Some(f) = dvfs_freqs(props, "voltage-states9", gpu_scale) {
                        self.gpu_freqs = f;
                    }
                    unsafe { CFRelease(props) };
                }
                unsafe { IOObjectRelease(entry) };
                break;
            }
            unsafe { IOObjectRelease(entry) };
        }
    }
}

/// GPU core count from the `AGXAccelerator` registry entry's `gpu-core-count`.
fn read_gpu_cores() -> Option<u32> {
    let iter = IOServiceIterator::new("AGXAccelerator")?;
    for (entry, _name) in iter {
        let cores = entry_properties(entry).and_then(|props| {
            let n =
                cfdict_get_val(props, "gpu-core-count").and_then(|p| cfnum_i64(p as CFNumberRef));
            unsafe { CFRelease(props) };
            n
        });
        unsafe { IOObjectRelease(entry) };
        if let Some(n) = cores {
            return Some(n as u32);
        }
    }
    None
}

/// M1–M3 and A-series store DVFS frequencies in Hz; M4+ in kHz.
fn cpu_freq_scale(chip: &str) -> u32 {
    let hz =
        chip.contains("M1") || chip.contains("M2") || chip.contains("M3") || chip.contains("A1");
    if hz { 1_000_000 } else { 1_000 }
}

/// Read a `voltage-states*` CFData blob: pairs of `(freq u32, voltage u32)` little-
/// endian. Returns frequencies divided by `scale` (-> MHz), dropping the voltages.
fn dvfs_freqs(dict: CFDictionaryRef, key: &str, scale: u32) -> Option<Vec<u32>> {
    let data = cfdict_get_val(dict, key)? as CFDataRef;
    let bytes = cfdata_bytes(data);
    if bytes.len() < 8 {
        return None;
    }
    let scale = scale.max(1);
    let freqs: Vec<u32> = bytes
        .chunks_exact(8)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]) / scale)
        .collect();
    Some(freqs)
}

// MARK: sampler

/// One IOReport delta turned into SoC power + frequency metrics.
#[derive(Clone, Default, Debug)]
pub struct SocMetrics {
    pub ecpu_freq: u32,
    pub ecpu_active: f32, // 0..1 of max
    pub pcpu_freq: u32,
    pub pcpu_active: f32,
    pub cpu_usage_pct: f32, // 0..1 combined, core-weighted
    pub gpu_freq: u32,
    pub gpu_active: f32,
    pub cpu_power: f32,
    pub gpu_power: f32,
    pub ane_power: f32,
    pub ram_power: f32,
    pub all_power: f32,
}

pub struct SocSampler {
    soc: SocInfo,
    ior: IOReport,
}

impl SocSampler {
    pub fn new(soc: SocInfo) -> Option<Self> {
        let channels = [
            ("Energy Model", None),
            ("CPU Stats", Some(CPU_FREQ_CORE_SUBG)),
            ("GPU Stats", Some(GPU_FREQ_DICE_SUBG)),
        ];
        let ior = IOReport::new(&channels)?;
        Some(SocSampler { soc, ior })
    }

    /// Sample over `duration_ms` and reduce to [`SocMetrics`].
    pub fn sample(&self, duration_ms: u64) -> SocMetrics {
        let mut m = SocMetrics::default();
        let mut ecpu_usages: Vec<(u32, f32)> = Vec::new();
        let mut pcpu_usages: Vec<(u32, f32)> = Vec::new();

        for ch in self.ior.sample(duration_ms) {
            if ch.group == "CPU Stats" && ch.subgroup == CPU_FREQ_CORE_SUBG {
                if ch.channel.contains("PCPU") {
                    pcpu_usages.push(calc_freq(ch.item, &self.soc.pcpu_freqs));
                    continue;
                }
                if ch.channel.contains("ECPU") || ch.channel.contains("MCPU") {
                    ecpu_usages.push(calc_freq(ch.item, &self.soc.ecpu_freqs));
                    continue;
                }
            }

            if ch.group == "GPU Stats" && ch.subgroup == GPU_FREQ_DICE_SUBG && ch.channel == "GPUPH"
            {
                let gfreqs = if self.soc.gpu_freqs.len() > 1 {
                    &self.soc.gpu_freqs[1..]
                } else {
                    &self.soc.gpu_freqs[..]
                };
                let (f, a) = calc_freq(ch.item, gfreqs);
                m.gpu_freq = f;
                m.gpu_active = a;
            }

            if ch.group == "Energy Model" {
                let w = watts(ch.item, &ch.unit, duration_ms);
                let c = ch.channel.as_str();
                if c == "GPU Energy" {
                    m.gpu_power += w;
                } else if c.ends_with("CPU Energy") {
                    m.cpu_power += w;
                } else if c.starts_with("ANE") {
                    m.ane_power += w;
                } else if c.starts_with("DRAM") {
                    m.ram_power += w;
                }
            }
        }

        // Drop dead/disabled clusters (all-DOWN read as 0 active).
        ecpu_usages.retain(|&(_, p)| p > 0.0);

        let (ef, ea) = calc_freq_final(&ecpu_usages, &self.soc.ecpu_freqs);
        let (pf, pa) = calc_freq_final(&pcpu_usages, &self.soc.pcpu_freqs);
        m.ecpu_freq = ef;
        m.ecpu_active = ea;
        m.pcpu_freq = pf;
        m.pcpu_active = pa;

        let ec = self.soc.e_cores as f32;
        let pc = self.soc.p_cores as f32;
        let tc = (ec + pc).max(1.0);
        m.cpu_usage_pct = (ea * ec + pa * pc) / tc;
        m.all_power = m.cpu_power + m.gpu_power + m.ane_power;
        m
    }

    pub fn soc(&self) -> &SocInfo {
        &self.soc
    }
}

// MARK: frequency math (ported from macmon metrics.rs)

fn zero_div(a: f64, b: f64) -> f64 {
    if b == 0.0 { 0.0 } else { a / b }
}

/// Reduce one cluster's state residencies to `(avg_freq_mhz, active_fraction)`.
/// `freqs` is that cluster's DVFS table. Residency states lead with idle markers
/// (`IDLE`/`DOWN`/`OFF`) followed by one entry per frequency step.
fn calc_freq(item: CFDictionaryRef, freqs: &[u32]) -> (u32, f32) {
    if freqs.is_empty() {
        return (0, 0.0);
    }
    let items = residencies(item); // (state_name, residency)
    if items.len() <= freqs.len() {
        return (0, 0.0);
    }
    let Some(offset) = items
        .iter()
        .position(|x| x.0 != "IDLE" && x.0 != "DOWN" && x.0 != "OFF")
    else {
        return (0, 0.0);
    };
    if offset + freqs.len() > items.len() {
        return (0, 0.0);
    }

    let usage: f64 = items.iter().skip(offset).map(|x| x.1 as f64).sum();
    let total: f64 = items.iter().map(|x| x.1 as f64).sum();

    let mut avg_freq = 0f64;
    for i in 0..freqs.len() {
        let percent = zero_div(items[i + offset].1 as f64, usage);
        avg_freq += percent * freqs[i] as f64;
    }

    let usage_ratio = zero_div(usage, total);
    let min_freq = *freqs.first().unwrap() as f64;
    let max_freq = *freqs.last().unwrap() as f64;
    let from_max = (avg_freq.max(min_freq) * usage_ratio) / max_freq.max(1.0);

    (avg_freq as u32, from_max as f32)
}

/// Average per-core `(freq, active)` pairs into one cluster reading.
fn calc_freq_final(items: &[(u32, f32)], freqs: &[u32]) -> (u32, f32) {
    if items.is_empty() {
        return (0, 0.0);
    }
    let avg_freq = items.iter().map(|x| x.0 as f32).sum::<f32>() / items.len() as f32;
    let avg_perc = items.iter().map(|x| x.1).sum::<f32>() / items.len() as f32;
    let min_freq = *freqs.first().unwrap_or(&0) as f32;
    (avg_freq.max(min_freq) as u32, avg_perc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soc_identity_and_freq_tables() {
        let soc = SocInfo::get();
        eprintln!(
            "chip={:?} model={:?} P={} E={}",
            soc.chip_name, soc.mac_model, soc.p_cores, soc.e_cores
        );
        eprintln!("ecpu_freqs={:?}", soc.ecpu_freqs);
        eprintln!("pcpu_freqs={:?}", soc.pcpu_freqs);
        eprintln!("gpu_freqs={:?}", soc.gpu_freqs);

        assert!(!soc.chip_name.is_empty());
        assert!(soc.p_cores > 0, "expected at least one performance core");
        // Frequency tables must be present and ascending (DVFS steps).
        assert!(!soc.ecpu_freqs.is_empty(), "ecpu freq table empty");
        assert!(!soc.pcpu_freqs.is_empty(), "pcpu freq table empty");
        for table in [&soc.ecpu_freqs, &soc.pcpu_freqs] {
            assert!(
                table.windows(2).all(|w| w[0] <= w[1]),
                "freqs not ascending"
            );
            let max = *table.last().unwrap();
            assert!((800..=6000).contains(&max), "implausible max freq {max}");
        }
    }

    #[test]
    fn cpu_freq_scale_by_family() {
        assert_eq!(cpu_freq_scale("Apple M1 Pro"), 1_000_000);
        assert_eq!(cpu_freq_scale("Apple M3 Max"), 1_000_000);
        assert_eq!(cpu_freq_scale("Apple M4 Pro"), 1_000);
    }
}
