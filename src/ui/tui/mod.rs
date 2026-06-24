//! The Eldr live dashboard: a tabbed, responsive ANSI panel over [`crate::ui::term`].
//! Eight views (Overview · CPU · Cooling · Memory · Energy · Battery · Network · Storage)
//! sharing an identical header/footer; only the body swaps, like a segmented control. Width
//! tracks the terminal (up to 400 columns) so a wide screen fills with high-resolution
//! braille charts; narrow terminals fall back to a single stacked column. The Overview
//! is a dashboard wall — four tall braille charts (CPU·GPU·PWR·NET) filling the height
//! over a band of compact panels, degrading to compact single-row lanes when narrow.
//! Sampling runs off-thread so keys and quit are instant. Keys: `q`/Ctrl-C quit,
//! `←/→` or `Tab` or `1`-`8` switch view, `space` pause, `+`/`-` speed, `?` help.
//!
//! The module is split by concern: this file owns the engine (sampling loop, key
//! handling, rolling history), [`fmt`] the text/colour helpers, [`frame`] the chrome
//! (header, tab strip, footer, dispatch), and [`views`] the seven tab bodies.

mod fmt;
mod frame;
mod views;

use crate::sensors::snapshot::Snapshot;
use crate::sensors::system::SystemInfo;
use crate::ui::style::Style;
use crate::ui::term::{self, RawMode};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

const SAMPLE_MS: u64 = 250;
/// Rolling history depth. Generous so the braille charts fill even an ultra-wide screen
/// (a braille cell holds 2 samples, so this covers ~512 cells ≈ 360 columns); each frame
/// takes only the tail it needs for the current width.
const HIST: usize = 512;
const MIN_INTERVAL: u64 = 250;
const MAX_INTERVAL: u64 = 5000;
const NTABS: u8 = 8;
pub(super) const TABS: [&str; 8] = [
    "Overview", "CPU", "Cooling", "Memory", "Energy", "Battery", "Network", "Storage",
];

/// Mutable view state the key handler drives.
pub(super) struct Ui {
    pub(super) interval_ms: u64,
    pub(super) paused: bool,
    pub(super) help: bool,
    pub(super) tab: u8,
}

/// Rolling per-signal history that feeds the charts. `cpu`/`rpm`/`pwr` are seeded from
/// the guard's history file at startup; all six fill live as snapshots arrive.
#[derive(Default)]
pub(super) struct Hist {
    pub(super) cpu: Vec<f64>,
    pub(super) gpu: Vec<f64>,
    pub(super) rpm: Vec<f64>,
    pub(super) pwr: Vec<f64>,
    pub(super) net_rx: Vec<f64>,
    pub(super) net_tx: Vec<f64>,
}

/// Static identity shown in the header / Storage tab (read once at startup).
#[derive(Default, Clone)]
pub(super) struct Ident {
    pub(super) label: String, // "Mac mini · Apple M4 Pro · 8P+4E · 16 GPU · 48 GB"
    pub(super) ssd_model: String,
    pub(super) ssd_bytes: u64,
    pub(super) ssd_medium: String,
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
    // Pre-fill the charts from the guard's rolling history, if it's been running.
    let mut hist = load_history();
    let mut last: Option<Snapshot> = None;

