//! The eight tab bodies. Each takes the panel width (and a column count for the
//! multi-column tabs) plus the `line`/`blank` sinks from [`super::frame`], and fills the
//! middle of the frame. Charts come from [`crate::ui::chart`]; text and colour from
//! [`super::fmt`]. The Overview is a dashboard wall on a wide screen (tall charts filling
//! the height) and falls back to compact lanes when narrow; the rest fan out into `ncols`
//! columns when there's room and stack to one column when narrow.

use super::fmt::*;
use super::frame::{BlankFn, LineFn};
use super::{Hist, Ident};
use crate::sensors::snapshot::{HOG_CPU_PCT, HOG_RAM_FRAC, Snapshot};
use crate::ui::chart;
use crate::ui::style::{Style, bar_c, human_bytes, sparkline};

// MARK: Banner HUD lane

const LANE_LABEL_W: usize = 7;
const LANE_HERO_W: usize = 8;
const LANE_RIGHT_W: usize = 28;
// Fixed columns before the middle chart: lead(1) + label(7) + sp(1) + hero(8) + gap(2).
const LANE_PREFIX: usize = 1 + LANE_LABEL_W + 1 + LANE_HERO_W + 2;

/// Width available to a lane's middle chart, given the panel width. Keeps the right-hand
/// stats inside their budget so the whole line lands within `w` visible columns.
fn lane_midw(w: usize) -> usize {
    w.saturating_sub(LANE_PREFIX + 2 + LANE_RIGHT_W).max(8)
}

