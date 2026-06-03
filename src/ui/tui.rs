//! The Eldr live dashboard: an owned ANSI render over [`crate::ui::term`]. Per-core
//! bars, RAM gauge, GPU/ANE, thermal/fan, history sparklines and top processes.
//! Quits cleanly on `q` or Ctrl-C; the [`RawMode`] guard restores the terminal.

use crate::sensors::snapshot::{Level, Snapshot, Thermal};
use crate::ui::style::{Style, bar, gib, sparkline};
use crate::ui::term::{self, RawMode};
use std::io::Write;

const SAMPLE_MS: u64 = 250;
const HIST: usize = 48;

/// Run the dashboard until the user quits. `interval_ms` is the target refresh.
pub fn run(interval_ms: u64) {
    let Some(_raw) = RawMode::enter() else {
        eprintln!("eldr: not a terminal (tui needs a tty)");
        return;
    };

    let mut cpu_hist: Vec<f64> = Vec::with_capacity(HIST);
    let mut rpm_hist: Vec<f64> = Vec::with_capacity(HIST);

    loop {
        let mut snap = Snapshot::gather(SAMPLE_MS);
        snap.source = "tui".into();
        let _ = snap.write_status();

        push_hist(&mut cpu_hist, (snap.cpu_load_pct * 100.0) as f64);
        push_hist(&mut rpm_hist, snap.fan_rpm as f64);

        let frame = render(&snap, &cpu_hist, &rpm_hist);
        let mut out = std::io::stdout();
        let _ = out.write_all(frame.as_bytes());
        let _ = out.flush();

        // Wait for the rest of the interval; a key cuts it short.
        let wait = interval_ms.saturating_sub(SAMPLE_MS).max(50) as i32;
        if let Some(k) = term::read_key(wait)
            && (k == b'q' || k == b'Q' || k == 3)
        {
            break;
        }
    }
}

fn push_hist(h: &mut Vec<f64>, v: f64) {
    h.push(v);
    if h.len() > HIST {
        h.remove(0);
    }
}

fn level_color(st: &Style, lvl: Level) -> &'static str {
    match lvl {
        Level::Ok => st.green,
        Level::Warn => st.yellow,
        Level::Alert => st.red,
    }
}

fn thermal_color(st: &Style, t: Thermal) -> &'static str {
    match t {
        Thermal::Nominal => st.green,
        Thermal::Fair => st.yellow,
        Thermal::Serious | Thermal::Critical => st.red,
        Thermal::Unknown => st.dim,
    }
}

