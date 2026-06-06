//! The unified [`Snapshot`] — Eldr's single data contract.
//!
//! Every sensor source (SoC via IOReport, host via mach/sysctl/libproc, fan via SMC,
//! temps via IOHID, thermal via Foundation) fills part of this struct. Consumers —
//! `now`, `check`, `status`, the TUI, the guard — read only this. The agent contract
//! is `status.json`, produced by [`Snapshot::to_json`].

use crate::config;
use crate::ffi::{battery, iohid, smc, thermal};
use crate::sensors::{host, soc};

/// status.json / `--json` schema version. Bump on a breaking shape change so an agent
/// can tell what it's parsing.
pub const SCHEMA_VERSION: &str = "1";

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

/// One mounted user-facing volume (boot disk or an external/data volume). The boot
/// volume is also mirrored into [`DiskInfo`] so the original `disk_*` JSON keys stay.
#[derive(Clone, Default, Debug)]
pub struct VolumeInfo {
    pub name: String,        // canonical volume name, e.g. "Macintosh HD" or "Vault"
    pub mount_point: String, // e.g. "/" or "/Volumes/Vault"
    pub device: String,      // e.g. "/dev/disk4s2"
    pub fs: String,          // e.g. "apfs"
    pub total: u64,
    pub free: u64,
    pub external: bool, // mounted under /Volumes (refined to a true bus check in M-disk)
}

/// Health of one physical disk: identity, the firmware SMART verdict, and the I/O error
/// and latency counters that surface degradation before SMART flips. Counters are
/// cumulative since boot; the guard watches them for growth.
#[derive(Clone, Default, Debug)]
pub struct DiskHealth {
    pub bsd_name: String, // "disk4"
    pub model: String,    // "Samsung SSD 990 PRO 4TB"
    pub external: bool,
    pub interconnect: String, // bus: "PCI-Express" | "USB" | "SATA" | "Apple Fabric"
    pub solid_state: bool,
    pub smart: String, // "verified" | "failing" | "not supported" | "" (unread)
    pub read_errors: u64,
    pub write_errors: u64,
    pub read_retries: u64,
    pub write_retries: u64,
    pub read_latency_ms: f32,
    pub write_latency_ms: f32,
    /// Firmware NVMe SMART telemetry (temp, wear, TBW), when the disk exposes it.
    pub nvme: Option<crate::ffi::nvme::NvmeSmart>,
}

impl DiskHealth {
    /// Build from raw IOKit counters, deriving mean latency (service time / operations).
    pub fn from_stat(d: crate::ffi::iostat::DiskStat) -> Self {
        let latency = |time_ns: u64, ops: u64| {
            if ops > 0 {
                (time_ns as f64 / ops as f64 / 1.0e6) as f32
            } else {
                0.0
            }
        };
        DiskHealth {
            read_latency_ms: latency(d.read_time_ns, d.read_ops),
            write_latency_ms: latency(d.write_time_ns, d.write_ops),
            bsd_name: d.bsd_name,
            model: d.model,
            external: d.external,
            interconnect: d.interconnect,
            solid_state: d.solid_state,
            smart: String::new(),
            read_errors: d.read_errors,
            write_errors: d.write_errors,
            read_retries: d.read_retries,
            write_retries: d.write_retries,
            nvme: d.nvme,
        }
    }

    /// True when the firmware's critical-warning bitfield is set (spare low, reliability
    /// degraded, read-only, volatile-memory failure, or over temperature).
    pub fn nvme_critical(&self) -> bool {
        self.nvme.map(|n| n.critical_warning != 0).unwrap_or(false)
    }

