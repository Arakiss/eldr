//! The seven tab bodies. Each takes the panel width (and a column count for the
//! multi-column tabs) plus the `line`/`blank` sinks from [`super::frame`], and fills the
//! middle of the frame. Charts come from [`crate::ui::chart`]; text and colour from
//! [`super::fmt`]. The Overview is the Banner HUD; the rest fan out into `ncols` columns
//! when there's room and stack to one column when narrow.

use super::fmt::*;
use super::frame::{BlankFn, LineFn};
use super::{Hist, Ident};
use crate::sensors::snapshot::{HOG_CPU_PCT, HOG_RAM_FRAC, Snapshot};
use crate::ui::chart;
use crate::ui::style::{Style, bar_c};

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
    line: &LineFn,
    blank: &BlankFn,
    f: &mut String,
) {
    let d = st.dim;
    let z = st.reset;
    let r = st.red;
    let (_head, sub) = human_status(s);
    // When something is hogging the machine, lead with a red callout — glanceable on an
    // always-on wide panel. Otherwise the calm status line.
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
    blank(f);

    let midw = lane_midw(w);

    // CPU — fire activity, history area.
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
    // GPU — fire activity.
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
    // MEM — health gradient by used fraction.
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
    // HEAT — health gradient over the operating range; the thermal word carries truth.
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
    // PWR — fire activity, history area scaled to its own peak.
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
    // NET — download area; both rates on the right.
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

    // TOP processes (horizontal — saves vertical space on a low panel) + disk summary.
    line(format!(" {d}TOP CPU{z}   {}", procs_cpu(s, st, 8)), f);
    line(format!(" {d}TOP RAM{z}   {}", procs_mem(s, st, 8)), f);
    line(format!(" {d}DISK{z}      {}", storage_summary(s, st)), f);
    let _ = id;
}

