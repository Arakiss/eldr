//! Controlled thermal experiment: drive a fixed load, sample to a CSV, then summarize
//! the steady state (tail window) and compare two iso-load runs. Ported from the bash
//! prototype. The discipline: a passive baseline is confounded by ambient drift and
//! unmatched load, so run two matched runs back-to-back and compare their steady state.

use crate::config;
use crate::ffi::mach;
use crate::sensors::snapshot::Snapshot;
use core::ffi::c_int;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

const SAMPLE_MS: u64 = 500;
static STOP: AtomicBool = AtomicBool::new(false);

unsafe extern "C" {
    fn signal(signum: c_int, handler: extern "C" fn(c_int)) -> usize;
}
extern "C" fn on_signal(_s: c_int) {
    STOP.store(true, Ordering::SeqCst);
}

fn ncpu() -> u64 {
    mach::sysctl_u64("hw.ncpu").unwrap_or(8)
}

/// Spawn the load. Returns the child processes to reap when done.
fn spawn_load(cmd: Option<&str>) -> (String, Vec<Child>) {
    let null = || (Stdio::null(), Stdio::null());
    if let Some(c) = cmd {
        let (o, e) = null();
        let child = Command::new("bash").arg("-c").arg(c).stdout(o).stderr(e).spawn();
        return (format!("custom load: {c}"), child.into_iter().collect());
    }
    let n = ncpu();
    // Prefer stress-ng (matrixprod is a stable, heat-dense workload).
    if Command::new("stress-ng").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false) {
        let (o, e) = null();
        let child = Command::new("stress-ng")
            .args(["--cpu", &n.to_string(), "--cpu-method", "matrixprod"])
            .stdout(o)
            .stderr(e)
            .spawn();
        return (format!("stress-ng matrixprod x{n}"), child.into_iter().collect());
    }
    // Fallback: N spinning `yes` processes (weaker, uneven, but dependency-free).
    let mut kids = Vec::new();
    for _ in 0..n {
        if let Ok(c) = Command::new("yes").stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
            kids.push(c);
        }
    }
    (format!("yes x{n} (weak)"), kids)
}

/// `eldr bench <label>` — run a fixed load for `dur_s`, sampling every `interval_s`.
pub fn bench(label: &str, dur_s: u64, interval_s: u64, cmd: Option<&str>) -> i32 {
    unsafe {
        signal(2, on_signal); // SIGINT
        signal(15, on_signal); // SIGTERM
    }
    let dir = config::ensure_data_dir();
    let csv = dir.join(format!("{label}.csv"));
    if let Err(e) = std::fs::write(
        &csv,
        "ts,elapsed,cpu_temp,gpu_temp,fan_rpm,thermal,cpu_power,pkg_power,cpu_load\n",
    ) {
        eprintln!("eldr: cannot write {}: {e}", csv.display());
        return 1;
    }

    let (desc, mut kids) = spawn_load(cmd);
    println!("eldr bench '{label}' — {desc} · {dur_s}s · sample/{interval_s}s");
    println!("  run the matched runs the SAME day, same room temperature.");

    let start = std::time::Instant::now();
    let interval_ms = interval_s.max(1) * 1000;
    while start.elapsed().as_secs() < dur_s && !STOP.load(Ordering::SeqCst) {
        // If a custom load finished on its own, stop early.
        if cmd.is_some()
            && let Some(first) = kids.first_mut()
            && matches!(first.try_wait(), Ok(Some(_)))
        {
            println!("  (the load finished on its own at {}s)", start.elapsed().as_secs());
            break;
        }
        let s = Snapshot::gather(SAMPLE_MS);
        let elapsed = start.elapsed().as_secs();
        append_row(&csv, &s, elapsed);
        println!(
            "  t={elapsed:>4}s  CPU {ct:>2.0}°C  fan {rpm:>4}  {th}",
            ct = s.cpu_temp,
            rpm = s.fan_rpm,
            th = s.thermal.as_str()
        );
        // Sleep the remainder of the interval (sampling already took ~SAMPLE_MS).
        let mut slept = SAMPLE_MS;
        while slept < interval_ms && !STOP.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(200));
            slept += 200;
        }
    }

    for k in &mut kids {
        let _ = k.kill();
        let _ = k.wait();
    }
    println!("done -> {}", csv.display());
    report(label, 300)
}

fn append_row(csv: &std::path::Path, s: &Snapshot, elapsed: u64) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(csv) {
        let _ = writeln!(
            f,
            "{},{},{:.1},{:.1},{},{},{:.2},{:.2},{:.3}",
            s.ts,
            elapsed,
            s.cpu_temp,
            s.gpu_temp,
            s.fan_rpm,
            s.thermal.as_str(),
            s.cpu_power,
            s.all_power,
            s.cpu_load_pct
        );
    }
}