fn render(s: &Snapshot, cpu_hist: &[f64], rpm_hist: &[f64]) -> String {
    let st = Style::color(); // we are in a tty (raw mode)
    let d = st.dim;
    let z = st.reset;
    let b = st.bold;
    let lc = level_color(&st, s.level);
    let tc = thermal_color(&st, s.thermal);

    let mut f = String::with_capacity(2048);
    f.push_str(term::home());

    let line = |row: String, out: &mut String| {
        out.push_str(&row);
        out.push_str(term::clear_eol());
        out.push('\n');
    };

    let rule = "─".repeat(60);
    // ISO timestamp "YYYY-MM-DDTHH:MM:SSZ" -> "HH:MM:SS"; tolerate short/empty ts.
    let clock = if s.ts.len() >= 19 { &s.ts[11..19] } else { "--:--:--" };

    line(
        format!(
            " {b}eldr{z} {d}·{z} {chip} {d}·{z} {p}P+{e}E {d}·{z} {g} GPU{pad}{d}{clock}{z}",
            b = b, z = z, d = d, chip = s.chip, p = s.p_cores, e = s.e_cores, g = s.gpu_cores,
            pad = " ".repeat(pad_to(&s.chip, s.p_cores, s.e_cores, s.gpu_cores)),
            clock = clock,
        ),
        &mut f,
    );
    line(format!(" {d}{rule}{z}", d = d, z = z, rule = rule), &mut f);

    line(
        format!(
            " {d}STATE{z}  {lc}{b}{lvl:<6}{z}  {d}thermal{z} {tc}{th}{z}",
            d = d, z = z, lc = lc, b = b, lvl = s.level.as_str(), tc = tc, th = s.thermal.as_str(),
        ),
        &mut f,
    );

    let cores = sparkline(&s.per_core.iter().map(|&v| v as f64).collect::<Vec<_>>(), 0.0, 1.0);
    line(
        format!(
            " {d}CPU{z}    P {pf:>4} {d}·{z} E {ef:>4} MHz   {d}busy{z} {busy:>3.0}%   {cores}",
            d = d, z = z, pf = s.pcpu_freq_mhz, ef = s.ecpu_freq_mhz,
            busy = s.cpu_usage_pct * 100.0, cores = cores,
        ),
        &mut f,
    );
    line(
        format!(
            " {d}GPU{z}    {gf:>4} MHz   {d}busy{z} {busy:>3.0}%",
            d = d, z = z, gf = s.gpu_freq_mhz, busy = s.gpu_active * 100.0,
        ),
        &mut f,
    );
    line(
        format!(
            " {d}PWR{z}    cpu {cpu:>4.1} {d}·{z} gpu {gpu:>4.1} {d}·{z} ane {ane:>4.1} {d}·{z} pkg {b}{all:>4.1}{z} {d}·{z} sys {sys:>4.1} W",
            d = d, z = z, b = b, cpu = s.cpu_power, gpu = s.gpu_power, ane = s.ane_power,
            all = s.all_power, sys = s.sys_power,
        ),
        &mut f,
    );

    let fan = if s.fan_max > 0 {
        format!("{rpm} {d}({min}–{max}){z}", rpm = s.fan_rpm, min = s.fan_min, max = s.fan_max, d = d, z = z)
    } else {
        format!("{d}n/a{z}", d = d, z = z)
    };
    line(
        format!(
            " {d}TMP{z}    cpu {ct:>2.0}°C {d}·{z} gpu {gt:>2.0}°C   {d}fan{z} {fan}",
            d = d, z = z, ct = s.cpu_temp, gt = s.gpu_temp, fan = fan,
        ),
        &mut f,
    );

    let ram_frac = if s.ram_total > 0 { s.ram_used as f64 / s.ram_total as f64 } else { 0.0 };
    line(
        format!(
            " {d}RAM{z}    {bar}  {used:.1} / {total:.1} GiB  {pct:.0}%",
            d = d, z = z, bar = bar(ram_frac, 0.0, 1.0, 18),
            used = gib(s.ram_used), total = gib(s.ram_total), pct = ram_frac * 100.0,
        ),
        &mut f,
    );

    line(
        format!(" {d}cpu%{z}   {y}{spark}{z}", d = d, z = z, y = st.green, spark = sparkline(cpu_hist, 0.0, 100.0)),
        &mut f,
    );
    let rpm_hi = (s.fan_max.max(1)) as f64;
    line(
        format!(" {d}rpm{z}    {y}{spark}{z}", d = d, z = z, y = st.yellow, spark = sparkline(rpm_hist, (s.fan_min as f64).min(rpm_hi), rpm_hi)),
        &mut f,
    );

    if !s.top_procs.is_empty() {
        let tops = s
            .top_procs
            .iter()
            .take(4)
            .map(|p| format!("{} {d}{:.0}%{z}", trunc(&p.name, 18), p.cpu, d = d, z = z))
            .collect::<Vec<_>>()
            .join("  ");
        line(format!(" {d}TOP{z}    {tops}", d = d, z = z), &mut f);
    }

    line(format!(" {d}{rule}{z}", d = d, z = z, rule = rule), &mut f);
    line(format!(" {d}q{z} quit {d}·{z} {d}every {ms}ms{z}", d = d, z = z, ms = SAMPLE_MS), &mut f);

    f.push_str(term::clear_eos());
    f
}

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n.saturating_sub(1)).collect::<String>() + "…"
    }
}

/// Padding so the clock sits near the right of the ~60-col header.
fn pad_to(chip: &str, p: u32, e: u32, g: u32) -> usize {
    let used = 7 + chip.len() + 3 + format!("{p}P+{e}E").len() + 3 + format!("{g} GPU").len() + 6;
    60usize.saturating_sub(used).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sensors::snapshot::ProcInfo;

    #[test]
    fn render_is_stable_and_complete() {
        let mut s = Snapshot::default();
        s.ts = "2026-06-03T15:52:21Z".into();
        s.chip = "Apple M4 Pro".into();
        s.p_cores = 8;
        s.e_cores = 4;
        s.gpu_cores = 16;
        s.per_core = vec![0.1, 0.2, 0.9, 0.5, 0.3, 0.0, 1.0, 0.4, 0.6, 0.6, 0.6, 0.6];
        s.pcpu_freq_mhz = 4512;
        s.ecpu_freq_mhz = 1991;
        s.cpu_usage_pct = 0.43;
        s.cpu_load_pct = 0.44;
        s.gpu_freq_mhz = 338;
        s.cpu_power = 13.5;
        s.all_power = 13.7;
        s.sys_power = 35.4;
        s.cpu_temp = 88.0;
        s.gpu_temp = 78.0;
        s.fan_rpm = 1763;
        s.fan_min = 1000;
        s.fan_max = 4900;
        s.ram_total = 48 << 30;
        s.ram_used = 41 << 30;
        s.thermal = Thermal::Fair;
        s.level = Level::Warn;
        s.top_procs = vec![ProcInfo { pid: 1, cpu: 6.0, name: "Virtua".into() }];

        let out = render(&s, &[10.0, 20.0, 30.0], &[1000.0, 1700.0]);
        // Frame starts at home, ends clearing to end of screen.
        assert!(out.starts_with("\x1b[H"));
        assert!(out.ends_with("\x1b[J"));
        // Key content present.
        assert!(out.contains("Apple M4 Pro"));
        assert!(out.contains("WARN"));
        assert!(out.contains("4512"));
        assert!(out.contains("88"));
        assert!(out.contains("Virtua"));
        // No panic on empty histories / zero cores.
        let empty = Snapshot::default();
        let _ = render(&empty, &[], &[]);
    }

    #[test]
    fn trunc_handles_unicode() {
        assert_eq!(trunc("abc", 5), "abc");
        assert_eq!(trunc("abcdef", 4), "abc…");
    }
}
