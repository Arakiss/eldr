//! `eldr` — thin binary. Hand-rolled arg parsing (no `clap`), then dispatch to the
//! library. The library does the work; `main` only routes and sets exit codes.

use eldr::daemon::{bench, guard, launchd, scrub, watchdog};
use eldr::sensors::snapshot::Snapshot;
use eldr::sensors::system::SystemInfo;
use eldr::ui::{pretty, tui};
use eldr::watch;

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
    watch [--interval N]    stream one line per sample (--json = NDJSON) — for agents
    disk                    per-volume usage + per-disk health (SMART, I/O errors)
    system                  static machine identity (model, serial, macOS, SSD)
    sensors                 every SMC sensor — temps, fans, power, current, voltage

GUARD
    guard [--interval N]    background monitor -> status.json, alerts, interventions
    guard-stop              stop a running guard
    guard-install           run guard 24/7 via launchd
    guard-uninstall         remove the launchd agent
    watchdog-test           dry-run: show exactly what an intervention would do

ACTIONS (reversible; for agents)
    suspend <pid>           SIGSTOP a process (refuses protected ones)
    resume <pid>            SIGCONT a suspended process
    checkpoint <path>       non-destructive git stash-create snapshot of a dirty repo

INTEGRITY
    scrub init <path>       fingerprint a tree (SHA-256) into a manifest
    scrub verify <path>     re-hash and report bit rot, edits, new/missing
                            (--notify alerts on corruption for scheduled runs)
    scrub status [path]     manifest summary

EXPERIMENT
    bench <label> [opts]    controlled load -> steady state
    report <label>          steady-state summary
    compare <a> <b>         iso-load delta + verdict

    --json                  machine-readable JSON on stdout (now/status/check/disk/
                            system/sensors/scrub) — for agents
    -h, --help              this help
    -V, --version           print version";

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

/// True when `--json` is present — machine-readable output to stdout, for agents.
fn json_wanted(args: &[String]) -> bool {
    args.iter().any(|a| a == "--json")
}

fn dispatch(cmd: &str, rest: &[String]) -> i32 {
    match cmd {
        "tui" => {
            // --interval is in seconds (parity with the prototype).
            let secs = opt(rest, "--interval")
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(1.0);
            let ms = (secs * 1000.0).max(200.0) as u64;
            tui::run(ms);
            0
        }
        "watch" => {
            // --interval in seconds; the sample window doubles as the cadence.
            let secs = opt(rest, "--interval")
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(2.0);
            let ms = (secs * 1000.0).max(250.0) as u64;
            watch::run(ms, json_wanted(rest))
        }
        "now" | "status" => {
            let snap = Snapshot::gather(DEFAULT_SAMPLE_MS);
            let _ = snap.write_status();
            if json_wanted(rest) {
                println!("{}", snap.to_json());
            } else {
                pretty::panel(&snap, "(live)");
            }
            0
        }
        "system" => {
            let info = SystemInfo::get();
            if json_wanted(rest) {
                println!("{}", info.to_json());
            } else {
                info.render();
            }
            0
        }
        "disk" => {
            let mut snap = Snapshot::gather(DEFAULT_SAMPLE_MS);
            snap.read_smart();
            let _ = snap.write_status();
            if json_wanted(rest) {
                println!("{}", snap.to_json());
                snap.disk_exit_code()
            } else {
                pretty::disk_panel(&snap)
            }
        }
        "sensors" => {
            if json_wanted(rest) {
                pretty::sensors_json();
            } else {
                pretty::sensors_panel();
            }
            0
        }
        "check" => {
            let snap = Snapshot::gather(DEFAULT_SAMPLE_MS);
            let _ = snap.write_status();
            if json_wanted(rest) {
                println!("{}", snap.to_json());
            } else {
                pretty::check_line(&snap);
            }
            snap.level.exit_code()
        }
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            0
        }
        "-V" | "--version" | "version" => {
            println!("eldr {}", env!("CARGO_PKG_VERSION"));
            0
        }
        "guard" => {
            let secs = opt(rest, "--interval")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(30);
            guard::run(secs)
        }
        "guard-stop" => {
            if guard::stop() {
                println!("eldr guard stopped");
            } else {
                println!("no guard was running");
            }
            0
        }
        "guard-install" => launchd::install(),
        "guard-uninstall" => launchd::uninstall(),
        "watchdog-test" => watchdog::test_report(),
        "suspend" | "resume" => {
            let Some(pid) = rest
                .iter()
                .find(|a| !a.starts_with("--"))
                .and_then(|s| s.parse::<i32>().ok())
            else {
                eprintln!("usage: eldr {cmd} <pid>");
                return 2;
            };
            if cmd == "suspend" {
                watchdog::suspend_pid(pid, json_wanted(rest))
            } else {
                watchdog::resume_pid(pid, json_wanted(rest))
            }
        }
        "checkpoint" => {
            let Some(path) = rest.iter().find(|a| !a.starts_with("--")) else {
                eprintln!("usage: eldr checkpoint <path>");
                return 2;
            };
            watchdog::checkpoint_path(path, json_wanted(rest))
        }
        "scrub" => scrub::run(rest),
        "bench" => {
            let Some(label) = rest.iter().find(|a| !a.starts_with("--")) else {
                eprintln!("usage: eldr bench <label> [--dur N --interval N --cmd \"...\"]");
                return 2;
            };
            let dur = opt(rest, "--dur")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1200);
            let interval = opt(rest, "--interval")
                .and_then(|v| v.parse().ok())
                .unwrap_or(15);
            let load = opt(rest, "--cmd");
            bench::bench(label, dur, interval, load)
        }
        "report" => {
            let Some(label) = rest.iter().find(|a| !a.starts_with("--")) else {
                eprintln!("usage: eldr report <label> [--tail N]");
                return 2;
            };
            let tail = opt(rest, "--tail")
                .and_then(|v| v.parse().ok())
                .unwrap_or(300);
            bench::report(label, tail)
        }
        "compare" => {
            let labels: Vec<&String> = rest.iter().filter(|a| !a.starts_with("--")).collect();
            if labels.len() < 2 {
                eprintln!("usage: eldr compare <a> <b> [--tail N]");
                return 2;
            }
            let tail = opt(rest, "--tail")
                .and_then(|v| v.parse().ok())
                .unwrap_or(300);
            bench::compare(labels[0], labels[1], tail)
        }
        other => {
            eprintln!("eldr: unknown command '{other}'\n");
            println!("{USAGE}");
            2
        }
    }
}
