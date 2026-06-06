//! Human-readable text output: the `now`/`status` panel and the terse `check` line.

use crate::ffi::smc;
use crate::sensors::snapshot::{Level, Snapshot, Thermal};
use crate::ui::style::{Style, bar, gib, human_bytes, sparkline};

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

fn human_rate(bps: f64) -> String {
    let mib = 1024.0 * 1024.0;
    if bps >= mib {
        format!("{:.1} MB/s", bps / mib)
    } else if bps >= 1024.0 {
        format!("{:.0} KB/s", bps / 1024.0)
    } else {
        format!("{:.0} B/s", bps)
    }
}

/// `eldr now` / `eldr status` — a full one-shot panel.
pub fn panel(s: &Snapshot, note: &str) {
    let st = Style::detect();
    let lc = level_color(&st, s.level);
    let tc = thermal_color(&st, s.thermal);

    println!();
    let gpu = if s.gpu_cores > 0 {
        format!(
            " {d}·{z} {g} GPU",
            d = st.dim,
            z = st.reset,
            g = s.gpu_cores
        )
    } else {
        String::new()
    };
    println!(
        "  {b}eldr{z}  {chip} {d}({model}){z}  {b}{p}P{z}+{b}{e}E{z}{gpu}   {lc}{b}{lvl}{z} {d}{note}{z}",
        b = st.bold,
        z = st.reset,
        d = st.dim,
        chip = s.chip,
        model = s.mac_model,
        p = s.p_cores,
        e = s.e_cores,
        gpu = gpu,
        lc = lc,
        lvl = s.level.as_str(),
        note = note,
    );

    // CPU: cluster freqs + load + per-core sparkline
    let cores = sparkline(
        &s.per_core.iter().map(|&v| v as f64).collect::<Vec<_>>(),
        0.0,
        1.0,
    );
    println!(
        "  {d}CPU{z}   P {pf:>4} {d}·{z} E {ef:>4} MHz   {load:>3.0}% {d}load{z} {d}·{z} {busy:>3.0}% {d}busy{z}   {cores}",
        d = st.dim,
        z = st.reset,
        pf = s.pcpu_freq_mhz,
        ef = s.ecpu_freq_mhz,
        load = s.cpu_load_pct * 100.0,
        busy = s.cpu_usage_pct * 100.0,
        cores = cores,
    );

    println!(
        "  {d}GPU{z}   {gf:>4} MHz   {busy:>3.0}% {d}busy{z}",
        d = st.dim,
        z = st.reset,
        gf = s.gpu_freq_mhz,
        busy = s.gpu_active * 100.0,
    );

    println!(
        "  {d}Pwr{z}   CPU {cpu:>4.1} {d}·{z} GPU {gpu:>4.1} {d}·{z} ANE {ane:>4.1} {d}·{z} pkg {b}{all:>4.1}{z} {d}·{z} sys {sys:>4.1} W",
        d = st.dim,
        z = st.reset,
        b = st.bold,
        cpu = s.cpu_power,
        gpu = s.gpu_power,
        ane = s.ane_power,
        all = s.all_power,
        sys = s.sys_power,
    );

    // Temps + fan + thermal
    let fan = if s.fan_max > 0 {
        format!(
            "{rpm} rpm {d}({min}–{max}){z}",
            rpm = s.fan_rpm,
            min = s.fan_min,
            max = s.fan_max,
            d = st.dim,
            z = st.reset,
        )
    } else {
        format!("{d}n/a{z}", d = st.dim, z = st.reset)
    };
    println!(
        "  {d}Tmp{z}   CPU {ct:>2.0}°C {d}·{z} GPU {gt:>2.0}°C   {d}fan{z} {fan}   {d}thermal{z} {tc}{th}{z}",
        d = st.dim,
        z = st.reset,
        ct = s.cpu_temp,
        gt = s.gpu_temp,
        fan = fan,
        tc = tc,
        th = s.thermal.as_str(),
    );

    // RAM — occupied vs available, with plain-language pressure (not a raw "% used").
    let ram_frac = if s.ram_total > 0 {
        s.ram_used as f64 / s.ram_total as f64
    } else {
        0.0
    };
    let press = s.mem_pressure();
    let pc = match press {
        "low" => st.green,
        "medium" => st.yellow,
        "high" => st.red,
        _ => st.dim,
    };
    println!(
        "  {d}RAM{z}   {bar}  {used:.0} {d}of{z} {total:.0} GB used {d}·{z} {avail:.0} GB free {d}·{z} {pc}{press}{z} {d}pressure{z}",
        d = st.dim,
        z = st.reset,
        bar = bar(ram_frac, 0.0, 1.0, 16),
        used = gib(s.ram_used),
        total = gib(s.ram_total),
        avail = gib(s.ram_available),
        pc = pc,
        press = press,
    );

    // Disk(s) + net — one entry per mounted volume (boot + external/data).
    if !s.volumes.is_empty() {
        let vols = s
            .volumes
            .iter()
            .map(|v| {
                let used = v.total.saturating_sub(v.free);
                format!(
                    "{name} {used}{d}/{z}{tot}",
                    name = v.name,
                    used = human_bytes(used),
                    tot = human_bytes(v.total),
                    d = st.dim,
                    z = st.reset,
                )
            })
            .collect::<Vec<_>>()
            .join(&format!(" {d}·{z} ", d = st.dim, z = st.reset));
        let net = if let Some(n) = &s.net {
            format!(
                "   {d}net{z} ↓{rx} ↑{tx}",
                rx = human_rate(n.rx_rate),
                tx = human_rate(n.tx_rate),
                d = st.dim,
                z = st.reset,
            )
        } else {
            String::new()
        };
        println!("  {d}Dsk{z}   {vols}{net}", d = st.dim, z = st.reset);
    } else if let Some(d) = &s.disk {
        // Fallback: volume enumeration failed — show the boot disk alone.
        let used = d.total.saturating_sub(d.free);
        let line = if let Some(n) = &s.net {
            format!(
                "{used} / {total} {d}used{z}   {d}net{z} ↓{rx} ↑{tx}",
                used = human_bytes(used),
                total = human_bytes(d.total),
                rx = human_rate(n.rx_rate),
                tx = human_rate(n.tx_rate),
                d = st.dim,
                z = st.reset,
            )
        } else {
            format!(
                "{used} / {total} used",
                used = human_bytes(used),
                total = human_bytes(d.total),
            )
        };
        println!("  {d}Dsk{z}   {line}", d = st.dim, z = st.reset);
    }

    // Top processes
    if !s.top_procs.is_empty() {
        let tops = s
            .top_procs
            .iter()
            .take(4)
            .map(|p| format!("{} {d}{:.0}%{z}", p.name, p.cpu, d = st.dim, z = st.reset))
            .collect::<Vec<_>>()
            .join("  ");
        println!("  {d}Top{z}   {tops}", d = st.dim, z = st.reset);
    }

    // Top processes by memory footprint
    if !s.top_mem.is_empty() {
        let mems = s
            .top_mem
            .iter()
            .take(4)
            .map(|p| {
                format!(
                    "{} {d}{}{z}",
                    p.name,
                    human_bytes(p.mem),
                    d = st.dim,
                    z = st.reset
                )
            })
            .collect::<Vec<_>>()
            .join("  ");
        println!("  {d}Mem{z}   {mems}", d = st.dim, z = st.reset);
    }

    println!();
}

