//! The unified [`Snapshot`] — Eldr's single data contract.
//!
//! Every sensor source (SoC via IOReport, host via mach/sysctl, fan via SMC, temps
//! via IOHID, thermal via Foundation) fills part of this struct. Consumers — `now`,
//! `check`, `status`, the TUI, the guard — read only this. The struct grows across
//! milestones; fields not yet wired default to zero and are not rendered.

use crate::sensors::{host, soc};

/// Overall health verdict. Mirrors the bash prototype's OK/WARN/ALERT.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Level {
    #[default]
    Ok,
    Warn,
    Alert,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Ok => "OK",
            Level::Warn => "WARN",
            Level::Alert => "ALERT",
        }
    }
    /// Process exit code for `eldr check` (0/1/2).
    pub fn exit_code(self) -> i32 {
        match self {
            Level::Ok => 0,
            Level::Warn => 1,
            Level::Alert => 2,
        }
    }
}

/// macOS thermal pressure (`NSProcessInfoThermalState`).
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Thermal {
    #[default]
    Unknown,
    Nominal,
    Fair,
    Serious,
    Critical,
}

impl Thermal {
    pub fn as_str(self) -> &'static str {
        match self {
            Thermal::Unknown => "unknown",
            Thermal::Nominal => "nominal",
            Thermal::Fair => "fair",
            Thermal::Serious => "serious",
            Thermal::Critical => "critical",
        }
    }
    pub fn from_raw(v: i64) -> Self {
        match v {
            0 => Thermal::Nominal,
            1 => Thermal::Fair,
            2 => Thermal::Serious,
            3 => Thermal::Critical,
            _ => Thermal::Unknown,
        }
    }
}

#[derive(Clone, Default, Debug)]
pub struct DiskInfo {
    pub total: u64,
    pub free: u64,
}

#[derive(Clone, Default, Debug)]
pub struct NetInfo {
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_rate: f64, // bytes/s over the sample interval
    pub tx_rate: f64,
}

#[derive(Clone, Default, Debug)]
pub struct ProcInfo {
    pub pid: i32,
    pub cpu: f32, // percent
    pub name: String,
}

/// One coherent reading of the whole machine.
#[derive(Clone, Default, Debug)]
pub struct Snapshot {
    pub ts: String,

    // SoC identity
    pub chip: String,
    pub mac_model: String,
    pub p_cores: u32,
    pub e_cores: u32,
    pub gpu_cores: u32,

    // CPU activity
    pub cpu_usage_pct: f32,  // 0..1 combined, core-weighted
    pub per_core: Vec<f32>,  // per logical core, 0..1
    pub ecpu_freq_mhz: u32,
    pub pcpu_freq_mhz: u32,
    pub ecpu_active: f32, // 0..1 of max freq
    pub pcpu_active: f32,
    pub load_avg: (f32, f32, f32),

    // GPU
    pub gpu_freq_mhz: u32,
    pub gpu_active: f32,

    // power (Watts)
    pub cpu_power: f32,
    pub gpu_power: f32,
    pub ane_power: f32,
    pub ram_power: f32,
    pub sys_power: f32,
    pub all_power: f32,

    // memory (bytes)
    pub ram_total: u64,
    pub ram_used: u64,
    pub swap_total: u64,
    pub swap_used: u64,

    // temps (Celsius)
    pub cpu_temp: f32,
    pub gpu_temp: f32,

    // fan
    pub fan_rpm: u32,
    pub fan_min: u32,
    pub fan_max: u32,

    // thermal pressure
    pub thermal: Thermal,

    // host extras
    pub uptime_secs: u64,
    pub disk: Option<DiskInfo>,
    pub net: Option<NetInfo>,
    pub top_procs: Vec<ProcInfo>,

    pub level: Level,
}

impl Snapshot {
    /// Gather a host-only snapshot (SoC identity + RAM/swap). Cheap, no sampling
    /// interval. Used as the base for [`Snapshot::gather`].
    pub fn gather_host() -> Self {
        let soc = soc::SocInfo::get();
        Snapshot::from_host(&soc)
    }

    fn from_host(soc: &soc::SocInfo) -> Self {
        let mut s = Snapshot::default();
        s.chip = soc.chip_name.clone();
        s.mac_model = soc.mac_model.clone();
        s.p_cores = soc.p_cores;
        s.e_cores = soc.e_cores;
        s.gpu_cores = soc.gpu_cores;
        s.ram_total = host::ram_total();
        s.ram_used = host::ram_used();
        let (su, st) = host::swap();
        s.swap_used = su;
        s.swap_total = st;
        s.ts = host::timestamp();
        s
    }

    /// Full snapshot: host identity/memory plus one IOReport sample over
    /// `duration_ms` for SoC power and per-cluster frequencies. Temps, fan,
    /// per-core load and thermal pressure are layered in by later milestones.
    pub fn gather(duration_ms: u64) -> Self {
        let soc = soc::SocInfo::get();
        let mut s = Snapshot::from_host(&soc);

        if let Some(sampler) = soc::SocSampler::new(soc) {
            let m = sampler.sample(duration_ms);
            s.ecpu_freq_mhz = m.ecpu_freq;
            s.pcpu_freq_mhz = m.pcpu_freq;
            s.ecpu_active = m.ecpu_active;
            s.pcpu_active = m.pcpu_active;
            s.cpu_usage_pct = m.cpu_usage_pct;
            s.gpu_freq_mhz = m.gpu_freq;
            s.gpu_active = m.gpu_active;
            s.cpu_power = m.cpu_power;
            s.gpu_power = m.gpu_power;
            s.ane_power = m.ane_power;
            s.ram_power = m.ram_power;
            s.all_power = m.all_power;
            // sys_power (SMC PSTR) is wired in M2; approximate with package power.
            s.sys_power = m.all_power;
        }
        s
    }
}
