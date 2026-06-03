//! `eldr` — thin binary. Hand-rolled arg parsing (no `clap`), then dispatch to the
//! library. The library does the work; `main` only routes and sets exit codes.

use eldr::sensors::snapshot::Snapshot;
use eldr::ui::{pretty, tui};

/// Default IOReport sampling window for one-shot readings (`now`/`status`/`check`).
const DEFAULT_SAMPLE_MS: u64 = 500;

const USAGE: &str = "\
eldr — global hardware monitor + protective watchdog (Apple Silicon, no sudo)

USAGE:
    eldr <command> [options]

READINGS
    now                     one-shot snapshot (pretty)
    check                   terse line + exit 0/1/2 (OK/WARN/ALERT) — for agents
    status                  panel (live, or last guard sample)
    tui [--interval N]      live self-refreshing dashboard

GUARD
    guard [--interval N]    background monitor -> status.json, alerts, interventions
    guard-stop              stop a running guard
    guard-install           run guard 24/7 via launchd
    guard-uninstall         remove the launchd agent
    watchdog-test           dry-run: show exactly what an intervention would do

EXPERIMENT
    bench <label> [opts]    controlled load -> steady state
    report <label>          steady-state summary
    compare <a> <b>         iso-load delta + verdict

    -h, --help              this help";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("now");
    let rest = if args.is_empty() { &[][..] } else { &args[1..] };

    let code = dispatch(cmd, rest);
    std::process::exit(code);
}

/// Extract the value following `--flag` in `args`.
fn opt<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str())
}

fn dispatch(cmd: &str, rest: &[String]) -> i32 {
    match cmd {
        "tui" => {
            // --interval is in seconds (parity with the prototype).
            let secs = opt(rest, "--interval").and_then(|v| v.parse::<f64>().ok()).unwrap_or(1.0);
            let ms = (secs * 1000.0).max(200.0) as u64;
            tui::run(ms);
            0
        }
        "now" => {
            let snap = Snapshot::gather(DEFAULT_SAMPLE_MS);
            let _ = snap.write_status();
            pretty::panel(&snap, "(live)");
            0
        }
        "status" => {
            let snap = Snapshot::gather(DEFAULT_SAMPLE_MS);
            let _ = snap.write_status();
            pretty::panel(&snap, "(live)");
            0
        }
        "check" => {
            let snap = Snapshot::gather(DEFAULT_SAMPLE_MS);
            let _ = snap.write_status();
            pretty::check_line(&snap);
            snap.level.exit_code()
        }
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            0
        }
        // Wired in later milestones.
        "guard" | "guard-stop" | "guard-install" | "guard-uninstall" | "watchdog-test"
        | "bench" | "report" | "compare" => {
            eprintln!("eldr: '{cmd}' not implemented yet (in progress)");
            1
        }
        other => {
            eprintln!("eldr: unknown command '{other}'\n");
            println!("{USAGE}");
            2
        }
    }
}
