//! The Eldr live dashboard: a spacious, human-readable ANSI panel over
//! [`crate::ui::term`]. Plain-language status, performance/temperature/memory/power
//! sections, history sparklines and top apps. Adapts to the terminal width.
//! Keys: `q`/Ctrl-C quit, `space` pause, `+`/`-` speed, `?` help. The [`RawMode`]
//! guard restores the terminal, so quitting never leaks raw mode.

use crate::sensors::snapshot::{Level, Snapshot, Thermal};
use crate::ui::style::{Style, bar, gib, sparkline};
use crate::ui::term::{self, RawMode};
use std::io::Write;

const SAMPLE_MS: u64 = 250;
const HIST: usize = 48;
const MIN_INTERVAL: u64 = 250;
const MAX_INTERVAL: u64 = 5000;

/// Mutable view state the key handler drives.
struct Ui {
    interval_ms: u64,
    paused: bool,
    help: bool,
}

/// Run the dashboard until the user quits. `interval_ms` is the target refresh.
pub fn run(interval_ms: u64) {
    let Some(_raw) = RawMode::enter() else {
        eprintln!("eldr: not a terminal (tui needs a tty)");
        return;
    };

    let mut ui = Ui {
        interval_ms: interval_ms.clamp(MIN_INTERVAL, MAX_INTERVAL),
        paused: false,
        help: false,
    };
    let mut cpu_hist: Vec<f64> = Vec::with_capacity(HIST);
    let mut rpm_hist: Vec<f64> = Vec::with_capacity(HIST);
    let mut last: Option<Snapshot> = None;

    loop {
        if !ui.paused {
            let mut snap = Snapshot::gather(SAMPLE_MS);
            snap.source = "tui".into();
            let _ = snap.write_status();
            push_hist(&mut cpu_hist, (snap.cpu_load_pct * 100.0) as f64);
            push_hist(&mut rpm_hist, snap.fan_rpm as f64);
            last = Some(snap);
        }

        if let Some(snap) = &last {
            let frame = render(snap, &cpu_hist, &rpm_hist, &ui);
            let mut out = std::io::stdout();
            let _ = out.write_all(frame.as_bytes());
            let _ = out.flush();
        }

        // Paused: poll keys briefly so the UI stays responsive without resampling.
        let wait = if ui.paused {
            150
        } else {
            ui.interval_ms.saturating_sub(SAMPLE_MS).max(50) as i32
        };
        if let Some(k) = term::read_key(wait) {
            match k {
                b'q' | b'Q' | 3 => break,
                b' ' => ui.paused = !ui.paused,
                b'+' | b'=' => {
                    ui.interval_ms = ui.interval_ms.saturating_sub(250).max(MIN_INTERVAL)
                }
                b'-' | b'_' => ui.interval_ms = (ui.interval_ms + 250).min(MAX_INTERVAL),
                b'?' => ui.help = !ui.help,
                _ => {}
            }
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

/// A plain-language verdict + one explaining sentence, from the REAL signals
/// (thermal pressure and the fan) — never from absolute die temperature.
fn human_status(s: &Snapshot) -> (&'static str, &'static str) {
    if s.fan_max > 0 && s.fan_rpm < 500 {
        return (
            "Check the fan",
            "The fan reads stopped — that's worth a look.",
        );
    }
    match s.thermal {
        Thermal::Critical | Thermal::Serious => {
            ("Running hot", "macOS is easing off to cool things down.")
        }
        Thermal::Fair => (
            "Warming up",
            "A little thermal pressure, but handling it fine.",
        ),
        Thermal::Nominal => ("All good", "Cool and fast. Nothing is throttling."),
        Thermal::Unknown => ("Running", "Live readings below."),
    }
}

/// Frequency in human units: `3.7 GHz`, or `338 MHz` below 1 GHz.
fn ghz(mhz: u32) -> String {
    if mhz >= 1000 {
        format!("{:.1} GHz", mhz as f64 / 1000.0)
    } else {
        format!("{mhz} MHz")
    }
}

/// Friendly process name: strip a path and the `com.apple.` reverse-DNS prefix.
fn clean_proc(name: &str) -> String {
    let base = name.rsplit('/').next().unwrap_or(name);
    let short = base
        .strip_prefix("com.apple.")
        .and_then(|r| r.split('.').next())
        .unwrap_or(base);
    trunc(short, 18)
}

/// Short uptime like `3d 4h`, `4h 12m`, `7m`.
fn fmt_uptime(s: u64) -> String {
    let (d, h, m) = (s / 86400, (s % 86400) / 3600, (s % 3600) / 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

fn render(s: &Snapshot, cpu_hist: &[f64], rpm_hist: &[f64], ui: &Ui) -> String {
    let st = Style::color(); // we are in a tty (raw mode)
    let d = st.dim;
    let z = st.reset;
    let b = st.bold;
    let (cols, _rows) = term::size();
    let w = (cols as usize).clamp(46, 92);
    let barw = ((w as f64 * 0.30) as usize).clamp(14, 28);
    let rule = "─".repeat(w.saturating_sub(2));

    let mut f = String::with_capacity(2560);
    f.push_str(term::home());

    let line = |row: String, out: &mut String| {
        out.push_str(&row);
        out.push_str(term::clear_eol());
        out.push('\n');
    };
    let blank = |out: &mut String| {
        out.push_str(term::clear_eol());
        out.push('\n');
    };
    // Right-align `right` after `left` within width `w` (both measured plain).
    let pad =
        |left: usize, right: usize| -> String { " ".repeat(w.saturating_sub(left + right).max(1)) };

    // Header: eldr (left) · chip (right).
    let clock = if s.ts.len() >= 19 { &s.ts[11..19] } else { "" };
    line(
        format!(
            " {b}eldr{z}{sp}{d}{chip}{z}",
            chip = s.chip,
            sp = pad(1 + 4, s.chip.chars().count()),
        ),
        &mut f,
    );
    line(format!(" {d}{rule}{z}"), &mut f);
    blank(&mut f);

    // Plain-language status: a coloured dot, a verdict, one sentence, uptime.
    let (head, sub) = human_status(s);
    let dot = level_color(&st, s.level);
    line(format!(" {dot}●{z}  {b}{head}{z}"), &mut f);
    let up = format!("up {}", fmt_uptime(s.uptime_secs));
    line(
        format!(
            " {d}   {sub}{sp}{up}{z}",
            sp = pad(4 + sub.chars().count(), up.chars().count()),
        ),
        &mut f,
    );
    blank(&mut f);

    // Performance.
    line(format!(" {d}Performance{z}"), &mut f);
    line(
        format!(
            "    Processor   {pf:<8} {barp}  {busy:>3.0}%  {d}· E {ef}{z}",
            pf = ghz(s.pcpu_freq_mhz),
            ef = ghz(s.ecpu_freq_mhz),
            barp = bar(s.cpu_usage_pct as f64, 0.0, 1.0, barw),
            busy = s.cpu_usage_pct * 100.0,
        ),
        &mut f,
    );
    line(
        format!(
            "    Graphics    {gf:<8} {barg}  {busy:>3.0}%",
            gf = ghz(s.gpu_freq_mhz),
            barg = bar(s.gpu_active as f64, 0.0, 1.0, barw),
            busy = s.gpu_active * 100.0,
        ),
        &mut f,
    );
    blank(&mut f);

    // Temperature — informative, never an alarm on its own.
    line(format!(" {d}Temperature{z}"), &mut f);
    line(
        format!(
            "    Chip        {ct:>2.0}°C {d}· GPU {gt:>2.0}°C   ~90° is normal on Apple Silicon{z}",
            ct = s.cpu_temp,
            gt = s.gpu_temp,
        ),
        &mut f,
    );
    let rpm_hi = (s.fan_max.max(1)) as f64;
    let fan_spark = if s.fan_max > 0 {
        sparkline(rpm_hist, (s.fan_min as f64).min(rpm_hi), rpm_hi)
    } else {
        String::new()
    };
    let fan_stopped = s.fan_max > 0 && s.fan_rpm < 500;
    let fan_c = if fan_stopped { st.red } else { z };
    if s.fan_max > 0 {
        line(
            format!(
                "    Fans        {fan_c}{rpm} rpm{z}  {y}{fan_spark}{z}  {d}· range {min}–{max}{z}",
                rpm = s.fan_rpm,
                y = st.yellow,
                min = s.fan_min,
                max = s.fan_max,
            ),
            &mut f,
        );
    }
    blank(&mut f);

    // Memory + Power.
    let ram_frac = if s.ram_total > 0 {
        s.ram_used as f64 / s.ram_total as f64
    } else {
        0.0
    };
    let ram_c = if ram_frac >= 0.90 { st.yellow } else { z };
    line(
        format!(
            " {d}Memory{z}      {used:.1} of {total:.0} GB   {ram_c}{barm}{z}  {pct:>3.0}%",
            used = gib(s.ram_used),
            total = gib(s.ram_total),
            barm = bar(ram_frac, 0.0, 1.0, barw),
            pct = ram_frac * 100.0,
        ),
        &mut f,
    );
    line(
        format!(
            " {d}Power{z}       {all:.0} W chip {d}·{z} {sys:.0} W whole machine",
            all = s.all_power,
            sys = s.sys_power,
        ),
        &mut f,
    );
    blank(&mut f);

    // Load history + busiest apps.
    line(
        format!(
            " {d}Load{z}        {g}{spark}{z}  {cur:>3.0}%   {d}recent history{z}",
            g = st.green,
            spark = sparkline(cpu_hist, 0.0, 100.0),
            cur = s.cpu_load_pct * 100.0,
        ),
        &mut f,
    );
    if !s.top_procs.is_empty() {
        let tops = s
            .top_procs
            .iter()
            .take(4)
            .map(|p| format!("{} {d}{:.0}%{z}", clean_proc(&p.name), p.cpu))
            .collect::<Vec<_>>()
            .join(&format!(" {d}·{z} "));
        line(format!(" {d}Busiest{z}     {tops}"), &mut f);
    }
    blank(&mut f);

    line(format!(" {d}{rule}{z}"), &mut f);
    if ui.help {
        line(
            format!(" {d}note   ~90°C is normal here. The real signal is thermal pressure{z}"),
            &mut f,
        );
        line(
            format!(" {d}       (nominal→fair→serious→critical) and a live fan.{z}"),
            &mut f,
        );
        line(
            format!(
                " {b}q{z} Quit {d}·{z} {b}space{z} Pause {d}·{z} {b}+ −{z} Speed {d}·{z} {b}?{z} Close"
            ),
            &mut f,
        );
    } else {
        let paused = if ui.paused {
            format!("  {st_y}[paused]{z}", st_y = st.yellow)
        } else {
            String::new()
        };
        line(
            format!(
                " {d}q{z} Quit {d}·{z} {d}space{z} Pause {d}·{z} {d}+ −{z} Speed {d}·{z} {d}?{z} Help {d}· every {secs:.2}s{z}{paused}{clk}",
                secs = ui.interval_ms as f64 / 1000.0,
                clk = if clock.is_empty() {
                    String::new()
                } else {
                    format!("{d}   {clock}{z}")
                },
            ),
            &mut f,
        );
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sensors::snapshot::ProcInfo;

    fn ui() -> Ui {
        Ui {
            interval_ms: 250,
            paused: false,
            help: false,
        }
    }

    #[test]
    fn render_is_stable_and_human() {
        let mut s = Snapshot::default();
        s.ts = "2026-06-03T15:52:21Z".into();
        s.chip = "Apple M4 Pro".into();
        s.p_cores = 8;
        s.e_cores = 4;
        s.gpu_cores = 16;
        s.per_core = vec![0.1, 0.2, 0.9, 0.5];
        s.pcpu_freq_mhz = 4512;
        s.ecpu_freq_mhz = 1991;
        s.cpu_usage_pct = 0.43;
        s.cpu_load_pct = 0.44;
        s.gpu_freq_mhz = 338;
        s.all_power = 30.0;
        s.sys_power = 59.0;
        s.cpu_temp = 91.0;
        s.gpu_temp = 76.0;
        s.fan_rpm = 3150;
        s.fan_min = 1000;
        s.fan_max = 4900;
        s.ram_total = 48 << 30;
        s.ram_used = 39 << 30;
        s.uptime_secs = 3 * 86400 + 4 * 3600;
        s.thermal = Thermal::Fair;
        s.level = Level::Warn;
        s.top_procs = vec![ProcInfo {
            pid: 1,
            cpu: 6.0,
            name: "com.apple.Virtualization.VM".into(),
        }];

        let out = render(&s, &[10.0, 20.0, 30.0], &[1000.0, 1700.0], &ui());
        assert!(out.starts_with("\x1b[H"));
        assert!(out.ends_with("\x1b[J"));
        assert!(out.contains("Apple M4 Pro"));
        // Plain-language verdict (Fair -> "Warming up"), not a raw level token.
        assert!(out.contains("Warming up"));
        // Human units.
        assert!(out.contains("4.5 GHz"));
        assert!(out.contains("of 48 GB"));
        assert!(out.contains("up 3d 4h"));
        // Reverse-DNS prefix stripped.
        assert!(out.contains("Virtualization"));
        assert!(!out.contains("com.apple.Virtualization"));
        // Help toggles the pressure note.
        let mut h = ui();
        h.help = true;
        let out_h = render(&s, &[], &[], &h);
        assert!(out_h.contains("real signal is thermal pressure"));
        // No panic on empty data.
        let _ = render(&Snapshot::default(), &[], &[], &ui());
    }

    #[test]
    fn human_status_reads_pressure_not_temp() {
        let mut s = Snapshot::default();
        s.fan_max = 4900;
        s.fan_rpm = 1800;
        // High die temp alone stays "All good".
        s.cpu_temp = 99.0;
        s.thermal = Thermal::Nominal;
        assert_eq!(human_status(&s).0, "All good");
        s.thermal = Thermal::Fair;
        assert_eq!(human_status(&s).0, "Warming up");
        // A stopped fan is the real danger.
        s.fan_rpm = 0;
        assert_eq!(human_status(&s).0, "Check the fan");
    }

    #[test]
    fn ghz_and_clean_proc() {
        assert_eq!(ghz(4512), "4.5 GHz");
        assert_eq!(ghz(338), "338 MHz");
        assert_eq!(clean_proc("com.apple.Virtualization.VM"), "Virtualization");
        assert_eq!(clean_proc("/usr/bin/stress-ng"), "stress-ng");
    }

    #[test]
    fn trunc_handles_unicode() {
        assert_eq!(trunc("abc", 5), "abc");
        assert_eq!(trunc("abcdef", 4), "abc…");
    }
}
