//! Apple Silicon identity and SoC sensors.
//!
//! M0 fills the identity (chip name, model, P/E core counts) from sysctl. M1 adds
//! the frequency tables (IORegistry `pmgr` voltage-states) and the IOReport sampler
//! for power and per-cluster frequency residencies.

use crate::ffi::mach;

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
    /// Per-cluster DVFS frequency tables (MHz), filled in M1.
    pub ecpu_freqs: Vec<u32>,
    pub pcpu_freqs: Vec<u32>,
    pub gpu_freqs: Vec<u32>,
}

impl SocInfo {
    /// Read identity from sysctl. Infallible: missing values default sensibly.
    pub fn get() -> Self {
        let chip_name =
            mach::sysctl_string("machdep.cpu.brand_string").unwrap_or_else(|| "Unknown".into());
        let mac_model = mach::sysctl_string("hw.model").unwrap_or_else(|| "Unknown".into());
        // perflevel0 is always the highest-performance tier; perflevel1 the next.
        let p_cores = mach::sysctl_u64("hw.perflevel0.physicalcpu").unwrap_or(0) as u32;
        let e_cores = mach::sysctl_u64("hw.perflevel1.physicalcpu").unwrap_or(0) as u32;

        SocInfo {
            chip_name,
            mac_model,
            p_cores,
            e_cores,
            gpu_cores: 0, // filled from IORegistry in M1
            ecpu_freqs: Vec::new(),
            pcpu_freqs: Vec::new(),
            gpu_freqs: Vec::new(),
        }
    }

    /// Total logical CPU cores reported by sysctl.
    pub fn total_cores(&self) -> u32 {
        self.p_cores + self.e_cores
    }
}