    let draw = |snap: &Snapshot, h: &Hist, ui: &Ui, id: &Ident| {
        let mut out = std::io::stdout();
        let _ = out.write_all(frame::render(snap, h, ui, id).as_bytes());
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
            push_hist(&mut hist.cpu, (snap.cpu_load_pct * 100.0) as f64);
            push_hist(&mut hist.gpu, (snap.gpu_active * 100.0) as f64);
            push_hist(&mut hist.rpm, snap.fan_rpm as f64);
            push_hist(&mut hist.pwr, snap.sys_power as f64);
            let (rx_rate, tx_rate) = snap
                .net
                .as_ref()
                .map(|n| (n.rx_rate, n.tx_rate))
                .unwrap_or((0.0, 0.0));
            push_hist(&mut hist.net_rx, rx_rate);
            push_hist(&mut hist.net_tx, tx_rate);
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
                b'1'..=b'9' => {
                    let idx = k - b'1';
                    if idx < NTABS {
                        ui.tab = idx;
                        dirty = true;
                    }
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
            draw(snap, &hist, &ui, &ident);
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
        parts.push(format!("{:.0} GB", fmt::gib(s.ram_bytes)));
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

/// Seed the chart buffers from the guard's rolling history file (`cpu_load,fan_rpm,
/// sys_power`), newest `HIST` rows. The GPU and network buffers start empty and fill
/// live. Empty everywhere if the guard hasn't been running.
fn load_history() -> Hist {
    let mut h = Hist::default();
    let Ok(txt) = std::fs::read_to_string(crate::config::history_path()) else {
        return h;
    };
    let lines: Vec<&str> = txt.lines().collect();
    let start = lines.len().saturating_sub(HIST);
    for line in &lines[start..] {
        let mut it = line.split(',');
        if let (Some(c), Some(r), Some(p)) = (it.next(), it.next(), it.next())
            && let (Ok(c), Ok(r), Ok(p)) = (c.parse(), r.parse(), p.parse())
        {
            h.cpu.push(c);
            h.rpm.push(r);
            h.pwr.push(p);
        }
    }
    h
}

#[cfg(test)]
mod tests {
    use super::frame::{render, render_sized};
    use super::*;
    use crate::sensors::snapshot::{DiskInfo, Level, NetInfo, ProcInfo, Thermal};
    use crate::ui::style::visible_len;

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
    fn hist() -> Hist {
        let mut h = Hist::default();
        for i in 0..140u32 {
            h.cpu.push((i % 100) as f64);
            h.gpu.push(((i * 2) % 100) as f64);
            h.rpm.push(1500.0 + (i % 50) as f64 * 20.0);
            h.pwr.push(10.0 + (i % 30) as f64);
            h.net_rx.push((i % 20) as f64 * 100_000.0);
            h.net_tx.push((i % 10) as f64 * 50_000.0);
        }
        h
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
        s.gpu_active = 0.22;
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
        s.net = Some(NetInfo {
            rx_bytes: 0,
            tx_bytes: 0,
            rx_rate: 1_200_000.0,
            tx_rate: 300_000.0,
        });
        s.battery = Some(crate::ffi::battery::Battery {
            percent: 82,
            charging: false,
            on_ac: false,
            fully_charged: false,
            time_min: Some(110),
            power_w: -27.2,
            temp_c: 31.0,
            cycles: Some(185),
            health_pct: Some(92),
        });
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
        let h = hist();
        for tab in 0..NTABS {
            let out = render(&s, &h, &ui(tab), &ident());
            assert!(out.starts_with("\x1b[H"));
            assert!(out.ends_with("\x1b[J"));
            assert!(out.contains("Apple M4 Pro"));
        }
    }

    #[test]
    fn wide_overview_has_every_lane_and_never_overflows() {
        let out = render_sized(&snap(), &hist(), &ui(0), &ident(), 220, 40);
        for tag in ["CPU", "GPU", "MEM", "HEAT", "PWR", "NET"] {
            assert!(out.contains(tag), "missing lane {tag}");
        }
        // No rendered line may exceed the panel width.
        for l in out.split('\n') {
            assert!(visible_len(l) <= 220, "line too wide: {}", visible_len(l));
        }
    }

    #[test]
    fn narrow_renders_every_tab_within_width() {
        let s = snap();
        let h = hist();
        for tab in 0..NTABS {
            let out = render_sized(&s, &h, &ui(tab), &ident(), 90, 24);
            assert!(out.starts_with("\x1b[H"));
            assert!(out.ends_with("\x1b[J"));
            for l in out.split('\n') {
                assert!(visible_len(l) <= 90, "tab {tab} line too wide");
            }
        }
    }

    #[test]
    fn no_tab_overflows_its_rows() {
        // Every tab must fit within the terminal height — an over-tall body scrolls the
        // header (the tab strip) off the panel. Check a wide/short screen and a laptop.
        let s = snap();
        let h = hist();
        // Include the tiny-terminal danger zone (rows 1..=8): clamp_lines(0) and the chrome
        // floor must keep even a 1-row terminal from scrolling the header off.
        let mut sizes: Vec<(u16, u16)> = vec![(229, 29), (200, 32), (120, 40), (90, 24), (56, 10)];
        for r in 1u16..=8 {
            sizes.push((80, r));
            sizes.push((229, r));
        }
        for &(cols, rows) in &sizes {
            for tab in 0..NTABS {
                let out = render_sized(&s, &h, &ui(tab), &ident(), cols, rows);
                // Strictly fewer newlines than rows: emitting a '\n' on the last row scrolls
                // the panel and pushes the header (with the version) off the top.
                let lines = out.matches('\n').count();
                assert!(
                    lines < rows as usize,
                    "tab {tab} at {cols}x{rows} emitted {lines} newlines (must be < {rows})",
                );
            }
        }
    }

    #[test]
    fn cooling_tab_shows_thermal_and_fans() {
        let out = render_sized(&snap(), &hist(), &ui(2), &ident(), 120, 48);
        assert!(out.contains("Thermal"));
        assert!(out.contains("CPU temp"));
        assert!(out.contains("Fan history"));
        assert!(out.contains("Watchdog"));
    }

    #[test]
    fn memory_tab_is_unmistakable() {
        let out = render_sized(&snap(), &hist(), &ui(3), &ident(), 120, 48);
        assert!(out.contains("In use"));
        assert!(out.contains("available"));
        assert!(out.contains("Pressure"));
        assert!(out.contains("cached"));
    }

    #[test]
    fn memory_tab_explains_why() {
        let out = render_sized(&snap(), &hist(), &ui(3), &ident(), 120, 48);
        // The pressure now carries a reason and names the biggest holder.
        assert!(out.contains("why →"));
        assert!(out.contains("reclaimable"));
        assert!(out.contains("biggest holder"));
        // Compression ratio is surfaced (snap holds 7 GB in 3 GB physical → ~2.3×).
        assert!(out.contains("packed"));
    }

    #[test]
    fn battery_tab_shows_charge_and_health() {
        let out = render(&snap(), &hist(), &ui(5), &ident());
        assert!(out.contains("Charge"));
        assert!(out.contains("82%"));
        assert!(out.contains("Health"));
        assert!(out.contains("185 cycles"));
        assert!(out.contains("draining"));
    }

    #[test]
    fn storage_tab_shows_real_ssd() {
        let out = render(&snap(), &hist(), &ui(7), &ident());
        assert!(out.contains("APPLE SSD AP0512Z"));
        assert!(out.contains("22 GB"));
    }

    #[test]
    fn network_tab_shows_rates() {
        let out = render(&snap(), &hist(), &ui(6), &ident());
        assert!(out.contains("Network"));
        assert!(out.contains("DOWNLOAD"));
        assert!(out.contains("UPLOAD"));
    }

    #[test]
    fn clean_proc_strips_prefix() {
        assert_eq!(
            fmt::clean_proc("com.apple.Virtualization.VM"),
            "Virtualization"
        );
        assert_eq!(fmt::clean_proc("/usr/bin/stress-ng"), "stress-ng");
    }
}
