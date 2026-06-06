//! The guard: a background loop that refreshes status.json, detects thermal
//! anomalies, and alerts passively (system notification + cmux badge). The watchdog's
//! armed interventions (M5) plug into [`Guard::on_recovery`] / the CONFIRM counter.
//!
//! Safety posture (inherited from the bash prototype): the guard only ever NOTIFIES.
//! It never kills, shuts down, or sends an agent a prompt.

use crate::config;
use crate::daemon::cmux;
use crate::daemon::watchdog::Watchdog;
use crate::sensors::snapshot::{DiskHealth, Level, Snapshot, Thermal};
use core::ffi::c_int;
use std::collections::{HashMap, HashSet};
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
    let wd = Watchdog::load();
    eprintln!(
        "eldr guard: every {interval_secs}s -> {}  (watchdog: confirm={} interrupt={} checkpoint={} suspend={} dryrun={})",
        config::status_path().display(),
        wd.confirm,
        wd.interrupt,
        wd.checkpoint,
        wd.suspend,
        wd.dryrun,
    );

    let mut last = Level::Ok;
    let mut crit: u32 = 0; // consecutive sustained-critical samples
    let mut fired = false; // one intervention per critical episode
    let mut history: Vec<(f64, u32, f32)> = Vec::new();
    let mut disk_prev: HashMap<String, (u64, u64)> = HashMap::new();
    let mut disk_alerted: HashSet<String> = HashSet::new();
    while !STOP.load(Ordering::SeqCst) {
        let mut snap = Snapshot::gather(SAMPLE_MS);
        snap.source = "guard".into();
        snap.read_smart();
        let _ = snap.write_status();
        push_history(&mut history, &snap);

        handle_transitions(&snap, &mut last);
        watch_disk_health(&snap, &mut disk_prev, &mut disk_alerted);

        // Sustained-critical gating for armed interventions.
        if is_critical(&snap) {
            crit += 1;
        } else {
            crit = 0;
        }
        if crit >= wd.confirm && !fired {
            // Always record what it WOULD do (confidence-building), then act per arming.
            wd.observe(&snap);
            wd.intervene(&snap, false);
            fired = true;
        }
        if snap.level == Level::Ok && fired {
            wd.unintervene();
            fired = false;
        }

        // Sleep the interval in small chunks so a stop signal is responsive.
        let mut slept = 0u64;
        while slept < interval_ms && !STOP.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(200));
            slept += 200;
        }
    }

    // On shutdown, resume anything we paused (don't leave a process suspended).
    wd.unintervene();

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
        let color = if s.level == Level::Alert {
            "#f85149"
        } else {
            "#d29922"
        };
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
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
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
    if unsafe { kill(pid, 0) } == 0 {
        Some(pid)
    } else {
        None
    }
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

/// The danger signal for sustained-critical counting (used by the M5 watchdog): macOS
/// thermal pressure at its peak, or a genuinely failed fan. A fan stopped at idle is
/// normal on Apple Silicon and must NOT arm interventions — see [`Snapshot::fan_failed`].
pub fn is_critical(s: &Snapshot) -> bool {
    s.thermal == Thermal::Critical || s.fan_failed()
}

