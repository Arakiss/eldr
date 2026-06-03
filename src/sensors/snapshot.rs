//! The unified [`Snapshot`] — Eldr's single data contract.
//!
//! Every sensor source (SoC via IOReport, host via mach/sysctl/libproc, fan via SMC,
//! temps via IOHID, thermal via Foundation) fills part of this struct. Consumers —
//! `now`, `check`, `status`, the TUI, the guard — read only this. The agent contract
//! is `status.json`, produced by [`Snapshot::to_json`].

use crate::config;
use crate::ffi::{iohid, smc, thermal};
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
    pub rx_rate: f64, // bytes/s over the sample window
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
    pub source: String, // "oneshot" | "tui" | "guard"

    // SoC identity
    pub chip: String,
    pub mac_model: String,
    pub p_cores: u32,
    pub e_cores: u32,
    pub gpu_cores: u32,

    // CPU activity
    pub cpu_usage_pct: f32, // 0..1 combined, core-weighted (from IOReport residency)
    pub cpu_load_pct: f32,  // 0..1 mean of per-core ticks (host_processor_info)
    pub per_core: Vec<f32>, // per logical core, 0..1
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
    fn from_host(soc: &soc::SocInfo) -> Self {
        let mut s = Snapshot::default();
        s.source = "oneshot".into();
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

    /// Full snapshot. One shared sample window of `duration_ms` covers the IOReport
    /// delta (SoC power/freq) and the host interval deltas (per-core, net, top
    /// processes). Point-in-time sensors (temps, fan, thermal) are read after.
    pub fn gather(duration_ms: u64) -> Self {
        let soc = soc::SocInfo::get();
        let mut s = Snapshot::from_host(&soc);

        // t0 for interval host metrics
        let ht0 = host::start();

        // SoC sample sleeps `duration_ms` internally; if IOReport is unavailable we
        // sleep ourselves so the host deltas still have a window.
        let sampler = soc::SocSampler::new(soc);
        match &sampler {
            Some(sp) => {
                let m = sp.sample(duration_ms);
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
            }
            None => std::thread::sleep(std::time::Duration::from_millis(duration_ms)),
        }

        let hm = host::finish(ht0, 5);
        s.per_core = hm.per_core;
        s.cpu_load_pct = hm.cpu_total;
        s.load_avg = hm.load;
        s.uptime_secs = hm.uptime_secs;
        s.disk = Some(hm.disk);
        s.net = Some(hm.net);
        s.top_procs = hm.top;

        // point-in-time sensors
        let smc = smc::read();
        s.fan_rpm = smc.fan_rpm;
        s.fan_min = smc.fan_min;
        s.fan_max = smc.fan_max;

        // Temps: SMC (Tp/Te/Tg) on macOS 14+, IOHID fallback for older Macs.
        if smc.has_temps {
            s.cpu_temp = smc.cpu_temp;
            s.gpu_temp = smc.gpu_temp;
        } else {
            let (cpu_t, gpu_t) = iohid::temps();
            s.cpu_temp = cpu_t;
            s.gpu_temp = gpu_t;
        }

        s.thermal = match thermal::thermal_state_raw() {
            Some(v) => Thermal::from_raw(v),
            None => Thermal::Unknown,
        };

        // sys_power via SMC PSTR, falling back to package power.
        s.sys_power = smc
            .sys_power
            .map(|p| p.max(s.all_power))
            .unwrap_or(s.all_power);

        s.level = s.compute_level();
        s
    }

    /// Health verdict. Gate ONLY on macOS thermal pressure (the clean throttle
    /// signal) plus a stopped fan — the same discipline as the bash prototype. Die
    /// temperature reads high on healthy hardware, so it informs but never triggers.
    pub fn compute_level(&self) -> Level {
        let mut lvl = match self.thermal {
            Thermal::Serious | Thermal::Critical => Level::Alert,
            Thermal::Fair => Level::Warn,
            _ => Level::Ok,
        };
        // A stopped/failed fan is danger regardless of thermal — but only trust it
        // when the SMC actually reported a fan envelope (avoids false alarms when SMC
        // is unavailable and everything reads zero).
        if self.fan_max > 0 && self.fan_rpm < 500 {
            lvl = Level::Alert;
        }
        lvl
    }

    /// Write status.json atomically (temp file + rename) into the data dir.
    pub fn write_status(&self) -> std::io::Result<()> {
        let dir = config::ensure_data_dir();
        let final_path = config::status_path();
        let tmp = dir.join("status.json.tmp");
        std::fs::write(&tmp, self.to_json())?;
        std::fs::rename(&tmp, &final_path)
    }

    /// Serialize to status.json (hand-rolled; no `serde`). One flat object.
    pub fn to_json(&self) -> String {
        let mut o = JsonObj::new();
        o.s("ts", &self.ts);
        o.s("source", &self.source);
        o.s("level", self.level.as_str());
        o.s("chip", &self.chip);
        o.s("mac_model", &self.mac_model);
        o.u("p_cores", self.p_cores as u64);
        o.u("e_cores", self.e_cores as u64);
        o.u("gpu_cores", self.gpu_cores as u64);

        o.f("cpu_usage_pct", self.cpu_usage_pct);
        o.f("cpu_load_pct", self.cpu_load_pct);
        o.arr_f("per_core", &self.per_core);
        o.u("pcpu_freq_mhz", self.pcpu_freq_mhz as u64);
        o.u("ecpu_freq_mhz", self.ecpu_freq_mhz as u64);
        o.f("pcpu_active", self.pcpu_active);
        o.f("ecpu_active", self.ecpu_active);
        o.arr_f(
            "load_avg",
            &[self.load_avg.0, self.load_avg.1, self.load_avg.2],
        );

        o.u("gpu_freq_mhz", self.gpu_freq_mhz as u64);
        o.f("gpu_active", self.gpu_active);

        o.f("cpu_power", self.cpu_power);
        o.f("gpu_power", self.gpu_power);
        o.f("ane_power", self.ane_power);
        o.f("ram_power", self.ram_power);
        o.f("sys_power", self.sys_power);
        o.f("all_power", self.all_power);

        o.u("ram_total", self.ram_total);
        o.u("ram_used", self.ram_used);
        o.u("swap_total", self.swap_total);
        o.u("swap_used", self.swap_used);

        o.f("cpu_temp", self.cpu_temp);
        o.f("gpu_temp", self.gpu_temp);

        o.u("fan_rpm", self.fan_rpm as u64);
        o.u("fan_min", self.fan_min as u64);
        o.u("fan_max", self.fan_max as u64);
        o.s("thermal", self.thermal.as_str());

        o.u("uptime_secs", self.uptime_secs);
        if let Some(d) = &self.disk {
            o.u("disk_total", d.total);
            o.u("disk_free", d.free);
        }
        if let Some(n) = &self.net {
            o.u("net_rx_bytes", n.rx_bytes);
            o.u("net_tx_bytes", n.tx_bytes);
            o.f64("net_rx_rate", n.rx_rate);
            o.f64("net_tx_rate", n.tx_rate);
        }

        // top processes as an array of objects
        let procs = self
            .top_procs
            .iter()
            .map(|p| {
                format!(
                    "{{\"pid\":{},\"cpu\":{},\"name\":\"{}\"}}",
                    p.pid,
                    fmt_f(p.cpu),
                    json_escape(&p.name)
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        o.raw("top_procs", &format!("[{procs}]"));

        o.finish()
    }
}

// MARK: tiny JSON object builder

struct JsonObj {
    buf: String,
    first: bool,
}

impl JsonObj {
    fn new() -> Self {
        JsonObj {
            buf: String::from("{"),
            first: true,
        }
    }
    fn sep(&mut self) {
        if self.first {
            self.first = false;
        } else {
            self.buf.push(',');
        }
    }
    fn key(&mut self, k: &str) {
        self.sep();
        self.buf.push('"');
        self.buf.push_str(k);
        self.buf.push_str("\":");
    }
    fn s(&mut self, k: &str, v: &str) {
        self.key(k);
        self.buf.push('"');
        self.buf.push_str(&json_escape(v));
        self.buf.push('"');
    }
    fn u(&mut self, k: &str, v: u64) {
        self.key(k);
        self.buf.push_str(&v.to_string());
    }
    fn f(&mut self, k: &str, v: f32) {
        self.key(k);
        self.buf.push_str(&fmt_f(v));
    }
    fn f64(&mut self, k: &str, v: f64) {
        self.key(k);
        self.buf.push_str(&fmt_f(v as f32));
    }
    fn arr_f(&mut self, k: &str, v: &[f32]) {
        self.key(k);
        self.buf.push('[');
        for (i, x) in v.iter().enumerate() {
            if i > 0 {
                self.buf.push(',');
            }
            self.buf.push_str(&fmt_f(*x));
        }
        self.buf.push(']');
    }
    fn raw(&mut self, k: &str, v: &str) {
        self.key(k);
        self.buf.push_str(v);
    }
    fn finish(mut self) -> String {
        self.buf.push('}');
        self.buf
    }
}

/// Format a float with up to 3 decimals, no trailing noise, finite-guarded.
fn fmt_f(v: f32) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    let s = format!("{:.3}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" {
        "0".into()
    } else {
        s.to_string()
    }
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap() -> Snapshot {
        let mut s = Snapshot::default();
        s.fan_max = 4900;
        s.fan_rpm = 1800;
        s
    }

    #[test]
    fn level_gates_on_thermal_and_fan() {
        let mut s = snap();
        s.thermal = Thermal::Nominal;
        assert_eq!(s.compute_level(), Level::Ok);
        s.thermal = Thermal::Fair;
        assert_eq!(s.compute_level(), Level::Warn);
        s.thermal = Thermal::Serious;
        assert_eq!(s.compute_level(), Level::Alert);
        s.thermal = Thermal::Critical;
        assert_eq!(s.compute_level(), Level::Alert);
        // High die temp must NOT trip a level on its own.
        s.thermal = Thermal::Nominal;
        s.cpu_temp = 99.0;
        assert_eq!(s.compute_level(), Level::Ok);
        // Stopped fan is danger regardless of thermal.
        s.fan_rpm = 0;
        assert_eq!(s.compute_level(), Level::Alert);
        // ...but a zero fan with no SMC envelope must not false-alarm.
        s.fan_max = 0;
        assert_eq!(s.compute_level(), Level::Ok);
    }

    #[test]
    fn exit_codes() {
        assert_eq!(Level::Ok.exit_code(), 0);
        assert_eq!(Level::Warn.exit_code(), 1);
        assert_eq!(Level::Alert.exit_code(), 2);
    }

    #[test]
    fn json_is_wellformed_and_escapes() {
        let mut s = snap();
        s.chip = "Apple \"M4\" Pro".into();
        s.top_procs.push(ProcInfo {
            pid: 1,
            cpu: 3.5,
            name: "a\\b".into(),
        });
        let j = s.to_json();
        assert!(j.starts_with('{') && j.ends_with('}'));
        assert_eq!(j.matches('{').count(), j.matches('}').count());
        assert_eq!(j.matches('[').count(), j.matches(']').count());
        assert!(j.contains("\"level\":\"OK\""));
        assert!(j.contains("\\\"M4\\\"")); // escaped quotes
        assert!(j.contains("a\\\\b")); // escaped backslash
    }

    #[test]
    fn float_formatting_is_clean() {
        assert_eq!(fmt_f(0.0), "0");
        assert_eq!(fmt_f(1.5), "1.5");
        assert_eq!(fmt_f(10.000), "10");
        assert_eq!(fmt_f(f32::NAN), "0");
        assert_eq!(fmt_f(f32::INFINITY), "0");
    }
}