// MARK: report / compare

struct Steady {
    cpu_avg: f32,
    cpu_max: f32,
    fan_avg: f32,
    worst_thermal: String,
    n: usize,
}

fn thermal_rank(t: &str) -> u8 {
    match t {
        "fair" => 1,
        "serious" => 2,
        "critical" => 3,
        _ => 0,
    }
}

/// Summarize the steady state: the last `tail_s` seconds of the run.
fn summarize(label: &str, tail_s: u64) -> Option<Steady> {
    let csv = config::data_dir().join(format!("{label}.csv"));
    let text = std::fs::read_to_string(&csv).ok()?;
    let rows: Vec<Vec<String>> = text
        .lines()
        .skip(1)
        .map(|l| l.split(',').map(|s| s.to_string()).collect())
        .filter(|r: &Vec<String>| r.len() >= 6)
        .collect();
    if rows.is_empty() {
        return None;
    }
    let max_elapsed: u64 = rows.iter().filter_map(|r| r[1].parse().ok()).max().unwrap_or(0);
    let threshold = max_elapsed.saturating_sub(tail_s);

    let mut cpu = Vec::new();
    let mut fan = Vec::new();
    let mut worst = 0u8;
    for r in &rows {
        let elapsed: u64 = r[1].parse().unwrap_or(0);
        if elapsed < threshold {
            continue;
        }
        if let Ok(c) = r[2].parse::<f32>() {
            cpu.push(c);
        }
        if let Ok(f) = r[4].parse::<f32>() {
            fan.push(f);
        }
        worst = worst.max(thermal_rank(&r[5]));
    }
    if cpu.is_empty() {
        return None;
    }
    let cpu_avg = cpu.iter().sum::<f32>() / cpu.len() as f32;
    let cpu_max = cpu.iter().cloned().fold(0.0_f32, f32::max);
    let fan_avg = if fan.is_empty() { 0.0 } else { fan.iter().sum::<f32>() / fan.len() as f32 };
    let worst_thermal = ["nominal", "fair", "serious", "critical"][worst as usize].to_string();
    Some(Steady { cpu_avg, cpu_max, fan_avg, worst_thermal, n: cpu.len() })
}

/// `eldr report <label>` — steady-state summary.
pub fn report(label: &str, tail_s: u64) -> i32 {
    match summarize(label, tail_s) {
        Some(s) => {
            println!(
                "{label:<12} last {tail_s}s n={n}  ·  CPU avg {avg:.1}°C (max {max:.0})  fan avg {fan:.0} RPM  worst thermal {th}",
                n = s.n,
                avg = s.cpu_avg,
                max = s.cpu_max,
                fan = s.fan_avg,
                th = s.worst_thermal,
            );
            0
        }
        None => {
            eprintln!("no steady-state data for '{label}'");
            1
        }
    }
}

/// `eldr compare <a> <b>` — iso-load steady-state delta + verdict.
pub fn compare(a: &str, b: &str, tail_s: u64) -> i32 {
    let (Some(sa), Some(sb)) = (summarize(a, tail_s), summarize(b, tail_s)) else {
        eprintln!("missing data in one of the two runs");
        return 1;
    };
    println!("Steady state (last {tail_s}s) — identical load");
    println!("  {a:<10} CPU {:>5.1}°C   fan {:>5.0} RPM   worst thermal {}", sa.cpu_avg, sa.fan_avg, sa.worst_thermal);
    println!("  {b:<10} CPU {:>5.1}°C   fan {:>5.0} RPM   worst thermal {}", sb.cpu_avg, sb.fan_avg, sb.worst_thermal);
    let dt = sb.cpu_avg - sa.cpu_avg;
    let dr = sb.fan_avg - sa.fan_avg;
    println!("  delta      CPU {dt:>+5.1}°C   fan {dr:>+5.0} RPM");
    print!("\nReading: ");
    if dr > 150.0 && dt.abs() < 1.5 {
        println!("{b} retains heat — same °C but the fan spins {dr:.0} RPM more.");
    } else if dt > 1.5 {
        println!("{b} runs {dt:.1}°C hotter at similar RPM — retains heat.");
    } else if dr < -150.0 {
        println!("{b} runs cooler ({:.0} RPM less): check the ambient was the same.", -dr);
    } else {
        println!("no measurable effect, within noise — the case doesn't trap heat.");
    }
    println!("  (valid only with the same load and the same ambient temperature.)");
    0
}
