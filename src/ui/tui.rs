//! The Eldr live dashboard: a tabbed, human-readable ANSI panel over
//! [`crate::ui::term`]. Five views (Overview · CPU · Memory · Energy · Storage) sharing
//! an identical header/footer; only the body swaps, like a segmented control. Plain
//! language, honest memory (used vs available vs pressure), real storage. Sampling runs
//! off-thread so keys and quit are instant. Keys: `q`/Ctrl-C quit, `←/→` or `Tab` or
//! `1`-`5` switch view, `space` pause, `+`/`-` speed, `?` help.

use crate::sensors::snapshot::{Level, Snapshot, Thermal};
use crate::sensors::system::SystemInfo;
use crate::ui::style::{Style, bar_c, sparkline};
use crate::ui::term::{self, RawMode};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

const SAMPLE_MS: u64 = 250;
const HIST: usize = 48;
const MIN_INTERVAL: u64 = 250;
const MAX_INTERVAL: u64 = 5000;
const NTABS: u8 = 5;
const TABS: [&str; 5] = ["Overview", "CPU", "Memory", "Energy", "Storage"];

/// Mutable view state the key handler drives.
struct Ui {
    interval_ms: u64,
    paused: bool,
    help: bool,
    tab: u8,
}

/// Static identity shown in the header / Storage tab (read once at startup).
#[derive(Default, Clone)]
struct Ident {
    label: String, // "Mac mini · Apple M4 Pro · 8P+4E · 16 GPU · 48 GB"
    ssd_model: String,
    ssd_bytes: u64,
    ssd_medium: String,
}

fn loading_frame() -> String {
    let st = Style::color();
    format!(
        "{home} {b}eldr{z}  {d}reading sensors…{z}{eol}\n{eos}",
        home = term::home(),
        b = st.bold,
        z = st.reset,
        d = st.dim,
        eol = term::clear_eol(),
        eos = term::clear_eos(),
    )
}

/// Run the dashboard until the user quits.
pub fn run(interval_ms: u64) {
    let Some(_raw) = RawMode::enter() else {
        eprintln!("eldr: not a terminal (tui needs a tty)");
        return;
    };

    // Static identity (model + SSD) read once — IOKit is too slow for every frame.
    let ident = build_ident();

    let running = Arc::new(AtomicBool::new(true));
    let paused = Arc::new(AtomicBool::new(false));
    let interval = Arc::new(AtomicU64::new(
        interval_ms.clamp(MIN_INTERVAL, MAX_INTERVAL),
    ));
    let (tx, rx) = mpsc::channel::<Snapshot>();

    {
        let running = running.clone();
        let paused = paused.clone();
        let interval = interval.clone();
        std::thread::spawn(move || {
            while running.load(Ordering::Relaxed) {
                if paused.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
                let mut snap = Snapshot::gather(SAMPLE_MS);
                snap.source = "tui".into();
                let _ = snap.write_status();
                if tx.send(snap).is_err() {
                    break;
                }
                let extra = interval.load(Ordering::Relaxed).saturating_sub(SAMPLE_MS);
                let mut slept = 0u64;
                while slept < extra && running.load(Ordering::Relaxed) {
                    let chunk = (extra - slept).min(100);
                    std::thread::sleep(Duration::from_millis(chunk));
                    slept += chunk;
                }
            }
        });
    }

    let mut ui = Ui {
        interval_ms: interval.load(Ordering::Relaxed),
        paused: false,
        help: false,
        tab: 0,
    };
    let mut cpu_hist: Vec<f64> = Vec::with_capacity(HIST);
    let mut rpm_hist: Vec<f64> = Vec::with_capacity(HIST);
    let mut pwr_hist: Vec<f64> = Vec::with_capacity(HIST);
    let mut last: Option<Snapshot> = None;

    let draw = |snap: &Snapshot, c: &[f64], r: &[f64], p: &[f64], ui: &Ui, id: &Ident| {
        let mut out = std::io::stdout();
        let _ = out.write_all(render(snap, c, r, p, ui, id).as_bytes());
        let _ = out.flush();
    };

    {
        let mut out = std::io::stdout();
        let _ = out.write_all(loading_frame().as_bytes());
        let _ = out.flush();
    }

    loop {
        let mut dirty = false;
        while let Ok(snap) = rx.try_recv() {
            push_hist(&mut cpu_hist, (snap.cpu_load_pct * 100.0) as f64);
            push_hist(&mut rpm_hist, snap.fan_rpm as f64);
            push_hist(&mut pwr_hist, snap.sys_power as f64);
            last = Some(snap);
            dirty = true;
        }

        if let Some(k) = term::read_key(80) {
            match k {
                b'q' | b'Q' | 3 => break,
                b' ' => {
                    ui.paused = !ui.paused;
                    paused.store(ui.paused, Ordering::Relaxed);
                    dirty = true;
                }
                b'+' | b'=' => {
                    ui.interval_ms = ui.interval_ms.saturating_sub(250).max(MIN_INTERVAL);
                    interval.store(ui.interval_ms, Ordering::Relaxed);
                    dirty = true;
                }
                b'-' | b'_' => {
                    ui.interval_ms = (ui.interval_ms + 250).min(MAX_INTERVAL);
                    interval.store(ui.interval_ms, Ordering::Relaxed);
                    dirty = true;
                }
                b'?' => {
                    ui.help = !ui.help;
                    dirty = true;
                }
                b'1'..=b'5' => {
                    ui.tab = k - b'1';
                    dirty = true;
                }
                b'\t' => {
                    ui.tab = (ui.tab + 1) % NTABS;
                    dirty = true;
                }
                0x1b => {
                    // Arrow keys arrive as ESC [ C (right) / ESC [ D (left).
                    let bracket = term::read_key(8);
                    let dir = term::read_key(8);
                    if bracket == Some(b'[') {
                        match dir {
                            Some(b'C') => ui.tab = (ui.tab + 1) % NTABS,
                            Some(b'D') => ui.tab = (ui.tab + NTABS - 1) % NTABS,
                            _ => {}
                        }
                        dirty = true;
                    }
                }
                _ => {}
            }
        }

        if dirty && let Some(snap) = &last {
            draw(snap, &cpu_hist, &rpm_hist, &pwr_hist, &ui, &ident);
        }
    }

    running.store(false, Ordering::Relaxed);
}