#[allow(clippy::too_many_arguments)]
pub(super) fn body_cpu(
    s: &Snapshot,
    h: &Hist,
    st: &Style,
    w: usize,
    ncols: usize,
    line: &LineFn,
    blank: &BlankFn,
    f: &mut String,
) {
    let d = st.dim;
    let z = st.reset;
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

    // Tall history area across the full width.
    let chartw = w.saturating_sub(2).max(8);
    let (mn, mx) = min_max(&h.cpu);
    for row in chart::braille_area(&h.cpu, 0.0, 100.0, chartw, 4, st.fire, st) {
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

    // Per-core bars — Performance cluster | Efficiency cluster, side by side when wide.
    let p = s.p_cores as usize;
    let colw = if ncols >= 2 {
        w.saturating_sub(5) / 2
    } else {
        w.saturating_sub(2)
    };
    let barw = colw.saturating_sub(11).clamp(8, 60);
    let block = |title: &str, freq: String, act: f32, cores: &[f32], pre: char| -> Vec<String> {
        let mut v = Vec::with_capacity(cores.len() + 1);
        v.push(format!(
            "{d}{title}  {freq} · {:.0}% of max{z}",
            act * 100.0
        ));
        for (i, val) in cores.iter().enumerate() {
            v.push(format!(
                "{pre}{:<2} {} {:>3.0}%",
                i + 1,
                chart::fire_bar(*val as f64, 0.0, 1.0, barw, st),
                val * 100.0,
            ));
        }
        v
    };
    let pcores: Vec<f32> = s.per_core.iter().take(p).copied().collect();
    let ecores: Vec<f32> = s.per_core.iter().skip(p).copied().collect();
    let pblock = block(
        "Performance cores",
        ghz(s.pcpu_freq_mhz),
        s.pcpu_active,
        &pcores,
        'P',
    );
    let eblock = block(
        "Efficiency cores",
        ghz(s.ecpu_freq_mhz),
        s.ecpu_active,
        &ecores,
        'E',
    );
    if ncols >= 2 {
        for l in chart::columns(&[pblock, eblock], 4, st) {
            line(format!(" {l}"), f);
        }
    } else {
        for l in &pblock {
            line(format!(" {l}"), f);
        }
        blank(f);
        for l in &eblock {
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

    let colw = if ncols >= 2 {
        w.saturating_sub(5) / 2
    } else {
        w.saturating_sub(2)
    };
    let barw = colw.saturating_sub(16).clamp(8, 50);

    // Temps — health gradient over the operating range; the thermal word carries truth.
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

    // Fans — fire activity over each fan's envelope.
    let mut fans = vec![format!("{d}Fans{z}")];
    if s.fans.is_empty() {
        fans.push(format!(
            "{d}none reported — passively cooled or SMC unavailable{z}"
        ));
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
            fans.push(format!(
                "{d}envelope {}–{} rpm · 0% = idle floor{z}",
                f0.min, f0.max,
            ));
        }
    }

    if ncols >= 2 {
        for l in chart::columns(&[temps, fans], 4, st) {
            line(format!(" {l}"), f);
        }
    } else {
        for l in &temps {
            line(format!(" {l}"), f);
        }
        blank(f);
        for l in &fans {
            line(format!(" {l}"), f);
        }
    }
    blank(f);

    // Fan history — tall area across the full width.
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
    for row in chart::braille_area(&h.rpm, lo, hi, chartw, 3, st.fire, st) {
        line(format!(" {row}"), f);
    }
    blank(f);
    line(
        format!(
            " {d}Watchdog{z}  {d}arms on sustained thermal-critical or a stalled fan — reversible actions only{z}"
        ),
        f,
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn body_memory(
    s: &Snapshot,
    st: &Style,
    w: usize,
    ncols: usize,
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

    let colw = if ncols >= 2 {
        w.saturating_sub(5) / 2
    } else {
        w.saturating_sub(2)
    };

    // Left column: composition (segmented bar + breakdown).
    let segw = colw.saturating_sub(10).clamp(16, 90);
    let bw = colw.saturating_sub(24).clamp(10, 40);
    let mut left = vec![format!("{d}Composition{z}")];
    left.push(format!(
        "{}  {d}of {:.0} GB{z}",
        seg_bar(s.ram_used, s.ram_cached, s.ram_total, segw, st),
        gib(s.ram_total),
    ));
    left.push(format!(
        "{d}▓ in use {:.1}   ▒ cached {:.1}   ░ free {:.1}  (GB){z}",
        gib(s.ram_used),
        gib(s.ram_cached),
        gib(free),
    ));
    left.push(String::new());
    left.push(format!("{d}What the memory holds{z}"));
    // macOS packs more into the compressor than the bytes it occupies; the ratio shows
    // why the machine fits more than its RAM size suggests.
    let packed = if s.ram_compressed > 0 && s.ram_compressed_holds > s.ram_compressed {
        format!(
            "{d}holds {:.1} GB ({:.1}× packed){z}",
            gib(s.ram_compressed_holds),
            s.ram_compressed_holds as f64 / s.ram_compressed as f64,
        )
    } else {
        String::new()
    };
    for (label, val) in [
        ("App memory", app),
        ("Wired", s.ram_wired),
        ("Compressed", s.ram_compressed),
        ("Cached files", s.ram_cached),
    ] {
        left.push(format!(
            "{label:<13} {:>6.1} GB {}",
            gib(val),
            chart::fire_bar(val as f64, 0.0, s.ram_total.max(1) as f64, bw, st),
        ));
        if label == "Compressed" && !packed.is_empty() {
            left.push(format!("  {packed}"));
        }
    }
    left.push(format!(
        "{d}cached files are reusable — they count as free{z}"
    ));

    // Right column: pressure + why + swap + biggest holders.
    let mut right = vec![format!(
        "{d}Pressure{z}  {pc}● {press}{z}  {d}{}{z}",
        pressure_words(press)
    )];
    right.push(format!("{d}why → {}{z}", why_pressure(s)));
    if let Some(p) = s.top_mem.first() {
        right.push(format!(
            "{d}      biggest holder: {} at {:.1} GB{z}",
            clean_proc(&p.name),
            gib(p.mem),
        ));
    }
    right.push(String::new());
    let swap_note = if s.swap_used == 0 {
        "macOS hasn't needed to swap — good"
    } else {
        "parked on disk from earlier (clears on reboot)"
    };
    right.push(format!(
        "{d}Swap{z}  {:.1} {d}of{z} {:.1} GB",
        gib(s.swap_used),
        gib(s.swap_total),
    ));
    right.push(format!("{d}{swap_note}{z}"));
    right.push(String::new());
    right.push(format!("{d}Using most memory{z}"));
    for p in s.top_mem.iter().take(6) {
        right.push(format!(
            "  {} {d}{}{z}",
            clean_proc(&p.name),
            human_mem(p.mem),
        ));
    }

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
pub(super) fn body_energy(
    s: &Snapshot,
    h: &Hist,
    st: &Style,
    w: usize,
    ncols: usize,
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
    // Tall power history across the full width.
    let chartw = w.saturating_sub(2).max(8);
    for row in chart::braille_area(&h.pwr, 0.0, mx.max(1.0), chartw, 4, st.fire, st) {
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
    let colw = if ncols >= 2 {
        w.saturating_sub(5) / 2
    } else {
        w.saturating_sub(2)
    };
    let barw = colw.saturating_sub(16).clamp(10, 60);
    let cap = s.all_power.max(0.1) as f64;
    let rails = [
        ("CPU", s.cpu_power),
        ("GPU", s.gpu_power),
        ("RAM", s.ram_power),
        ("ANE", s.ane_power),
    ];
    let mk = |items: &[(&str, f32)]| -> Vec<String> {
        items
            .iter()
            .map(|(label, val)| {
                format!(
                    "{label:<4} {:>5.1} W {}",
                    val,
                    chart::fire_bar(*val as f64, 0.0, cap, barw, st),
                )
            })
            .collect()
    };
    if ncols >= 2 {
        let left = mk(&rails[0..2]);
        let right = mk(&rails[2..4]);
        for l in chart::columns(&[left, right], 4, st) {
            line(format!(" {l}"), f);
        }
    } else {
        for l in mk(&rails) {
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
    line: &LineFn,
    blank: &BlankFn,
    f: &mut String,
) {
    let d = st.dim;
    let z = st.reset;
    let Some(bat) = &s.battery else {
        line(format!(" {d}No internal battery{z}"), f);
        line(
            format!(" {d}This Mac runs on wall power (desktop — Mac mini / Studio).{z}"),
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
pub(super) fn body_storage(
    s: &Snapshot,
    id: &Ident,
    st: &Style,
    w: usize,
    ncols: usize,
    line: &LineFn,
    blank: &BlankFn,
    f: &mut String,
) {
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