/// `eldr sensors` — every SMC sensor, grouped (temps, fans, power, currents, voltages).
pub fn sensors_panel() {
    let st = Style::detect();
    let sensors = smc::all_sensors();
    println!();
    if sensors.is_empty() {
        println!("  {d}no SMC sensors available{z}", d = st.dim, z = st.reset);
        return;
    }
    for group in [
        smc::SensorGroup::Temp,
        smc::SensorGroup::Fan,
        smc::SensorGroup::Power,
        smc::SensorGroup::Current,
        smc::SensorGroup::Voltage,
    ] {
        let mut rows: Vec<&smc::Sensor> = sensors.iter().filter(|s| s.group == group).collect();
        if rows.is_empty() {
            continue;
        }
        rows.sort_by(|a, b| a.key.cmp(&b.key));
        println!(
            "  {b}{title}{z}  {d}({n}){z}",
            b = st.bold,
            z = st.reset,
            d = st.dim,
            title = group.title(),
            n = rows.len(),
        );
        // Two columns keep the long lists compact.
        let cell = |s: &smc::Sensor| {
            format!(
                "{d}{key:<5}{z} {val:>7.1} {unit:<3}",
                d = st.dim,
                z = st.reset,
                key = s.key,
                val = s.value,
                unit = group.unit(),
            )
        };
        for pair in rows.chunks(2) {
            let right = pair.get(1).map(|s| cell(s)).unwrap_or_default();
            println!("    {}   {right}", cell(pair[0]));
        }
        println!();
    }
}

/// `eldr check` — one terse line; the caller exits with `s.level.exit_code()`.
pub fn check_line(s: &Snapshot) {
    println!(
        "{lvl} cpu={busy:.0}% temp={ct:.0}C fan={rpm}rpm thermal={th} pkg={pkg:.1}W",
        lvl = s.level.as_str(),
        busy = s.cpu_load_pct * 100.0,
        ct = s.cpu_temp,
        rpm = s.fan_rpm,
        th = s.thermal.as_str(),
        pkg = s.all_power,
    );
}
