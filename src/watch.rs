//! `eldr watch` — a continuous one-line-per-sample stream, for an agent (or a person)
//! that wants to follow the machine over time without parsing the TUI. With `--json`
//! each line is the full snapshot as NDJSON; otherwise it's a terse status line. Ctrl-C
//! (SIGINT) stops it cleanly, and a closed pipe (e.g. `eldr watch --json | head`) ends it
//! quietly. Unlike the guard it only reads and prints — no status.json, no interventions.

use crate::sensors::snapshot::Snapshot;
use core::ffi::c_int;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

static STOP: AtomicBool = AtomicBool::new(false);
const SIGINT: c_int = 2;
const SIGTERM: c_int = 15;

unsafe extern "C" {
    fn signal(signum: c_int, handler: extern "C" fn(c_int)) -> usize;
}

extern "C" fn on_signal(_sig: c_int) {
    STOP.store(true, Ordering::SeqCst);
}

/// Stream one line per sample until interrupted. `interval_ms` is the per-sample window
/// (it doubles as the cadence, since `gather` sleeps it). `json` selects NDJSON vs terse.
pub fn run(interval_ms: u64, json: bool) -> i32 {
    unsafe {
        signal(SIGINT, on_signal);
        signal(SIGTERM, on_signal);
    }
    let mut out = std::io::stdout();
    while !STOP.load(Ordering::SeqCst) {
        let snap = Snapshot::gather(interval_ms);
        let line = if json { snap.to_json() } else { terse(&snap) };
        // A write error means the consumer closed the pipe — stop quietly.
        if writeln!(out, "{line}").is_err() || out.flush().is_err() {
            break;
        }
    }
    0
}

fn terse(s: &Snapshot) -> String {
    format!(
        "{} {} cpu={:.0}% temp={:.0}C fan={}rpm thermal={} pkg={:.1}W",
        s.ts,
        s.level.as_str(),
        s.cpu_load_pct * 100.0,
        s.cpu_temp,
        s.fan_rpm,
        s.thermal.as_str(),
        s.all_power,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terse_line_shape() {
        let mut s = Snapshot::default();
        s.ts = "2026-01-01T00:00:00Z".into();
        let l = terse(&s);
        assert!(l.starts_with("2026-01-01T00:00:00Z OK "));
        assert!(l.contains("cpu=") && l.contains("temp=") && l.contains("thermal="));
    }
}