    /// An unambiguous NVMe-firmware degradation signal worth alerting on, or `None`.
    /// Deliberately conservative — uses the disk's own thresholds, not arbitrary ones —
    /// so it never cries wolf (transient high temperature, for instance, is not here).
    pub fn nvme_alarm(&self) -> Option<&'static str> {
        let n = self.nvme?;
        if n.critical_warning != 0 {
            Some("NVMe critical warning")
        } else if n.spare_threshold > 0 && n.available_spare < n.spare_threshold {
            Some("NVMe spare below firmware threshold")
        } else if n.percentage_used >= 100 {
            Some("NVMe endurance exhausted")
        } else {
            None
        }
    }
    pub fn errors(&self) -> u64 {
        self.read_errors + self.write_errors
    }
    pub fn retries(&self) -> u64 {
        self.read_retries + self.write_retries
    }
    /// True only when the firmware itself predicts imminent failure.
    pub fn smart_failing(&self) -> bool {
        self.smart.eq_ignore_ascii_case("failing")
    }
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
    pub mem: u64, // memory footprint in bytes (ri_phys_footprint)
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

    // memory (bytes) — `used` = app+wired+compressed (truly occupied), `available` =
    // free + reclaimable cache. Apple's framing, not a misleading raw "% used".
    pub ram_total: u64,
    pub ram_used: u64,
    pub ram_available: u64,
    pub ram_cached: u64,
    pub ram_wired: u64,
    pub ram_compressed: u64,
    /// Uncompressed size of the data held in the compressor (≥ `ram_compressed`); the
    /// ratio of the two is how hard macOS is packing memory.
    pub ram_compressed_holds: u64,
    pub swap_total: u64,
    pub swap_used: u64,

    // temps (Celsius)
    pub cpu_temp: f32,
    pub gpu_temp: f32,

    // fan
    pub fan_rpm: u32,
    pub fan_min: u32,
    pub fan_max: u32,
    /// RPM the macOS thermal controller is commanding (`F0Tg`). Zero when the system
    /// wants no airflow — the normal idle state on Apple Silicon, where fans fully stop.
    pub fan_target: u32,
    /// Every fan the SMC reports, for the Cooling view. The fields above stay the
    /// watchdog's single signal (the primary fan); this is the full set for display.
    pub fans: Vec<smc::FanReading>,
    /// Internal battery, or `None` on a desktop Mac.
    pub battery: Option<battery::Battery>,

    // thermal pressure
    pub thermal: Thermal,

    // host extras
    pub uptime_secs: u64,
    pub disk: Option<DiskInfo>,
    /// Every mounted user-facing volume (boot + external/data). Empty if enumeration
    /// failed, in which case `disk` still carries the boot volume.
    pub volumes: Vec<VolumeInfo>,
    /// Health of each physical disk (I/O errors, retries, latency; SMART filled lazily
    /// by [`Snapshot::read_smart`]).
    pub disk_health: Vec<DiskHealth>,
    pub net: Option<NetInfo>,
    pub top_procs: Vec<ProcInfo>,
    pub top_mem: Vec<ProcInfo>,

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
        let mem = host::mem_info();
        s.ram_total = mem.total;
        s.ram_used = mem.used;
        s.ram_available = mem.available;
        s.ram_cached = mem.cached;
        s.ram_wired = mem.wired;
        s.ram_compressed = mem.compressed;
        s.ram_compressed_holds = mem.compressed_holds;
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
        s.volumes = hm.volumes;
        s.disk_health = crate::ffi::iostat::disks()
            .into_iter()
            .map(DiskHealth::from_stat)
            .collect();
        s.net = Some(hm.net);
        s.top_procs = hm.top;
        s.top_mem = hm.top_mem;

        // point-in-time sensors
        let smc = smc::read();
        s.fan_rpm = smc.fan_rpm;
        s.fan_min = smc.fan_min;
        s.fan_max = smc.fan_max;
        s.fan_target = smc.fan_target;
        s.fans = crate::ffi::smc::read_fans();
        s.battery = battery::read();

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
    /// signal) plus a genuinely failed fan — the same discipline as the bash prototype.
    /// Die temperature reads high on healthy hardware, so it informs but never triggers.
    pub fn compute_level(&self) -> Level {
        let mut lvl = match self.thermal {
            Thermal::Serious | Thermal::Critical => Level::Alert,
            Thermal::Fair => Level::Warn,
            _ => Level::Ok,
        };
        // A failed fan is danger regardless of thermal pressure.
        if self.fan_failed() {
            lvl = Level::Alert;
        }
        lvl
    }

    /// True when the cooling controller is COMMANDING airflow (`fan_target` up) but the
    /// fan isn't spinning — a genuinely failed or blocked fan.
    ///
    /// Crucially this is NOT "the fan reads zero". Apple Silicon stops its fans entirely
    /// when the machine is cool (passive cooling), so 0 RPM at idle is normal, not a
    /// fault — flagging it produced false `ALERT`s on a perfectly healthy, cold Mac.
    /// Gating on a non-zero commanded target (and requiring a real SMC envelope, so a
    /// missing SMC reading zero everywhere can't false-alarm) keeps the genuine
    /// failure — a dead fan while the system is calling for cooling — while staying
    /// quiet at idle.
    pub fn fan_failed(&self) -> bool {
        // True when airflow is commanded (target up) but a fan isn't spinning. Checks the
        // primary fan (the canonical signal, and the only one the tests set) plus every
        // other fan the SMC reports — so a dead secondary fan is caught too, not just F0.
        let stalled = |max: u32, target: u32, rpm: u32| max > 0 && target >= 500 && rpm < 500;
        stalled(self.fan_max, self.fan_target, self.fan_rpm)
            || self.fans.iter().any(|f| stalled(f.max, f.target, f.rpm))
    }

    /// Plain-language memory pressure from how much is reclaimable (free + cache),
    /// not from a misleading raw "% used".
    pub fn mem_pressure(&self) -> &'static str {
        if self.ram_total == 0 {
            return "unknown";
        }
        let avail = self.ram_available as f64 / self.ram_total as f64;
        if avail >= 0.30 {
            "low"
        } else if avail >= 0.10 {
            "medium"
        } else {
            "high"
        }
    }

    /// Fill the firmware SMART verdict for each physical disk. Shells out to `diskutil`,
    /// so call it from one-shot views (`now`, `disk`) and the guard loop — never from the
    /// TUI refresh, which must stay free of process spawns.
    pub fn read_smart(&mut self) {
        for h in &mut self.disk_health {
            if !h.bsd_name.is_empty() {
                h.smart = crate::ffi::iostat::smart_status(&h.bsd_name);
            }
        }
    }

    /// True if any physical disk's firmware reports SMART failing — a back-up-now signal.
    pub fn any_disk_failing(&self) -> bool {
        self.disk_health.iter().any(|h| h.smart_failing())
    }

    /// Storage exit code for agents: 2 if any disk is SMART-failing or NVMe-critical,
    /// 1 if any disk shows I/O errors, else 0.
    pub fn disk_exit_code(&self) -> i32 {
        if self
            .disk_health
            .iter()
            .any(|h| h.smart_failing() || h.nvme_critical())
        {
            2
        } else if self.disk_health.iter().any(|h| h.errors() > 0) {
            1
        } else {
            0
        }
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
        o.s("schema_version", SCHEMA_VERSION);
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
        o.u("ram_available", self.ram_available);
        o.u("ram_cached", self.ram_cached);
        o.u("ram_wired", self.ram_wired);
        o.u("ram_compressed", self.ram_compressed);
        o.u("ram_compressed_holds", self.ram_compressed_holds);
        o.s("mem_pressure", self.mem_pressure());
        o.u("swap_total", self.swap_total);
        o.u("swap_used", self.swap_used);

        o.f("cpu_temp", self.cpu_temp);
        o.f("gpu_temp", self.gpu_temp);

        o.u("fan_rpm", self.fan_rpm as u64);
        o.u("fan_min", self.fan_min as u64);
        o.u("fan_max", self.fan_max as u64);
        o.u("fan_target", self.fan_target as u64);
        if let Some(b) = &self.battery {
            o.u("battery_percent", b.percent as u64);
            o.s(
                "battery_state",
                if b.charging {
                    "charging"
                } else if b.on_ac {
                    "ac"
                } else {
                    "discharging"
                },
            );
            o.f("battery_power_w", b.power_w);
            if let Some(t) = b.time_min {
                o.u("battery_time_min", t as u64);
            }
            if let Some(c) = b.cycles {
                o.u("battery_cycles", c as u64);
            }
            if let Some(h) = b.health_pct {
                o.u("battery_health_pct", h as u64);
            }
        }
        o.s("thermal", self.thermal.as_str());

        o.u("uptime_secs", self.uptime_secs);
        if let Some(d) = &self.disk {
            o.u("disk_total", d.total);
            o.u("disk_free", d.free);
        }
        // All mounted volumes (boot + external). Additive to disk_total/disk_free, which
        // stay for the existing agent contract.
        let vols = self
            .volumes
            .iter()
            .map(|v| {
                format!(
                    "{{\"name\":\"{}\",\"mount\":\"{}\",\"device\":\"{}\",\"fs\":\"{}\",\"total\":{},\"free\":{},\"external\":{}}}",
                    json_escape(&v.name),
                    json_escape(&v.mount_point),
                    json_escape(&v.device),
                    json_escape(&v.fs),
                    v.total,
                    v.free,
                    v.external,
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        o.raw("volumes", &format!("[{vols}]"));
        // Per-physical-disk health (I/O errors/retries/latency + SMART verdict).
        let health = self
            .disk_health
            .iter()
            .map(|h| {
                let nvme = match &h.nvme {
                    Some(n) => {
                        let sensors = n
                            .temp_sensors
                            .iter()
                            .filter(|t| **t > 0.0)
                            .map(|t| fmt_f(*t))
                            .collect::<Vec<_>>()
                            .join(",");
                        format!(
                            "{{\"temp_c\":{},\"percentage_used\":{},\"available_spare\":{},\"spare_threshold\":{},\"critical_warning\":{},\"tbw_tb\":{},\"power_on_hours\":{},\"media_errors\":{},\"temp_sensors\":[{sensors}]}}",
                            fmt_f(n.temp_c),
                            n.percentage_used,
                            n.available_spare,
                            n.spare_threshold,
                            n.critical_warning,
                            fmt_f(n.tbw() as f32),
                            n.power_on_hours,
                            n.media_errors,
                        )
                    }
                    None => "null".to_string(),
                };
                format!(
                    "{{\"bsd\":\"{}\",\"model\":\"{}\",\"external\":{},\"interconnect\":\"{}\",\"solid_state\":{},\"smart\":\"{}\",\"read_errors\":{},\"write_errors\":{},\"read_retries\":{},\"write_retries\":{},\"read_latency_ms\":{},\"write_latency_ms\":{},\"nvme\":{}}}",
                    json_escape(&h.bsd_name),
                    json_escape(&h.model),
                    h.external,
                    json_escape(&h.interconnect),
                    h.solid_state,
                    json_escape(&h.smart),
                    h.read_errors,
                    h.write_errors,
                    h.read_retries,
                    h.write_retries,
                    fmt_f(h.read_latency_ms),
                    fmt_f(h.write_latency_ms),
                    nvme,
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        o.raw("disk_health", &format!("[{health}]"));
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

        // top processes by memory footprint
        let mems = self
            .top_mem
            .iter()
            .map(|p| {
                format!(
                    "{{\"pid\":{},\"mem\":{},\"name\":\"{}\"}}",
                    p.pid,
                    p.mem,
                    json_escape(&p.name)
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        o.raw("top_mem", &format!("[{mems}]"));

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

pub fn json_escape(s: &str) -> String {
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
        // A stopped fan at idle (no airflow commanded) is normal on Apple Silicon —
        // it must NOT trip an alert.
        s.fan_rpm = 0;
        s.fan_target = 0;
        assert_eq!(s.compute_level(), Level::Ok);
        // ...but a stalled fan WHILE the controller is commanding airflow is danger.
        s.fan_target = 2000;
        assert_eq!(s.compute_level(), Level::Alert);
        // ...and a zero fan with no SMC envelope must not false-alarm.
        s.fan_max = 0;
        assert_eq!(s.compute_level(), Level::Ok);
    }

    #[test]
    fn fan_failed_needs_commanded_airflow() {
        let mut s = Snapshot::default();
        s.fan_max = 7826;
        // Cool Mac: controller commands nothing, fan legitimately stopped.
        s.fan_target = 0;
        s.fan_rpm = 0;
        assert!(!s.fan_failed());
        // Under load: airflow commanded and the fan is spinning — healthy.
        s.fan_target = 2317;
        s.fan_rpm = 2300;
        assert!(!s.fan_failed());
        // Airflow commanded but the fan is dead — the genuine failure.
        s.fan_rpm = 0;
        assert!(s.fan_failed());
        // No SMC envelope at all: never a failure, even with a stale target.
        s.fan_max = 0;
        assert!(!s.fan_failed());
    }

    #[test]
    fn fan_failed_catches_a_dead_secondary_fan() {
        use crate::ffi::smc::FanReading;
        let mut s = Snapshot::default();
        // Primary fan healthy (commanded and spinning).
        s.fan_max = 7826;
        s.fan_target = 2317;
        s.fan_rpm = 2300;
        // Secondary fan: airflow commanded but stalled — a real failure the watchdog
        // would miss if it only watched the primary.
        s.fans = vec![
            FanReading {
                rpm: 2300,
                min: 2317,
                max: 7826,
                target: 2317,
            },
            FanReading {
                rpm: 0,
                min: 2317,
                max: 7826,
                target: 2317,
            },
        ];
        assert!(s.fan_failed());
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
            mem: 0,
            name: "a\\b".into(),
        });
        let j = s.to_json();
        assert!(j.starts_with('{') && j.ends_with('}'));
        assert_eq!(j.matches('{').count(), j.matches('}').count());
        assert_eq!(j.matches('[').count(), j.matches(']').count());
        assert!(j.contains("\"level\":\"OK\""));
        assert!(j.contains("\"schema_version\":\"1\"")); // agent contract version
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