fn build_ident() -> Ident {
    let s = SystemInfo::get();
    let mut parts = Vec::new();
    if !s.marketing.is_empty() {
        parts.push(s.marketing.clone());
    }
    if !s.chip.is_empty() {
        parts.push(s.chip.clone());
    }
    if s.e_cores > 0 {
        parts.push(format!("{}P+{}E", s.p_cores, s.e_cores));
    }
    if s.ram_bytes > 0 {
        parts.push(format!("{:.0} GB", gib(s.ram_bytes)));
    }
    Ident {
        label: parts.join(" · "),
        ssd_model: s.ssd_model,
        ssd_bytes: s.ssd_bytes,
        ssd_medium: s.ssd_medium,
    }
}

fn push_hist(h: &mut Vec<f64>, v: f64) {
    h.push(v);
    if h.len() > HIST {
        h.remove(0);
    }
}

// MARK: formatting helpers

fn gib(b: u64) -> f64 {
    b as f64 / 1_073_741_824.0
}
fn gb_dec(b: u64) -> u64 {
    b / 1_000_000_000
}
fn ghz(mhz: u32) -> String {
    if mhz >= 1000 {
        format!("{:.1} GHz", mhz as f64 / 1000.0)
    } else {
        format!("{mhz} MHz")
    }
}
fn human_mem(b: u64) -> String {
    let g = gib(b);
    if g >= 1.0 {
        format!("{g:.1} GB")
    } else {
        format!("{:.0} MB", b as f64 / 1_048_576.0)
    }
}
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
fn clean_proc(name: &str) -> String {
    let base = name.rsplit('/').next().unwrap_or(name);
    let short = base
        .strip_prefix("com.apple.")
        .and_then(|r| r.split('.').next())
        .unwrap_or(base);
    trunc(short, 18)
}
fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n.saturating_sub(1)).collect::<String>() + "…"
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
fn pressure_color(st: &Style, p: &str) -> &'static str {
    match p {
        "low" => st.green,
        "medium" => st.yellow,
        "high" => st.red,
        _ => st.dim,
    }
}
fn human_status(s: &Snapshot) -> (&'static str, &'static str) {
    if s.fan_failed() {
        return (
            "Check the fan",
            "Cooling is calling for the fan, but it reads stopped.",
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

/// Three-zone gauge: used (▓) · cached/reclaimable (▒) · free (░).
fn seg_bar(used: u64, cached: u64, total: u64, width: usize, st: &Style) -> String {
    if total == 0 {
        return "░".repeat(width);
    }
    let cell = |b: u64| ((b as f64 / total as f64) * width as f64).round() as usize;
    let u = cell(used).min(width);
    let c = cell(cached).min(width - u);
    let fr = width - u - c;
    format!(
        "{}{}{}{}{}{}",
        st.reset,
        "▓".repeat(u),
        st.dim,
        "▒".repeat(c),
        "░".repeat(fr),
        st.reset,
    )
}

fn storage_color(st: &Style, free: u64, total: u64) -> (&'static str, &'static str) {
    let gb = gb_dec(free);
    let pct_free = if total > 0 {
        free as f64 / total as f64
    } else {
        1.0
    };
    // macOS cares about absolute free GB; the % keeps "plenty" honest on big disks.
    if gb < 10 || pct_free < 0.03 {
        (st.red, "almost full")
    } else if gb < 25 || pct_free < 0.10 {
        (st.yellow, "getting full")
    } else {
        (st.green, "plenty free")
    }
}

// MARK: render

fn render(
    s: &Snapshot,
    cpu_hist: &[f64],
    rpm_hist: &[f64],
    pwr_hist: &[f64],
    ui: &Ui,
    id: &Ident,
) -> String {
    let st = Style::color();
    let (cols, _rows) = term::size();
    let w = (cols as usize).clamp(56, 100);
    let barw = ((w as f64 * 0.34) as usize).clamp(16, 34);
    let d = st.dim;
    let z = st.reset;
    let b = st.bold;

    let mut f = String::with_capacity(4096);
    f.push_str(term::home());
    let rule = "─".repeat(w.saturating_sub(2));

    let line = |row: String, out: &mut String| {
        out.push_str(&row);
        out.push_str(term::clear_eol());
        out.push('\n');
    };
    let blank = |out: &mut String| {
        out.push_str(term::clear_eol());
        out.push('\n');
    };
    let pad = |left: usize, right: usize| " ".repeat(w.saturating_sub(left + right).max(1));

    // ---- header (identical on every tab) ----
    let (head, sub) = human_status(s);
    let lc = level_color(&st, s.level);
    let ident = if id.label.is_empty() {
        s.chip.clone()
    } else {
        id.label.clone()
    };
    let right = format!("{head}  ·  up {}", fmt_uptime(s.uptime_secs));
    line(
        format!(
            " {b}eldr{z}  {d}{ident}{z}{sp}{lc}●{z} {b}{head}{z} {d}· up {up}{z}",
            sp = pad(7 + ident.chars().count(), right.chars().count() + 2),
            up = fmt_uptime(s.uptime_secs),
        ),
        &mut f,
    );
    line(format!(" {d}{rule}{z}"), &mut f);

    // tab strip
    let mut strip = String::from(" ");
    for (i, name) in TABS.iter().enumerate() {
        if i as u8 == ui.tab {
            strip.push_str(&format!("{b}[ {name} ]{z}  "));
        } else {
            strip.push_str(&format!("{d}{name}{z}   "));
        }
    }
    let speed = format!("every {:.2}s", ui.interval_ms as f64 / 1000.0);
    let strip_len = 1 + TABS
        .iter()
        .enumerate()
        .map(|(i, n)| n.chars().count() + if i as u8 == ui.tab { 6 } else { 3 })
        .sum::<usize>();
    strip.push_str(&format!(
        "{sp}{d}{speed}{z}",
        sp = pad(strip_len, speed.chars().count())
    ));
    line(strip, &mut f);
    blank(&mut f);

    // ---- body ----
    match ui.tab {
        0 => body_overview(s, cpu_hist, id, &st, w, barw, &line, &mut f),
        1 => body_cpu(s, cpu_hist, &st, barw, &line, &blank, &mut f),
        2 => body_memory(s, &st, w, barw, &line, &blank, &mut f),
        3 => body_energy(s, pwr_hist, rpm_hist, &st, barw, &line, &blank, &mut f),
        _ => body_storage(s, id, &st, barw, &line, &blank, &mut f),
    }

    // ---- footer (identical on every tab) ----
    while f.matches('\n').count() < 20 {
        blank(&mut f);
    }
    line(format!(" {d}{rule}{z}"), &mut f);
    if ui.help {
        line(
            format!(" {d}~90°C is normal on Apple Silicon. The real heat signal is thermal{z}"),
            &mut f,
        );
        line(
            format!(
                " {d}pressure (nominal→fair→serious→critical) + a live fan, not the number.{z}"
            ),
            &mut f,
        );
    } else {
        let paused = if ui.paused {
            format!("  {y}[paused]{z}", y = st.yellow)
        } else {
            String::new()
        };
        line(
            format!(
                " {d}q{z} Quit {d}·{z} {d}←→/Tab{z} Views {d}·{z} {d}1-5{z} Jump {d}·{z} {d}space{z} Pause {d}·{z} {d}+−{z} Speed {d}·{z} {d}?{z} Help{paused}"
            ),
            &mut f,
        );
    }

    let _ = (sub, b);
    f.push_str(term::clear_eos());
    f
}

type LineFn<'a> = dyn Fn(String, &mut String) + 'a;

#[allow(clippy::too_many_arguments)]
fn body_overview(
    s: &Snapshot,
    cpu_hist: &[f64],
    id: &Ident,
    st: &Style,
    _w: usize,
    barw: usize,
    line: &LineFn,
    f: &mut String,
) {
    let d = st.dim;
    let z = st.reset;
    let (_h, sub) = human_status(s);
    line(
        format!(" {sub}   {d}~90°C is normal here, not a problem.{z}"),
        f,
    );
    line(String::new(), f);

    // One grid: every row is label · bar · reading, columns aligned. The bar carries
    // each signal's health in colour — fire for activity, green/amber/red for state.
    // CPU — fire (activity), bar = load.
    line(
        format!(
            " {d}CPU{z}      {bar}  {busy:>3.0}%   {d}{p} / {e} · load {la:.0}{z}",
            bar = bar_c(s.cpu_load_pct as f64, 0.0, 1.0, barw, st.fire, st),
            busy = s.cpu_load_pct * 100.0,
            p = ghz(s.pcpu_freq_mhz),
            e = ghz(s.ecpu_freq_mhz),
            la = s.load_avg.0,
        ),
        f,
    );
    // Memory — colour follows pressure, bar = used fraction.
    let press = s.mem_pressure();
    let pc = pressure_color(st, press);
    line(
        format!(
            " {d}Memory{z}   {bar}  {used:.0}{d}/{z}{tot:.0} GB {d}·{z} {pc}{press}{z}",
            bar = bar_c(s.ram_used as f64, 0.0, s.ram_total.max(1) as f64, barw, pc, st),
            used = gib(s.ram_used),
            tot = gib(s.ram_total),
        ),
        f,
    );
    // Heat — now a real grid row. Colour follows the thermal STATE (90° at nominal is
    // healthy), bar = chip temp over its operating range.
    let tc = thermal_color(st, s.thermal);
    line(
        format!(
            " {d}Heat{z}     {bar}  {ct:.0}°{d} chip · {gt:.0}° gpu ·{z} {tc}{th}{z}",
            bar = bar_c(s.cpu_temp as f64, 30.0, 105.0, barw, tc, st),
            ct = s.cpu_temp,
            gt = s.gpu_temp,
            th = s.thermal.as_str(),
        ),
        f,
    );
    // Storage — colour follows free-space health, bar = used fraction.
    if let Some(disk) = &s.disk {
        let (sc, word) = storage_color(st, disk.free, disk.total);
        let used = disk.total.saturating_sub(disk.free);
        line(
            format!(
                " {d}Storage{z}  {bar}  {used}{d}/{z}{tot} GB {d}·{z} {sc}{word}{z}",
                bar = bar_c(used as f64, 0.0, disk.total.max(1) as f64, barw, sc, st),
                used = gb_dec(used),
                tot = gb_dec(disk.total),
            ),
            f,
        );
    }
    let _ = (id, cpu_hist);
    line(String::new(), f);
    // Busiest
    line(format!(" {d}Busiest CPU{z}   {}", procs_cpu(s, st)), f);
    line(format!(" {d}Busiest RAM{z}   {}", procs_mem(s, st)), f);
}

#[allow(clippy::too_many_arguments)]
fn body_cpu(
    s: &Snapshot,
    cpu_hist: &[f64],
    st: &Style,
    barw: usize,
    line: &LineFn,
    blank: &dyn Fn(&mut String),
    f: &mut String,
) {
    let d = st.dim;
    let z = st.reset;
    line(
        format!(
            " {d}Total load{z}   {bar}  {busy:>3.0}%   {d}load avg {a:.2} · {b5:.2} · {c:.2}{z}",
            bar = bar_c(s.cpu_load_pct as f64, 0.0, 1.0, barw, st.fire, st),
            busy = s.cpu_load_pct * 100.0,
            a = s.load_avg.0,
            b5 = s.load_avg.1,
            c = s.load_avg.2,
        ),
        f,
    );
    let (mn, mx) = min_max(cpu_hist);
    line(
        format!(
            " {d}History{z}      {g}{spk}{z}   {d}now {now:.0}% · busiest {mx:.0}% · quietest {mn:.0}%{z}",
            g = st.green,
            spk = sparkline(cpu_hist, 0.0, 100.0),
            now = s.cpu_load_pct * 100.0,
        ),
        f,
    );
    blank(f);
    let p = s.p_cores as usize;
    line(
        format!(
            " {d}Performance cores{z}  {d}{pf} · {act:.0}% of max{z}",
            pf = ghz(s.pcpu_freq_mhz),
            act = s.pcpu_active * 100.0,
        ),
        f,
    );
    for (i, v) in s.per_core.iter().take(p).enumerate() {
        line(
            format!(
                "   P{n:<2} {bar} {pct:>3.0}%",
                n = i + 1,
                bar = bar_c(*v as f64, 0.0, 1.0, barw, st.fire, st),
                pct = v * 100.0,
            ),
            f,
        );
    }
    blank(f);
    line(
        format!(
            " {d}Efficiency cores{z}   {d}{ef} · {act:.0}% of max{z}",
            ef = ghz(s.ecpu_freq_mhz),
            act = s.ecpu_active * 100.0,
        ),
        f,
    );
    for (i, v) in s.per_core.iter().skip(p).enumerate() {
        line(
            format!(
                "   E{n:<2} {bar} {pct:>3.0}%",
                n = i + 1,
                bar = bar_c(*v as f64, 0.0, 1.0, barw, st.fire, st),
                pct = v * 100.0,
            ),
            f,
        );
    }
}

#[allow(clippy::too_many_arguments)]
/// Plain-language reason for the current memory pressure: how much is still
/// reclaimable and whether anything has spilled to swap. Turns a mute "● medium" into
/// something that says *why* — the biggest holder is printed on the line below this.
fn why_pressure(s: &Snapshot) -> String {
    let avail = gib(s.ram_available);
    match s.mem_pressure() {
        "low" => format!("{avail:.1} GB reclaimable — use it freely"),
        "medium" if s.swap_used > 0 => format!(
            "{avail:.1} GB still reclaimable; {:.1} GB spilled to swap earlier",
            gib(s.swap_used)
        ),
        "medium" => format!("{avail:.1} GB still reclaimable, nothing swapped"),
        "high" => format!("only {avail:.1} GB left to reclaim — macOS will swap to cope"),
        _ => "memory state unknown".to_string(),
    }
}

fn body_memory(
    s: &Snapshot,
    st: &Style,
    w: usize,
    _barw: usize,
    line: &LineFn,
    blank: &dyn Fn(&mut String),
    f: &mut String,
) {
    let d = st.dim;
    let z = st.reset;
    let press = s.mem_pressure();
    let pc = pressure_color(st, press);
    let app = s.ram_used.saturating_sub(s.ram_wired + s.ram_compressed);
    let free = s.ram_available.saturating_sub(s.ram_cached);
    let avail_pct = if s.ram_total > 0 {
        s.ram_available as f64 / s.ram_total as f64 * 100.0
    } else {
        0.0
    };

    line(
        format!(
            " {d}Total{z} {tot:.0} GB   {d}·{z} {d}In use{z} {b}{used:.1} GB{z}   {d}·{z} {d}Free{z} {b}{avail:.1} GB{z} {d}available ({pct:.0}%){z}",
            b = st.bold,
            tot = gib(s.ram_total),
            used = gib(s.ram_used),
            avail = gib(s.ram_available),
            pct = avail_pct,
        ),
        f,
    );
    blank(f);
    let segw = w.saturating_sub(8).min(70);
    line(
        format!(
            " {}  {d}of {tot:.0} GB{z}",
            seg_bar(s.ram_used, s.ram_cached, s.ram_total, segw, st),
            tot = gib(s.ram_total),
        ),
        f,
    );
    line(
        format!(
            " {d}▓ in use {used:.1}{z}   {d}▒ cached {cac:.1}{z}   {d}░ free {fr:.1}  (GB){z}",
            used = gib(s.ram_used),
            cac = gib(s.ram_cached),
            fr = gib(free),
        ),
        f,
    );
    blank(f);
    line(
        format!(
            " {d}Pressure{z}   {pc}● {press}{z}   {d}{}{z}",
            pressure_words(press)
        ),
        f,
    );
    line(format!("   {d}why → {}{z}", why_pressure(s)), f);
    if let Some(p) = s.top_mem.first() {
        line(
            format!(
                "   {d}      biggest holder: {} at {:.1} GB{z}",
                clean_proc(&p.name),
                gib(p.mem),
            ),
            f,
        );
    }
    blank(f);
    line(format!(" {d}What the memory holds{z}"), f);
    let bw = 14;
    // macOS packs more data into the compressor than the physical bytes it occupies;
    // surfacing the ratio explains why the machine fits more than its RAM size suggests.
    let packed = if s.ram_compressed > 0 && s.ram_compressed_holds > s.ram_compressed {
        format!(
            "  {d}holds {h:.1} GB ({r:.1}× packed){z}",
            h = gib(s.ram_compressed_holds),
            r = s.ram_compressed_holds as f64 / s.ram_compressed as f64,
        )
    } else {
        String::new()
    };
    for (label, val, note) in [
        ("App memory", app, ""),
        ("Wired", s.ram_wired, ""),
        ("Compressed", s.ram_compressed, packed.as_str()),
        ("Cached files", s.ram_cached, ""),
    ] {
        line(
            format!(
                "   {label:<13} {val:>6.1} GB  {bar}{note}",
                val = gib(val),
                bar = bar_c(val as f64, 0.0, s.ram_total.max(1) as f64, bw, st.fire, st),
            ),
            f,
        );
    }
    line(
        format!("   {d}cached files are reusable — they count as free{z}"),
        f,
    );
    blank(f);
    let (su, st_) = (s.swap_used, s.swap_total);
    let swap_note = if su == 0 {
        "macOS hasn't needed to swap — good"
    } else {
        "parked on disk from earlier (clears on reboot)"
    };
    line(
        format!(
            " {d}Swap{z}   {used:.1} {d}of{z} {tot:.1} GB   {d}{note}{z}",
            used = gib(su),
            tot = gib(st_),
            note = swap_note,
        ),
        f,
    );
    blank(f);
    line(
        format!(" {d}Using most memory{z}   {}", procs_mem(s, st)),
        f,
    );
}

#[allow(clippy::too_many_arguments)]
fn body_energy(
    s: &Snapshot,
    pwr_hist: &[f64],
    rpm_hist: &[f64],
    st: &Style,
    barw: usize,
    line: &LineFn,
    blank: &dyn Fn(&mut String),
    f: &mut String,
) {
    let d = st.dim;
    let z = st.reset;
    let (mn, mx) = min_max(pwr_hist);
    line(
        format!(
            " {d}Power{z}    Whole machine {b}{sys:.0} W{z}   {d}· chip package {all:.0} W{z}",
            b = st.bold,
            sys = s.sys_power,
            all = s.all_power,
        ),
        f,
    );
    line(
        format!(
            " {d}History{z}  {y}{spk}{z}   {d}now {now:.0} W · peak {mx:.0} · idle {mn:.0}{z}",
            y = st.yellow,
            spk = sparkline(pwr_hist, 0.0, mx.max(1.0)),
            now = s.sys_power,
        ),
        f,
    );
    blank(f);
    line(
        format!(" {d}Where the watts go{z}  {d}(chip package){z}"),
        f,
    );
    let cap = s.all_power.max(0.1) as f64;
    for (label, val) in [
        ("CPU", s.cpu_power),
        ("GPU", s.gpu_power),
        ("RAM", s.ram_power),
        ("ANE", s.ane_power),
    ] {
        line(
            format!(
                "   {label:<4} {val:>5.1} W  {bar}",
                val = val,
                bar = bar_c(val as f64, 0.0, cap, barw + 6, st.fire, st),
            ),
            f,
        );
    }
    blank(f);
    line(format!(" {d}Heat — the one signal that matters{z}"), f);
    let tc = thermal_color(st, s.thermal);
    line(
        format!(
            "   Thermal pressure  {tc}● {th}{z}   {d}nothing is being throttled{z}",
            th = s.thermal.as_str(),
        ),
        f,
    );
    line(
        format!(
            "   {d}Temperatures   Chip {ct:.0}° · GPU {gt:.0}°   (die runs hot by design){z}",
            ct = s.cpu_temp,
            gt = s.gpu_temp,
        ),
        f,
    );
    let fc = if s.fan_failed() { st.red } else { z };
    let pct = if s.fan_max > s.fan_min {
        (s.fan_rpm.saturating_sub(s.fan_min)) as f64 / (s.fan_max - s.fan_min) as f64 * 100.0
    } else {
        0.0
    };
    line(
        format!(
            "   Cooling   {fc}{rpm} rpm{z}  {y}{spk}{z}  {d}{pct:.0}% of range ({mn}–{mx}){z}",
            rpm = s.fan_rpm,
            y = st.yellow,
            spk = sparkline(
                rpm_hist,
                (s.fan_min as f64).min(s.fan_max as f64),
                s.fan_max.max(1) as f64
            ),
            mn = s.fan_min,
            mx = s.fan_max,
        ),
        f,
    );
}

#[allow(clippy::too_many_arguments)]
fn body_storage(
    s: &Snapshot,
    id: &Ident,
    st: &Style,
    barw: usize,
    line: &LineFn,
    blank: &dyn Fn(&mut String),
    f: &mut String,
) {
    let d = st.dim;
    let z = st.reset;
    let Some(disk) = &s.disk else {
        line(format!(" {d}disk info unavailable{z}"), f);
        return;
    };
    let used = disk.total.saturating_sub(disk.free);
    let pct = if disk.total > 0 {
        used as f64 / disk.total as f64 * 100.0
    } else {
        0.0
    };
    let (sc, word) = storage_color(st, disk.free, disk.total);
    line(format!(" {d}Startup disk “/”{z}"), f);
    line(
        format!(
            "   Used  {bar}  {used} {d}of{z} {tot} GB",
            bar = bar_c(used as f64, 0.0, disk.total.max(1) as f64, barw + 8, sc, st),
            used = gb_dec(used),
            tot = gb_dec(disk.total),
        ),
        f,
    );
    line(
        format!(
            "   Free  {free} GB {d}·{z} {pct:.0}% full   {sc}● {word}{z}",
            free = gb_dec(disk.free),
        ),
        f,
    );
    if gb_dec(disk.free) < 25 {
        line(
            format!("   {sc}clearing space keeps macOS snappy (it slows below ~10 GB free){z}"),
            f,
        );
    }
    blank(f);
    if !id.ssd_model.is_empty() {
        line(format!(" {d}The drive{z}    {}", id.ssd_model), f);
        let cap = if id.ssd_bytes > 0 {
            format!("{} GB", id.ssd_bytes / 1_000_000_000)
        } else {
            String::new()
        };
        let medium = if id.ssd_medium.is_empty() {
            String::new()
        } else {
            format!("   {d}{}{z}", id.ssd_medium)
        };
        line(
            format!(
                " {d}Capacity{z}     {cap}   {d}({usable} GB usable after formatting){z}{medium}",
                usable = gb_dec(disk.total),
            ),
            f,
        );
    }
}

// MARK: small shared bits

fn min_max(h: &[f64]) -> (f64, f64) {
    if h.is_empty() {
        return (0.0, 0.0);
    }
    let mn = h.iter().cloned().fold(f64::INFINITY, f64::min);
    let mx = h.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    (mn, mx)
}

fn pressure_words(p: &str) -> &'static str {
    match p {
        "low" => "the Mac has memory to spare",
        "medium" => "starting to compress — still ok",
        "high" => "little headroom; apps may slow",
        _ => "",
    }
}

fn procs_cpu(s: &Snapshot, st: &Style) -> String {
    let d = st.dim;
    let z = st.reset;
    s.top_procs
        .iter()
        .take(4)
        .map(|p| format!("{} {d}{:.0}%{z}", clean_proc(&p.name), p.cpu))
        .collect::<Vec<_>>()
        .join(&format!(" {d}·{z} "))
}
fn procs_mem(s: &Snapshot, st: &Style) -> String {
    let d = st.dim;
    let z = st.reset;
    s.top_mem
        .iter()
        .take(4)
        .map(|p| format!("{} {d}{}{z}", clean_proc(&p.name), human_mem(p.mem)))
        .collect::<Vec<_>>()
        .join(&format!(" {d}·{z} "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sensors::snapshot::{DiskInfo, ProcInfo};

    fn ui(tab: u8) -> Ui {
        Ui {
            interval_ms: 1000,
            paused: false,
            help: false,
            tab,
        }
    }
    fn ident() -> Ident {
        Ident {
            label: "Mac mini · Apple M4 Pro · 8P+4E · 48 GB".into(),
            ssd_model: "APPLE SSD AP0512Z".into(),
            ssd_bytes: 512_000_000_000,
            ssd_medium: "NVMe · Internal".into(),
        }
    }
    fn snap() -> Snapshot {
        let mut s = Snapshot::default();
        s.ts = "2026-06-03T15:52:21Z".into();
        s.chip = "Apple M4 Pro".into();
        s.p_cores = 8;
        s.e_cores = 4;
        s.per_core = vec![0.6, 0.3, 0.8, 0.2, 0.9, 0.4, 0.1, 0.5, 0.3, 0.2, 0.2, 0.4];
        s.pcpu_freq_mhz = 3700;
        s.ecpu_freq_mhz = 2600;
        s.cpu_load_pct = 0.34;
        s.gpu_freq_mhz = 338;
        s.cpu_power = 13.5;
        s.gpu_power = 0.1;
        s.ram_power = 0.4;
        s.all_power = 14.0;
        s.sys_power = 35.0;
        s.cpu_temp = 52.0;
        s.gpu_temp = 46.0;
        s.fan_rpm = 1840;
        s.fan_min = 1000;
        s.fan_max = 4900;
        s.ram_total = 48 << 30;
        s.ram_used = 30 << 30;
        s.ram_available = 14 << 30;
        s.ram_cached = 7 << 30;
        s.ram_wired = 7 << 30;
        s.ram_compressed = 3 << 30;
        s.ram_compressed_holds = 7 << 30;
        s.swap_used = 2 << 30;
        s.swap_total = 4 << 30;
        s.uptime_secs = 3 * 86400 + 4 * 3600;
        s.thermal = Thermal::Nominal;
        s.level = Level::Ok;
        s.disk = Some(DiskInfo {
            total: 460_000_000_000,
            free: 22_000_000_000,
        });
        s.top_procs = vec![ProcInfo {
            pid: 1,
            cpu: 6.0,
            mem: 0,
            name: "com.apple.Virtualization.VM".into(),
        }];
        s.top_mem = vec![ProcInfo {
            pid: 1,
            cpu: 0.0,
            mem: 32 << 30,
            name: "com.apple.Virtualization.VM".into(),
        }];
        s
    }

    #[test]
    fn every_tab_renders_without_panic() {
        let s = snap();
        let (c, r, p) = (vec![10.0, 40.0, 34.0], vec![1800.0], vec![20.0, 35.0]);
        for tab in 0..NTABS {
            let out = render(&s, &c, &r, &p, &ui(tab), &ident());
            assert!(out.starts_with("\x1b[H"));
            assert!(out.ends_with("\x1b[J"));
            assert!(out.contains("Apple M4 Pro"));
        }
    }

    #[test]
    fn memory_tab_is_unmistakable() {
        let out = render(&snap(), &[], &[], &[], &ui(2), &ident());
        assert!(out.contains("In use"));
        assert!(out.contains("available"));
        assert!(out.contains("Pressure"));
        assert!(out.contains("cached"));
    }

    #[test]
    fn memory_tab_explains_why() {
        let out = render(&snap(), &[], &[], &[], &ui(2), &ident());
        // The pressure now carries a reason and names the biggest holder.
        assert!(out.contains("why →"));
        assert!(out.contains("reclaimable"));
        assert!(out.contains("biggest holder"));
        // Compression ratio is surfaced (snap holds 7 GB in 3 GB physical → ~2.3×).
        assert!(out.contains("packed"));
    }

    #[test]
    fn storage_tab_shows_real_ssd() {
        let out = render(&snap(), &[], &[], &[], &ui(4), &ident());
        assert!(out.contains("APPLE SSD AP0512Z"));
        assert!(out.contains("22 GB"));
    }

    #[test]
    fn clean_proc_strips_prefix() {
        assert_eq!(clean_proc("com.apple.Virtualization.VM"), "Virtualization");
        assert_eq!(clean_proc("/usr/bin/stress-ng"), "stress-ng");
    }
}
