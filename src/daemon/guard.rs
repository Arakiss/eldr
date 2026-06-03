//! The guard: a background loop that refreshes status.json, detects thermal
//! anomalies, and alerts passively (system notification + cmux badge). The watchdog's
//! armed interventions (M5) plug into [`Guard::on_recovery`] / the CONFIRM counter.
//!
//! Safety posture (inherited from the bash prototype): the guard only ever NOTIFIES.
//! It never kills, shuts down, or sends an agent a prompt.

use crate::config;
use crate::daemon::cmux;
use crate::sensors::snapshot::{Level, Snapshot, Thermal};
use core::ffi::c_int;
use std::sync::atomic::{AtomicBool, Ordering};

const SAMPLE_MS: u64 = 500;
static STOP: AtomicBool = AtomicBool::new(false);

const SIGINT: c_int = 2;
const SIGTERM: c_int = 15;

unsafe extern "C" {
    fn signal(signum: c_int, handler: extern "C" fn(c_int)) -> usize;
    fn getpid() -> i32;
    fn kill(pid: i32, sig: c_int) -> c_int;
}

extern "C" fn on_signal(_sig: c_int) {
    STOP.store(true, Ordering::SeqCst);
}

/// Run the guard loop until stopped (signal or `guard-stop`). `interval_secs` is the
/// sampling cadence.
pub fn run(interval_secs: u64) -> i32 {
    if let Some(pid) = running_pid() {
        eprintln!("eldr guard: already running (pid {pid})");
        return 0;
    }

    unsafe {
        signal(SIGINT, on_signal);
        signal(SIGTERM, on_signal);
    }

    config::ensure_data_dir();
    let pidfile = config::pid_path();
    let _ = std::fs::write(&pidfile, unsafe { getpid() }.to_string());

    let interval_ms = (interval_secs.max(1)) * 1000;
    eprintln!(
        "eldr guard: every {interval_secs}s -> {}",
        config::status_path().display()
    );

    let mut last = Level::Ok;
    while !STOP.load(Ordering::SeqCst) {
        let mut snap = Snapshot::gather(SAMPLE_MS);
        snap.source = "guard".into();
        let _ = snap.write_status();

        handle_transitions(&snap, &mut last);

        // Sleep the interval in small chunks so a stop signal is responsive.
        let mut slept = 0u64;
        while slept < interval_ms && !STOP.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(200));
            slept += 200;
        }
    }

    // Clean shutdown: clear badges, remove pid file.
    cmux::clear_all();
    let _ = std::fs::remove_file(&pidfile);
    eprintln!("eldr guard: stopped");
    0
}

/// React to a level transition: alert on entering a non-OK level, recover on return.
fn handle_transitions(s: &Snapshot, last: &mut Level) {
    if s.level != Level::Ok && s.level != *last {
        // Entered (or escalated to) a non-OK level.
        log_alert(s);
        notify_os(
            &format!("eldr {}", s.level.as_str()),
            &format!(
                "cpu {:.0}°C · fan {} · {}",
                s.cpu_temp,
                s.fan_rpm,
                s.thermal.as_str()
            ),
        );
        snapshot_processes(s);
        let color = if s.level == Level::Alert { "#f85149" } else { "#d29922" };
        cmux::badge_all(
            s.level.as_str(),
            &format!("{:.0}°C {}rpm", s.cpu_temp, s.fan_rpm),
            color,
        );
        if s.level == Level::Alert {
            cmux::notify_all(
                "eldr · thermal anomaly",
                &format!("thermal {}", s.thermal.as_str()),
                &format!(
                    "cpu {:.0}°C · fan {}rpm. Reversible actions only; don't power anything off.",
                    s.cpu_temp, s.fan_rpm
                ),
            );
        }
    } else if s.level == Level::Ok && *last != Level::Ok {
        // Recovered.
        cmux::clear_all();
    }
    *last = s.level;
}

fn log_alert(s: &Snapshot) {
    let line = format!(
        "{} {} cpu={:.0}C fan={}rpm thermal={}\n",
        s.ts,
        s.level.as_str(),
        s.cpu_temp,
        s.fan_rpm,
        s.thermal.as_str()
    );
    append(&config::alerts_path(), &line);
}

/// Snapshot the top processes at alert time (forensics; never mutated).
fn snapshot_processes(s: &Snapshot) {
    let mut block = format!(
        "## {} {} cpu={:.0}C fan={}rpm thermal={}\n",
        s.ts,
        s.level.as_str(),
        s.cpu_temp,
        s.fan_rpm,
        s.thermal.as_str()
    );
    for p in s.top_procs.iter().take(8) {
        block.push_str(&format!("  {:>6}  {:>5.1}%  {}\n", p.pid, p.cpu, p.name));
    }
    block.push('\n');
    append(&config::processes_path(), &block);
}

fn append(path: &std::path::Path, text: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(text.as_bytes());
    }
}

/// macOS system notification via osascript (a system tool, not a crate dep).
fn notify_os(title: &str, body: &str) {
    let script = format!(
        "display notification \"{}\" with title \"{}\" sound name \"Basso\"",
        osa_escape(body),
        osa_escape(title)
    );
    let _ = std::process::Command::new("osascript")
        .args(["-e", &script])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

fn osa_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// PID of a live guard, if any (reads the pid file and probes the process).
pub fn running_pid() -> Option<i32> {
    let txt = std::fs::read_to_string(config::pid_path()).ok()?;
    let pid: i32 = txt.trim().parse().ok()?;
    // signal 0 probes existence without delivering a signal.
    if unsafe { kill(pid, 0) } == 0 { Some(pid) } else { None }
}

/// Stop a running guard (SIGTERM). Returns true if one was signalled.
pub fn stop() -> bool {
    if let Some(pid) = running_pid() {
        unsafe { kill(pid, SIGTERM) };
        true
    } else {
        let _ = std::fs::remove_file(config::pid_path());
        false
    }
}

/// Mark the watchdog thermal pressure as the danger signal for sustained-critical
/// counting (used by the M5 watchdog). Kept here so guard + watchdog agree.
pub fn is_critical(s: &Snapshot) -> bool {
    s.thermal == Thermal::Critical || (s.fan_max > 0 && s.fan_rpm < 500)
}