/// Watch each physical disk for the earliest degradation signals: a firmware SMART
/// "failing" verdict, or I/O error/retry counters that grow between samples. Notifies
/// once per disk per episode and clears the latch once the disk reads healthy again, so
/// a later resurgence re-alerts. This NEVER intervenes — a degrading disk calls for a
/// human (back up, replace), not an automated action the watchdog could take.
fn watch_disk_health(
    s: &Snapshot,
    prev: &mut HashMap<String, (u64, u64)>,
    alerted: &mut HashSet<String>,
) {
    for h in &s.disk_health {
        if h.bsd_name.is_empty() {
            continue;
        }
        let cur = (h.errors(), h.retries());
        let grew = counters_grew(prev.get(&h.bsd_name).copied(), cur);
        prev.insert(h.bsd_name.clone(), cur);
        let failing = h.smart_failing();
        let nvme = h.nvme_alarm();
        let trigger = failing || nvme.is_some() || grew;

        if trigger && alerted.insert(h.bsd_name.clone()) {
            let label = if h.model.is_empty() {
                h.bsd_name.clone()
            } else {
                format!("{} ({})", h.model, h.bsd_name)
            };
            let (title, body) = if failing {
                (
                    format!("eldr · disk {} SMART failing", h.bsd_name),
                    format!("{label}: firmware predicts failure — back up now."),
                )
            } else if let Some(reason) = nvme {
                (
                    format!("eldr · disk {} {reason}", h.bsd_name),
                    format!("{label}: {reason} — back up and check the disk."),
                )
            } else {
                (
                    format!("eldr · disk {} I/O errors", h.bsd_name),
                    format!("{label}: errors rising (err {} · retry {}).", cur.0, cur.1),
                )
            };
            log_disk_alert(s, h, failing, nvme, cur);
            notify_os(&title, &body);
            cmux::badge_all("DISK", &h.bsd_name, "#f85149");
        } else if !trigger {
            alerted.remove(&h.bsd_name);
        }
    }
}

/// True when either error or retry counter rose since the previous sample. A first
/// sighting (no previous reading) never counts as growth — cumulative counters are only
/// meaningful as a delta.
fn counters_grew(prev: Option<(u64, u64)>, cur: (u64, u64)) -> bool {
    prev.map(|(e, r)| cur.0 > e || cur.1 > r).unwrap_or(false)
}

fn log_disk_alert(s: &Snapshot, h: &DiskHealth, failing: bool, nvme: Option<&str>, cur: (u64, u64)) {
    let smart = if h.smart.is_empty() {
        "unknown"
    } else {
        h.smart.as_str()
    };
    let kind = if failing {
        "FAILING"
    } else if nvme.is_some() {
        "NVME"
    } else {
        "ERRORS"
    };
    let detail = nvme.map(|r| format!(" nvme=\"{r}\"")).unwrap_or_default();
    let line = format!(
        "{} DISK {} {} smart={} err={} retry={}{}\n",
        s.ts, h.bsd_name, kind, smart, cur.0, cur.1, detail,
    );
    append(&config::alerts_path(), &line);
}

const HISTORY_LEN: usize = 64;

/// Append a telemetry sample and rewrite the rolling history file (≤ `HISTORY_LEN` lines
/// of `cpu_load,fan_rpm,sys_power`), so the TUI can open with populated sparklines. Cheap:
/// the file is tiny and rewritten in full each sample.
fn push_history(hist: &mut Vec<(f64, u32, f32)>, s: &Snapshot) {
    hist.push(((s.cpu_load_pct * 100.0) as f64, s.fan_rpm, s.sys_power));
    if hist.len() > HISTORY_LEN {
        let drop = hist.len() - HISTORY_LEN;
        hist.drain(0..drop);
    }
    let body: String = hist
        .iter()
        .map(|(c, r, p)| format!("{c:.1},{r},{p:.1}\n"))
        .collect();
    let _ = std::fs::write(config::history_path(), body);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_growth_detection() {
        // First sighting is never "growth": a raw cumulative count says nothing alone.
        assert!(!counters_grew(None, (5, 2)));
        // Stable counters between samples: healthy, no alert.
        assert!(!counters_grew(Some((5, 2)), (5, 2)));
        // A new read error: growth.
        assert!(counters_grew(Some((5, 2)), (6, 2)));
        // A new retry: growth.
        assert!(counters_grew(Some((5, 2)), (5, 3)));
        // Counters can't fall on real hardware, but a reset must not false-alarm.
        assert!(!counters_grew(Some((9, 9)), (0, 0)));
    }

    #[test]
    fn smart_failing_is_strict() {
        let mut h = DiskHealth {
            smart: "verified".into(),
            ..Default::default()
        };
        assert!(!h.smart_failing());
        h.smart = "Failing".into();
        assert!(h.smart_failing());
        h.smart = "not supported".into();
        assert!(!h.smart_failing());
    }
}
