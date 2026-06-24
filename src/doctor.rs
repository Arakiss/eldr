//! `eldr doctor` — a one-shot health check of eldr itself: which sensor sources answer,
//! whether the guard is installed and running, where data lives, how it's configured, and
//! whether a newer version is known (from cache only — no network unless `eldr update`
//! has run). Local and fast; meant to be the first thing to run when something looks off.

use crate::config;
use crate::daemon::{launchd, watchdog::Watchdog};
use crate::sensors::snapshot::Snapshot;
use crate::sensors::system::SystemInfo;
use crate::ui::style::{Style, human_bytes};
use crate::update;

pub fn run() -> i32 {
    let st = Style::detect();
    let (d, z, b) = (st.dim, st.reset, st.bold);
    let ok = format!("{}✓{z}", st.green);
    let warn = format!("{}•{z}", st.yellow);
    let mark = |good: bool| if good { &ok } else { &warn };

    println!("\n  {b}eldr doctor{z}  {d}· self-check{z}\n");

    // ---- version ----
    println!("  {b}Version{z}");
    match update::cached_newer() {
        Some(latest) => println!(
            "    {warn} eldr {}  {d}— {latest} available · run `eldr update`{z}",
            update::current()
        ),
        None => println!("    {ok} eldr {}", update::current()),
    }

    // ---- machine ----
    let s = SystemInfo::get();
    println!("\n  {b}Machine{z}");
    println!(
        "    {ok} {} {d}({}){z} · macOS {} {d}({}){z} · {}",
        s.marketing, s.model_id, s.os_version, s.os_build, s.arch
    );
    println!(
        "    {ok} {} · {}P+{}E · {:.0} GB RAM",
        s.chip,
        s.p_cores,
        s.e_cores,
        s.ram_bytes as f64 / 1_073_741_824.0,
    );

    // ---- sensors ----
    let snap = Snapshot::gather(400);
    println!("\n  {b}Sensors{z}");
    println!(
        "    {} IOReport power/freq {d}— pkg {:.1} W · CPU {} MHz{z}",
        mark(snap.all_power > 0.0),
        snap.all_power,
        snap.pcpu_freq_mhz,
    );
    println!(
        "    {} temperatures {d}— CPU {:.0}° · GPU {:.0}°{z}",
        mark(snap.cpu_temp > 0.0),
        snap.cpu_temp,
        snap.gpu_temp,
    );
    println!(
        "    {} fan {d}— {} reported{z}",
        mark(snap.fan_max > 0),
        snap.fans.len(),
    );
    println!(
        "    {} thermal pressure {d}— {}{z}",
        mark(snap.thermal != crate::sensors::snapshot::Thermal::Unknown),
        snap.thermal.as_str(),
    );
    println!(
        "    {ok} per-core load {d}— {} cores{z}",
        snap.per_core.len()
    );
    println!(
        "    {ok} disks {d}— {} physical · {} volumes{z}",
        snap.disk_health.len(),
        snap.volumes.len(),
    );
    println!(
        "    {} network {d}— {}{z}",
        mark(snap.net.is_some()),
        snap.net
            .as_ref()
            .map(|_| "counters available")
            .unwrap_or("unavailable"),
    );
    println!(
        "    {} battery {d}— {}{z}",
        if snap.battery.is_some() { &ok } else { &warn },
        snap.battery
            .as_ref()
            .map(|bat| format!("{}%", bat.percent))
            .unwrap_or_else(|| "desktop (no battery)".to_string()),
    );

    // ---- guard ----
    println!("\n  {b}Guard{z}");
    match crate::daemon::guard::running_pid() {
        Some(pid) => println!("    {ok} running {d}(pid {pid}){z}"),
        None => println!("    {warn} not running {d}— `eldr guard` or `eldr guard-install`{z}"),
    }
    if launchd::installed() {
        println!("    {ok} LaunchAgent installed {d}(starts at login){z}");
    } else {
        println!("    {warn} LaunchAgent not installed {d}— `eldr guard-install` for 24/7{z}");
    }
    let wd = Watchdog::load();
    println!(
        "    {d}arming: cmux={} interrupt={} checkpoint={} suspend={} confirm={} dryrun={}{z}",
        wd.cmux as u8,
        wd.interrupt as u8,
        wd.checkpoint as u8,
        wd.suspend as u8,
        wd.confirm,
        wd.dryrun as u8,
    );

    // ---- storage / config ----
    println!("\n  {b}Files{z}");
    let dir = config::data_dir();
    println!(
        "    {ok} data dir {d}— {} ({}){z}",
        dir.display(),
        human_bytes(crate::daemon::maint::dir_size(&dir)),
    );
    let conf = config::default_path();
    if conf.exists() {
        println!("    {ok} config {d}— {}{z}", conf.display());
    } else {
        println!("    {warn} config {d}— none ({}){z}", conf.display());
    }
    let check_on = config::Config::load().flag("ELDR_UPDATE_CHECK", false);
    println!(
        "    {d}update check: {} (ELDR_UPDATE_CHECK){z}",
        if check_on { "on" } else { "off" },
    );

    // ---- install ----
    println!("\n  {b}Install{z}");
    if let Ok(exe) = std::env::current_exe() {
        let p = exe.display().to_string();
        let how = if p.contains("/Cellar/") || p.contains("/homebrew/") {
            "Homebrew"
        } else {
            "from source / manual"
        };
        println!("    {ok} {p} {d}({how}){z}");
        if let Some(parent) = exe.parent().and_then(|x| x.to_str()) {
            let on_path = std::env::var("PATH")
                .map(|v| v.split(':').any(|d| d == parent))
                .unwrap_or(false);
            if on_path {
                println!("    {ok} on PATH");
            } else {
                println!("    {warn} {parent} {d}is not on PATH{z}");
            }
        }
    }
    println!();
    0
}