/// One HUD lane: ` LABEL   HERO   <middle chart>   right stats `. `middle` must already
/// be exactly `lane_midw(w)` visible columns wide; `right` must be ≤ `LANE_RIGHT_W`.
fn lane(
    st: &Style,
    label: &str,
    hero: &str,
    hero_color: &str,
    middle: &str,
    right: &str,
) -> String {
    format!(
        " {d}{label:<lw$}{z} {b}{hc}{hero:>hw$}{z}  {middle}  {d}{right}{z}",
        d = st.dim,
        z = st.reset,
        b = st.bold,
        hc = hero_color,
        lw = LANE_LABEL_W,
        hw = LANE_HERO_W,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn body_overview(
    s: &Snapshot,
    h: &Hist,
    id: &Ident,
    st: &Style,
    w: usize,
    ncols: usize,
    rows: usize,
    line: &LineFn,
    blank: &BlankFn,
    f: &mut String,
) {
    overview_callout(s, st, line, f);
    blank(f);
    // Wide screens get the dashboard wall (big charts filling the height); narrow ones
    // fall back to the compact single-row lanes.
    if ncols >= 2 {
        overview_wall(s, h, st, w, rows, line, f);
    } else {
        overview_lanes(s, h, st, w, line, blank, f);
    }
    let _ = id;
}

/// Lead the Overview with a red callout when something is hogging the machine —
/// glanceable on an always-on panel — otherwise the calm status line.
fn overview_callout(s: &Snapshot, st: &Style, line: &LineFn, f: &mut String) {
    let d = st.dim;
    let z = st.reset;
    let r = st.red;
    let (_head, sub) = human_status(s);
    let cpu_hog = s.top_procs.iter().find(|p| p.cpu >= HOG_CPU_PCT);
    let ram_hog = s
        .top_mem
        .iter()
        .find(|p| p.mem as f64 / s.ram_total.max(1) as f64 >= HOG_RAM_FRAC);
    if let Some(p) = cpu_hog {
        line(
            format!(
                " {r}⚠ {} is using {:.0}% CPU{z}{d} — likely the slowdown{z}",
                clean_proc(&p.name),
                p.cpu,
            ),
            f,
        );
    } else if let Some(p) = ram_hog {
        line(
            format!(
                " {r}⚠ {} is holding {:.1} GB RAM{z}{d} ({:.0}% of memory){z}",
                clean_proc(&p.name),
                gib(p.mem),
                p.mem as f64 / s.ram_total.max(1) as f64 * 100.0,
            ),
            f,
        );
    } else {
        line(
            format!(" {d}{sub}   ~90°C is normal here, not a problem.{z}"),
            f,
        );
    }
}

/// Dashboard wall for wide/short screens: four tall braille charts (CPU·GPU·PWR·NET)
/// that grow to fill the vertical space, then a band of compact panels and the process
/// and disk summaries — so the whole 32:9 panel is used, not just the top strip.
fn overview_wall(
    s: &Snapshot,
    h: &Hist,
    st: &Style,
    w: usize,
    rows: usize,
    line: &LineFn,
    f: &mut String,
) {
    let d = st.dim;
    let z = st.reset;
    let b = st.bold;
    let gutter = 2;
    let cells = (w.saturating_sub(1).saturating_sub(3 * gutter)) / 4;

    // Grow the top charts to fill the height left after the chrome, callout, rule, and
    // the bottom panel band (panels 4 + TOP RAM + DISK = 6, plus chart title + stat).
    let reserved =
        4 /*header*/ + 2 /*footer*/ + 2 /*callout+blank*/ + 1 /*rule*/ + 6 /*bottom*/ + 2;
    let chart_h = rows.saturating_sub(reserved).clamp(3, 24);

    let (_cmn, cmx) = min_max(&h.cpu);
    let (_pmn, pmx) = min_max(&h.pwr);
    let (_nmn, nmx) = min_max(&h.net_rx);
    let rx = s.net.as_ref().map(|n| n.rx_rate).unwrap_or(0.0);
    let txr = s.net.as_ref().map(|n| n.tx_rate).unwrap_or(0.0);

    let chart = |title: &str, hero: String, stat: String, data: &[f64], lo: f64, hi: f64| {
        let mut v = Vec::with_capacity(chart_h + 2);
        v.push(format!("{d}{title}{z}  {b}{fire}{hero}{z}", fire = st.fire));
        for row in chart::braille_area(data, lo, hi, cells, chart_h, st.fire, st) {
            v.push(row);
        }
        v.push(format!("{d}{stat}{z}"));
        v
    };
    let cpu = chart(
        "CPU",
        format!("{:.0}%", s.cpu_load_pct * 100.0),
        format!(
            "now {:.0}% · max {:.0}% · {:.1}/{:.1}GHz",
            s.cpu_load_pct * 100.0,
            cmx,
            s.pcpu_freq_mhz as f64 / 1000.0,
            s.ecpu_freq_mhz as f64 / 1000.0,
        ),
        &h.cpu,
        0.0,
        100.0,
    );
    let gpu = chart(
        "GPU",
        format!("{:.0}%", s.gpu_active * 100.0),
        format!(
            "{:.1} GHz · {:.0}°",
            s.gpu_freq_mhz as f64 / 1000.0,
            s.gpu_temp
        ),
        &h.gpu,
        0.0,
        100.0,
    );
    let pwr = chart(
        "PWR",
        format!("{:.0} W", s.sys_power),
        format!("chip {:.0}W · peak {:.0}", s.all_power, pmx),
        &h.pwr,
        0.0,
        pmx.max(1.0),
    );
    let net = chart(
        "NET",
        format!("↓{}", fmt_rate(rx)),
        format!("↓{} · ↑{}", fmt_rate(rx), fmt_rate(txr)),
        &h.net_rx,
        0.0,
        nmx.max(1.0),
    );
    for l in chart::columns(&[cpu, gpu, pwr, net], gutter, st) {
        line(format!(" {l}"), f);
    }

    let rule = "─".repeat(w.saturating_sub(2));
    line(format!(" {d}{rule}{z}"), f);

    // Bottom band: compact panels (memory · heat · cores · top CPU), then the wide
    // process and disk summaries that naturally span the width.
    let press = s.mem_pressure();
    let pc = pressure_color(st, press);
    let app = s.ram_used.saturating_sub(s.ram_wired + s.ram_compressed);
    let barw = (w / 4).saturating_sub(12).clamp(8, 28);
    let mem = vec![
        format!(
            "{d}MEM{z} {pc}{:.0}%{z}",
            s.ram_used as f64 / s.ram_total.max(1) as f64 * 100.0
        ),
        format!(
            "{} {d}{:.0}/{:.0}{z}",
            chart::bar_grad(s.ram_used as f64, 0.0, s.ram_total.max(1) as f64, barw, st),
            gib(s.ram_used),
            gib(s.ram_total),
        ),
        format!(
            "{d}app {:.0} wir {:.0} cmp {:.0}{z}",
            gib(app),
            gib(s.ram_wired),
            gib(s.ram_compressed),
        ),
        format!(
            "{d}swap {:.0}/{:.0} · {press}{z}",
            gib(s.swap_used),
            gib(s.swap_total),
        ),
    ];
    let tc = thermal_color(st, s.thermal);
    let tbar = barw.saturating_sub(6).max(6);
    let fan = if s.fan_rpm > 0 {
        format!("{d}fan {} rpm{z}", s.fan_rpm)
    } else {
        format!("{d}fan idle (cool){z}")
    };
    let heat = vec![
        format!("{d}HEAT{z} {tc}{}{z}", s.thermal.as_str()),
        format!(
            "{d}CPU{z} {} {:.0}°",
            chart::bar_grad(s.cpu_temp as f64, 30.0, 105.0, tbar, st),
            s.cpu_temp,
        ),
        format!(
            "{d}GPU{z} {} {:.0}°",
            chart::bar_grad(s.gpu_temp as f64, 30.0, 105.0, tbar, st),
            s.gpu_temp,
        ),
        fan,
    ];
    let p = s.p_cores as usize;
    let pcore: Vec<f64> = s.per_core.iter().take(p).map(|&x| x as f64).collect();
    let ecore: Vec<f64> = s.per_core.iter().skip(p).map(|&x| x as f64).collect();
    let cores = vec![
        format!("{d}CORES{z} {d}{}P+{}E{z}", s.p_cores, s.e_cores),
        format!("{d}P{z} {}{}{z}", st.fire, sparkline(&pcore, 0.0, 1.0)),
        format!("{d}E{z} {}{}{z}", st.fire, sparkline(&ecore, 0.0, 1.0)),
        format!(
            "{d}{:.1}/{:.1} GHz{z}",
            s.pcpu_freq_mhz as f64 / 1000.0,
            s.ecpu_freq_mhz as f64 / 1000.0,
        ),
    ];
    let mut top = vec![format!("{d}TOP CPU{z}")];
    for p in s.top_procs.iter().take(3) {
        let col = if p.cpu >= HOG_CPU_PCT {
            st.red
        } else if p.cpu >= 100.0 {
            st.fire
        } else {
            z
        };
        top.push(format!(
            "{col}{}{z} {d}{:.0}%{z}",
            clean_proc(&p.name),
            p.cpu
        ));
    }
    while top.len() < 4 {
        top.push(String::new());
    }
    for l in chart::columns(&[mem, heat, cores, top], gutter, st) {
        line(format!(" {l}"), f);
    }
    line(format!(" {d}TOP RAM{z}  {}", procs_mem(s, st, 8)), f);
    line(format!(" {d}DISK{z}     {}", storage_summary(s, st)), f);
}

/// Compact single-row HUD lanes — the fallback for narrow terminals (laptop width),
/// where four charts side by side won't fit.
fn overview_lanes(
    s: &Snapshot,
    h: &Hist,
    st: &Style,
    w: usize,
    line: &LineFn,
    blank: &BlankFn,
    f: &mut String,
) {
    let d = st.dim;
    let z = st.reset;
    let midw = lane_midw(w);

    let cpu_mid = chart::braille_area(&h.cpu, 0.0, 100.0, midw, 1, st.fire, st);
    line(
        lane(
            st,
            "CPU",
            &format!("{:.0}%", s.cpu_load_pct * 100.0),
            st.fire,
            &cpu_mid[0],
            &format!(
                "{:.1}/{:.1} GHz · load {:.1}",
                s.pcpu_freq_mhz as f64 / 1000.0,
                s.ecpu_freq_mhz as f64 / 1000.0,
                s.load_avg.0,
            ),
        ),
        f,
    );
    let gpu_mid = chart::braille_area(&h.gpu, 0.0, 100.0, midw, 1, st.fire, st);
    line(
        lane(
            st,
            "GPU",
            &format!("{:.0}%", s.gpu_active * 100.0),
            st.fire,
            &gpu_mid[0],
            &format!(
                "{:.1} GHz · {:.0}°",
                s.gpu_freq_mhz as f64 / 1000.0,
                s.gpu_temp,
            ),
        ),
        f,
    );
    let press = s.mem_pressure();
    let pc = pressure_color(st, press);
    let mem_mid = chart::bar_grad(s.ram_used as f64, 0.0, s.ram_total.max(1) as f64, midw, st);
    line(
        lane(
            st,
            "MEM",
            &format!(
                "{:.0}%",
                s.ram_used as f64 / s.ram_total.max(1) as f64 * 100.0
            ),
            pc,
            &mem_mid,
            &format!(
                "{:.0}/{:.0} GB · {press}",
                gib(s.ram_used),
                gib(s.ram_total)
            ),
        ),
        f,
    );
    let tc = thermal_color(st, s.thermal);
    let heat_mid = chart::bar_grad(s.cpu_temp as f64, 30.0, 105.0, midw, st);
    line(
        lane(
            st,
            "HEAT",
            &format!("{:.0}°", s.cpu_temp),
            tc,
            &heat_mid,
            &format!("{:.0}° gpu · {}", s.gpu_temp, s.thermal.as_str()),
        ),
        f,
    );
    let (_pmn, pmx) = min_max(&h.pwr);
    let pwr_mid = chart::braille_area(&h.pwr, 0.0, pmx.max(1.0), midw, 1, st.fire, st);
    line(
        lane(
            st,
            "PWR",
            &format!("{:.0} W", s.sys_power),
            st.fire,
            &pwr_mid[0],
            &format!("chip {:.0} W · peak {:.0}", s.all_power, pmx),
        ),
        f,
    );
    let rx = s.net.as_ref().map(|n| n.rx_rate).unwrap_or(0.0);
    let txr = s.net.as_ref().map(|n| n.tx_rate).unwrap_or(0.0);
    let (_nmn, nmx) = min_max(&h.net_rx);
    let net_mid = chart::braille_area(&h.net_rx, 0.0, nmx.max(1.0), midw, 1, st.fire, st);
    line(
        lane(
            st,
            "NET",
            &format!("↓{}", fmt_rate(rx)),
            st.fire,
            &net_mid[0],
            &format!("↓{} · ↑{}", fmt_rate(rx), fmt_rate(txr)),
        ),
        f,
    );

    blank(f);
    let rule = "─".repeat(w.saturating_sub(2));
    line(format!(" {d}{rule}{z}"), f);
    line(format!(" {d}TOP CPU{z}   {}", procs_cpu(s, st, 8)), f);
    line(format!(" {d}TOP RAM{z}   {}", procs_mem(s, st, 8)), f);
    line(format!(" {d}DISK{z}      {}", storage_summary(s, st)), f);
}

/// Vertical room for a tab's tall chart, after the chrome (header 4 + footer 2) and the
/// `reserved` lines the rest of the tab occupies. Clamped so it never collapses or runs
/// away on a very tall screen.
fn tall_h(rows: usize, reserved: usize) -> usize {
    rows.saturating_sub(6 + reserved).clamp(3, 28)
}

/// Split a flat list of cells into `ncols` column-blocks (top-to-bottom within each), for
/// `chart::columns` — spreads per-core bars, power rails and panels across the width.
fn into_columns(cells: &[String], ncols: usize) -> Vec<Vec<String>> {
    let ncols = ncols.max(1);
    let per = cells.len().div_ceil(ncols).max(1);
    cells.chunks(per).map(<[String]>::to_vec).collect()
}

#[allow(clippy::too_many_arguments)]
pub(super) fn body_cpu(
    s: &Snapshot,
    h: &Hist,
    st: &Style,
    w: usize,
    ncols: usize,
    rows: usize,
    line: &LineFn,
    blank: &BlankFn,
    f: &mut String,
) {
    let d = st.dim;
    let z = st.reset;
    let p = s.p_cores as usize;
    let e = s.per_core.len().saturating_sub(p);

    // Total load as a HUD lane.
    let midw = lane_midw(w);
    let load_mid = chart::braille_area(&h.cpu, 0.0, 100.0, midw, 1, st.fire, st);
    line(
        lane(
            st,
            "Load",
            &format!("{:.0}%", s.cpu_load_pct * 100.0),
            st.fire,
            &load_mid[0],
            &format!(
                "avg {:.2}·{:.2}·{:.2}",
                s.load_avg.0, s.load_avg.1, s.load_avg.2
            ),
        ),
        f,
    );
    blank(f);

    // Tall load history, filling the height left over the per-core grid. Reserve every
    // non-chart line: lane, blank, stat, blank, the Performance header + P grid, and the
    // Efficiency header + E grid.
    let pg = p.div_ceil(ncols.max(1)).max(1);
    let eg = if e > 0 {
        e.div_ceil(ncols.max(1)).max(1)
    } else {
        0
    };
    let reserved = 4 + 1 + pg + if e > 0 { 1 + eg } else { 0 };
    let chart_h = tall_h(rows, reserved);
    let chartw = w.saturating_sub(2).max(8);
    let (mn, mx) = min_max(&h.cpu);
    for row in chart::braille_area(&h.cpu, 0.0, 100.0, chartw, chart_h, st.fire, st) {
        line(format!(" {row}"), f);
    }
    line(
        format!(
            " {d}now {:.0}% · busiest {:.0}% · quietest {:.0}% · {} samples{z}",
            s.cpu_load_pct * 100.0,
            mx,
            mn,
            h.cpu.len(),
        ),
        f,
    );
    blank(f);

    // Per-core bars spread across the full width.
    let colw = w
        .saturating_sub(2)
        .saturating_sub(ncols.saturating_sub(1) * 2)
        / ncols.max(1);
    let barw = colw.saturating_sub(9).clamp(6, 50);
    let core_cell = |idx: usize, pre: char, val: f32| {
        format!(
            "{pre}{idx:<2} {} {:>3.0}%",
            chart::fire_bar(val as f64, 0.0, 1.0, barw, st),
            val * 100.0,
        )
    };
    line(
        format!(
            " {d}Performance cores  {} · {:.0}% of max{z}",
            ghz(s.pcpu_freq_mhz),
            s.pcpu_active * 100.0,
        ),
        f,
    );
    let pcells: Vec<String> = s
        .per_core
        .iter()
        .take(p)
        .enumerate()
        .map(|(i, &v)| core_cell(i + 1, 'P', v))
        .collect();
    for l in chart::columns(&into_columns(&pcells, ncols), 2, st) {
        line(format!(" {l}"), f);
    }
    if e > 0 {
        line(
            format!(
                " {d}Efficiency cores  {} · {:.0}% of max{z}",
                ghz(s.ecpu_freq_mhz),
                s.ecpu_active * 100.0,
            ),
            f,
        );
        let ecells: Vec<String> = s
            .per_core
            .iter()
            .skip(p)
            .enumerate()
            .map(|(i, &v)| core_cell(i + 1, 'E', v))
            .collect();
        for l in chart::columns(&into_columns(&ecells, ncols), 2, st) {
            line(format!(" {l}"), f);
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn body_cooling(
    s: &Snapshot,
    h: &Hist,
    st: &Style,
    w: usize,
    ncols: usize,
    rows: usize,
    line: &LineFn,
    blank: &BlankFn,
    f: &mut String,
) {
    let d = st.dim;
    let z = st.reset;
    let tc = thermal_color(st, s.thermal);
    line(
        format!(
            " {d}Thermal{z}  {tc}● {}{z}   {d}{}{z}",
            s.thermal.as_str(),
            thermal_words(s.thermal),
        ),
        f,
    );
    blank(f);

    // Fan history — tall area filling the height over the bottom panels.
    let bottom = if s.fans.len() > 2 {
        s.fans.len() + 2
    } else {
        5
    };
    let chart_h = tall_h(rows, 1 + 1 + 1 + 1 + bottom);
    let chartw = w.saturating_sub(2).max(8);
    let lo = (s.fan_min as f64).min(s.fan_max as f64);
    let hi = s.fan_max.max(1) as f64;
    line(
        format!(
            " {d}Fan history{z}  {d}primary fan, last {} samples{z}",
            h.rpm.len()
        ),
        f,
    );
    for row in chart::braille_area(&h.rpm, lo, hi, chartw, chart_h, st.fire, st) {
        line(format!(" {row}"), f);
    }
    blank(f);

    // Bottom band: temperatures · fans · watchdog, spread across the width.
    let colw = (w
        .saturating_sub(2)
        .saturating_sub((ncols.max(1).max(3) - 1) * 4))
        / ncols.max(3);
    let barw = colw.saturating_sub(14).clamp(8, 40);
    let mut temps = vec![format!("{d}Temps{z}")];
    temps.push(format!(
        "CPU temp  {} {:>3.0}°",
        chart::bar_grad(s.cpu_temp as f64, 30.0, 105.0, barw, st),
        s.cpu_temp,
    ));
    temps.push(format!(
        "GPU temp  {} {:>3.0}°",
        chart::bar_grad(s.gpu_temp as f64, 30.0, 105.0, barw, st),
        s.gpu_temp,
    ));

    let mut fans = vec![format!("{d}Fans{z}")];
    if s.fans.is_empty() {
        fans.push(format!("{d}none — passively cooled or no SMC{z}"));
    } else {
        for (i, fan) in s.fans.iter().enumerate() {
            let failed = fan.max > 0 && fan.target >= 500 && fan.rpm < 500;
            let mark = if failed { st.red } else { "" };
            fans.push(format!(
                "{mark}F{:<2}{z} {} {:>4} rpm",
                i + 1,
                chart::fire_bar(fan.rpm as f64, fan.min as f64, fan.max as f64, barw, st),
                fan.rpm,
            ));
        }
        if let Some(f0) = s.fans.first() {
            fans.push(format!("{d}envelope {}–{} rpm{z}", f0.min, f0.max));
        }
    }

    let watch = vec![
        format!("{d}Watchdog{z}"),
        format!("{d}arms on sustained thermal-{z}"),
        format!("{d}critical or a stalled fan{z}"),
        format!("{d}reversible actions only{z}"),
    ];

    if ncols >= 2 {
        for l in chart::columns(&[temps, fans, watch], 4, st) {
            line(format!(" {l}"), f);
        }
    } else {
        for blk in [&temps, &fans, &watch] {
            for l in blk {
                line(format!(" {l}"), f);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn body_memory(
    s: &Snapshot,
    st: &Style,
    w: usize,
    ncols: usize,
    rows: usize,
    line: &LineFn,
    blank: &BlankFn,
    f: &mut String,
) {
    let d = st.dim;
    let z = st.reset;
    let b = st.bold;
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
            " {d}Total{z} {:.0} GB   {d}·{z} {d}In use{z} {b}{:.1} GB{z}   {d}·{z} {d}Free{z} {b}{:.1} GB{z} {d}available ({:.0}%){z}",
            gib(s.ram_total),
            gib(s.ram_used),
            gib(s.ram_available),
            avail_pct,
        ),
        f,
    );
    blank(f);

    // Memory has no time series, so use the space for a big used/cached/free "tank": a
    // full-width segmented bar, several rows thick, that fills the vertical room.
    let segw = w.saturating_sub(14).clamp(16, 360);
    let comp = seg_bar(s.ram_used, s.ram_cached, s.ram_total, segw, st);
    let bar_h = tall_h(rows, 11);
    for i in 0..bar_h {
        if i == bar_h / 2 {
            line(format!(" {comp}  {d}of {:.0} GB{z}", gib(s.ram_total)), f);
        } else {
            line(format!(" {comp}"), f);
        }
    }
    line(
        format!(
            " {d}▓ in use {:.1}   ▒ cached {:.1}   ░ free {:.1}  (GB){z}",
            gib(s.ram_used),
            gib(s.ram_cached),
            gib(free),
        ),
        f,
    );
    blank(f);

    // Three panels across the width: breakdown · pressure/why/swap · biggest holders.
    let colw = (w
        .saturating_sub(2)
        .saturating_sub((ncols.max(1).max(3) - 1) * 4))
        / ncols.max(3);
    let bw = colw.saturating_sub(24).clamp(8, 40);
    let packed = if s.ram_compressed > 0 && s.ram_compressed_holds > s.ram_compressed {
        format!(
            "{d}holds {:.1} GB ({:.1}× packed){z}",
            gib(s.ram_compressed_holds),
            s.ram_compressed_holds as f64 / s.ram_compressed as f64,
        )
    } else {
        String::new()
    };
    let mut breakdown = vec![format!("{d}What the memory holds{z}")];
    for (label, val) in [
        ("App memory", app),
        ("Wired", s.ram_wired),
        ("Compressed", s.ram_compressed),
        ("Cached files", s.ram_cached),
    ] {
        breakdown.push(format!(
            "{label:<13} {:>6.1} GB {}",
            gib(val),
            chart::fire_bar(val as f64, 0.0, s.ram_total.max(1) as f64, bw, st),
        ));
        if label == "Compressed" && !packed.is_empty() {
            breakdown.push(format!("  {packed}"));
        }
    }
    breakdown.push(format!("{d}cached files are reusable — count as free{z}"));

    let swap_note = if s.swap_used == 0 {
        "macOS hasn't needed to swap — good"
    } else {
        "parked on disk from earlier (clears on reboot)"
    };
    let mut state = vec![format!(
        "{d}Pressure{z}  {pc}● {press}{z}  {d}{}{z}",
        pressure_words(press)
    )];
    state.push(format!("{d}why → {}{z}", why_pressure(s)));
    if let Some(p) = s.top_mem.first() {
        state.push(format!(
            "{d}biggest holder: {} at {:.1} GB{z}",
            clean_proc(&p.name),
            gib(p.mem),
        ));
    }
    state.push(String::new());
    state.push(format!(
        "{d}Swap{z}  {:.1} {d}of{z} {:.1} GB",
        gib(s.swap_used),
        gib(s.swap_total),
    ));
    state.push(format!("{d}{swap_note}{z}"));

    let mut holders = vec![format!("{d}Using most memory{z}")];
    for p in s.top_mem.iter().take(rows.saturating_sub(10).clamp(4, 12)) {
        holders.push(format!(
            "{} {d}{}{z}",
            clean_proc(&p.name),
            human_mem(p.mem),
        ));
    }

    if ncols >= 2 {
        for l in chart::columns(&[breakdown, state, holders], 4, st) {
            line(format!(" {l}"), f);
        }
    } else {
        for blk in [&breakdown, &state, &holders] {
            for l in blk {
                line(format!(" {l}"), f);
            }
            blank(f);
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn body_energy(
    s: &Snapshot,
    h: &Hist,
    st: &Style,
    w: usize,
    ncols: usize,
    rows: usize,
    line: &LineFn,
    blank: &BlankFn,
    f: &mut String,
) {
    let d = st.dim;
    let z = st.reset;
    let b = st.bold;
    let (_mn, mx) = min_max(&h.pwr);
    line(
        format!(
            " {d}Power{z}  Whole machine {b}{:.0} W{z}   {d}· chip package {:.0} W · peak {:.0} W{z}",
            s.sys_power, s.all_power, mx,
        ),
        f,
    );
    blank(f);
    // Tall power history filling the height over the rails. Reserve every non-chart line:
    // power header, blank, stat, blank, the "Where the watts go" title, and the rails.
    let rail_rows = 4usize.div_ceil(ncols.max(1));
    let chart_h = tall_h(rows, 1 + 1 + 1 + 1 + 1 + rail_rows);
    let chartw = w.saturating_sub(2).max(8);
    for row in chart::braille_area(&h.pwr, 0.0, mx.max(1.0), chartw, chart_h, st.fire, st) {
        line(format!(" {row}"), f);
    }
    line(
        format!(
            " {d}now {:.0} W · peak {:.0} W · {} samples{z}",
            s.sys_power,
            mx,
            h.pwr.len(),
        ),
        f,
    );
    blank(f);
    line(
        format!(" {d}Where the watts go{z}  {d}(chip package){z}"),
        f,
    );
    let colw = w
        .saturating_sub(2)
        .saturating_sub(ncols.saturating_sub(1) * 4)
        / ncols.max(1);
    let barw = colw.saturating_sub(14).clamp(10, 60);
    let cap = s.all_power.max(0.1) as f64;
    let rails = [
        ("CPU", s.cpu_power),
        ("GPU", s.gpu_power),
        ("RAM", s.ram_power),
        ("ANE", s.ane_power),
    ];
    let cells: Vec<String> = rails
        .iter()
        .map(|(label, val)| {
            format!(
                "{label:<4} {:>5.1} W {}",
                val,
                chart::fire_bar(*val as f64, 0.0, cap, barw, st),
            )
        })
        .collect();
    if ncols >= 2 {
        for l in chart::columns(&into_columns(&cells, ncols), 4, st) {
            line(format!(" {l}"), f);
        }
    } else {
        for l in cells {
            line(format!(" {l}"), f);
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn body_battery(
    s: &Snapshot,
    st: &Style,
    w: usize,
    ncols: usize,
    rows: usize,
    line: &LineFn,
    blank: &BlankFn,
    f: &mut String,
) {
    let d = st.dim;
    let z = st.reset;
    let Some(bat) = &s.battery else {
        // Desktop (Mac mini / Studio): no battery. Don't leave a near-empty panel — say
        // it plainly and point at where the power readings live.
        for _ in 0..rows.saturating_sub(8) / 2 {
            blank(f);
        }
        line(format!(" {d}No internal battery{z}"), f);
        blank(f);
        line(
            format!(" {d}This Mac runs on wall power (desktop — Mac mini / Studio).{z}"),
            f,
        );
        line(
            format!(" {d}Power draw and history live in the {z}Energy{d} tab.{z}"),
            f,
        );
        return;
    };
    let bc = battery_color(bat.percent, st);
    let state = if bat.charging {
        "charging"
    } else if bat.fully_charged {
        "fully charged"
    } else if bat.on_ac {
        "on AC, not charging"
    } else {
        "on battery"
    };
    let colw = if ncols >= 2 {
        w.saturating_sub(5) / 2
    } else {
        w.saturating_sub(2)
    };
    // Leave room for the longest trailing text ("…% of design · NNN cycles").
    let barw = colw.saturating_sub(36).clamp(8, 40);

    // Left column: charge, health, flow.
    let mut left = vec![format!(
        "{d}Charge{z}  {} {bc}{}%{z} {d}{state}{z}",
        bar_c(bat.percent as f64, 0.0, 100.0, barw, bc, st),
        bat.percent,
    )];
    if let Some(hh) = bat.health_pct {
        let hcol = if hh < 80 { st.yellow } else { st.green };
        let cyc = bat
            .cycles
            .map(|c| format!(" {d}· {c} cycles{z}"))
            .unwrap_or_default();
        left.push(format!(
            "{d}Health{z}  {} {hcol}{hh}%{z} {d}of design{z}{cyc}",
            bar_c(hh as f64, 0.0, 100.0, barw, hcol, st),
        ));
    } else if let Some(c) = bat.cycles {
        left.push(format!("{d}Health{z}  {c} cycles"));
    }
    let (fc, flow) = if bat.power_w > 0.5 {
        (st.green, format!("+{:.1} W charging", bat.power_w))
    } else if bat.power_w < -0.5 {
        (
            st.fire,
            format!("{:.1} W draining the battery", bat.power_w),
        )
    } else {
        (d, "~0 W idle on AC".to_string())
    };
    left.push(format!("{d}Flow{z}    {fc}{flow}{z}"));

    // Right column: time, temp, AC.
    let time = match bat.time_min {
        Some(m) if bat.charging => format!("{}:{:02} to full", m / 60, m % 60),
        Some(m) => format!("{}:{:02} remaining", m / 60, m % 60),
        None if bat.fully_charged => "full".to_string(),
        None => "estimating…".to_string(),
    };
    let mut right = vec![format!("{d}Time{z}    {time}")];
    right.push(format!("{d}Temp{z}    {:.0}°{d} battery{z}", bat.temp_c));
    right.push(format!(
        "{d}AC{z}      {}",
        if bat.on_ac {
            "connected"
        } else {
            "disconnected"
        },
    ));

    if ncols >= 2 {
        for l in chart::columns(&[left, right], 4, st) {
            line(format!(" {l}"), f);
        }
    } else {
        for l in &left {
            line(format!(" {l}"), f);
        }
        blank(f);
        for l in &right {
            line(format!(" {l}"), f);
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn body_network(
    s: &Snapshot,
    h: &Hist,
    st: &Style,
    w: usize,
    ncols: usize,
    rows: usize,
    line: &LineFn,
    blank: &BlankFn,
    f: &mut String,
) {
    let d = st.dim;
    let z = st.reset;
    let net = s.net.as_ref();
    let rx = net.map(|n| n.rx_rate).unwrap_or(0.0);
    let tx = net.map(|n| n.tx_rate).unwrap_or(0.0);
    let (rxb, txb) = net.map(|n| (n.rx_bytes, n.tx_bytes)).unwrap_or((0, 0));
    let (_rmn, rmx) = min_max(&h.net_rx);
    let (_tmn, tmx) = min_max(&h.net_tx);

    line(
        format!(
            " {d}Network{z}   {fr}↓ {}{z} {d}down{z}    {fr}↑ {}{z} {d}up{z}    {d}· since boot  ↓{}  ↑{}{z}",
            fmt_rate(rx),
            fmt_rate(tx),
            human_bytes(rxb),
            human_bytes(txb),
            fr = st.fire,
        ),
        f,
    );
    blank(f);

    // Two tall charts that fill the height: download and upload. Side by side when wide
    // (one chart's height); stacked when narrow, where both charts share the height — so
    // halve it minus their own title/stat/blank lines.
    let stacked = ncols < 2;
    let chart_h = if stacked {
        (rows.saturating_sub(13)) / 2
    } else {
        tall_h(rows, 4)
    }
    .clamp(3, 28);
    let chart = |title: &str, hero: String, stat: String, data: &[f64], hi: f64, cells: usize| {
        let mut v = Vec::with_capacity(chart_h + 2);
        v.push(format!(
            "{d}{title}{z}  {b}{fr}{hero}{z}",
            b = st.bold,
            fr = st.fire
        ));
        for row in chart::braille_area(data, 0.0, hi, cells, chart_h, st.fire, st) {
            v.push(row);
        }
        v.push(format!("{d}{stat}{z}"));
        v
    };
    let dn = chart(
        "DOWNLOAD",
        format!("↓ {}", fmt_rate(rx)),
        format!("now {} · peak {}", fmt_rate(rx), fmt_rate(rmx)),
        &h.net_rx,
        rmx.max(1.0),
        if stacked {
            w.saturating_sub(2)
        } else {
            (w.saturating_sub(3)) / 2
        },
    );
    let up = chart(
        "UPLOAD",
        format!("↑ {}", fmt_rate(tx)),
        format!("now {} · peak {}", fmt_rate(tx), fmt_rate(tmx)),
        &h.net_tx,
        tmx.max(1.0),
        if stacked {
            w.saturating_sub(2)
        } else {
            (w.saturating_sub(3)) / 2
        },
    );
    if stacked {
        for l in &dn {
            line(format!(" {l}"), f);
        }
        blank(f);
        for l in &up {
            line(format!(" {l}"), f);
        }
    } else {
        for l in chart::columns(&[dn, up], 2, st) {
            line(format!(" {l}"), f);
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn body_storage(
    s: &Snapshot,
    id: &Ident,
    st: &Style,
    w: usize,
    ncols: usize,
    rows: usize,
    line: &LineFn,
    blank: &BlankFn,
    f: &mut String,
) {
    let _ = rows;
    let d = st.dim;
    let z = st.reset;
    let colw = if ncols >= 2 {
        (w.saturating_sub(2).saturating_sub((ncols - 1) * 4)) / ncols
    } else {
        w.saturating_sub(2)
    };
    let barw = colw.saturating_sub(14).clamp(10, 50);

    let mkblock = |title: String, total: u64, free: u64| -> Vec<String> {
        let used = total.saturating_sub(free);
        let pct = if total > 0 {
            used as f64 / total as f64 * 100.0
        } else {
            0.0
        };
        let (sc, word) = storage_color(st, free, total);
        vec![
            format!("{d}{title}{z}"),
            format!(
                "Used {} {} {d}of{z} {} GB",
                chart::bar_grad(used as f64, 0.0, total.max(1) as f64, barw, st),
                gb_dec(used),
                gb_dec(total),
            ),
            format!(
                "Free {} GB {d}·{z} {:.0}% full   {sc}● {word}{z}",
                gb_dec(free),
                pct,
            ),
        ]
    };

    let mut blocks: Vec<Vec<String>> = Vec::new();
    if s.volumes.is_empty() {
        if let Some(disk) = &s.disk {
            blocks.push(mkblock(
                "Startup disk “/”".to_string(),
                disk.total,
                disk.free,
            ));
        } else {
            line(format!(" {d}disk info unavailable{z}"), f);
        }
    } else {
        for v in &s.volumes {
            let title = if v.mount_point == "/" {
                "Startup disk “/”".to_string()
            } else {
                format!("{} ({})", v.name, v.mount_point)
            };
            blocks.push(mkblock(title, v.total, v.free));
        }
    }

    // Grid: `ncols` volumes per row when wide, stacked when narrow.
    if ncols >= 2 && blocks.len() > 1 {
        for group in blocks.chunks(ncols) {
            for l in chart::columns(group, 4, st) {
                line(format!(" {l}"), f);
            }
            blank(f);
        }
    } else {
        for (i, blk) in blocks.iter().enumerate() {
            if i > 0 {
                blank(f);
            }
            for l in blk {
                line(format!(" {l}"), f);
            }
        }
    }

    // Per-physical-disk health: bus, kind, I/O errors, and NVMe wear/temp where exposed.
    let disks: Vec<&crate::sensors::snapshot::DiskHealth> = s
        .disk_health
        .iter()
        .filter(|h| !h.model.is_empty() || !h.bsd_name.is_empty())
        .collect();
    if !disks.is_empty() {
        blank(f);
        let (tr, tw) = disks
            .iter()
            .fold((0.0, 0.0), |(r, w), h| (r + h.read_rate, w + h.write_rate));
        line(
            format!(
                " {d}Disks{z}   {fr}↓{} ↑{}{z} {d}total I/O{z}",
                fmt_rate(tr),
                fmt_rate(tw),
                fr = st.fire,
            ),
            f,
        );
        for hd in disks {
            let model = if hd.model.is_empty() {
                hd.bsd_name.clone()
            } else {
                hd.model.clone()
            };
            let kind = if hd.solid_state { "SSD" } else { "HDD" };
            let where_ = if hd.external { "external" } else { "internal" };
            let nvme = hd.nvme.as_ref();
            let wear = nvme
                .map(|n| format!(" {d}·{z} wear {}%", n.percentage_used))
                .unwrap_or_default();
            let temp = nvme
                .filter(|n| n.temp_c > 0.0)
                .map(|n| format!(" {d}·{z} {:.0}°", n.temp_c))
                .unwrap_or_default();
            let errc = if hd.errors() > 0 { st.red } else { d };
            line(
                format!(
                    "   {b}{model}{z}  {d}{where_} · {} · {kind}{z}  {fr}↓{} ↑{}{z}  {errc}err {}{z}{d} · retry {}{z}{wear}{temp}",
                    hd.interconnect,
                    fmt_rate(hd.read_rate),
                    fmt_rate(hd.write_rate),
                    hd.errors(),
                    hd.retries(),
                    b = st.bold,
                    fr = st.fire,
                ),
                f,
            );
        }
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
        let boot_total = s.disk.as_ref().map(|x| x.total).unwrap_or(0);
        line(
            format!(
                " {d}Capacity{z}     {cap}   {d}({} GB usable after formatting){z}{medium}",
                gb_dec(boot_total),
            ),
            f,
        );
    }
}
