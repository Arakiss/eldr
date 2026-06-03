//! Human-readable one-shot output: `eldr now`. The terse `check` line and the
//! `status` panel are layered in once the full Snapshot is populated (M2).

use crate::sensors::snapshot::Snapshot;
use crate::ui::style::{Style, bar, gib, human_bytes};

/// `eldr now` — a one-shot snapshot of the machine.
pub fn now(s: &Snapshot) {
    let st = Style::detect();
    println!();
    let gpu = if s.gpu_cores > 0 {
        format!(" {d}·{z} {g} GPU", d = st.dim, z = st.reset, g = s.gpu_cores)
    } else {
        String::new()
    };
    println!(
        "  {b}eldr{z}  {chip} {d}({model}){z}  {b}{p}P{z}+{b}{e}E{z}{gpu}",
        b = st.bold,
        z = st.reset,
        d = st.dim,
        chip = s.chip,
        model = s.mac_model,
        p = s.p_cores,
        e = s.e_cores,
        gpu = gpu,
    );

    // CPU clusters
    println!(
        "  {d}CPU{z}   P {pf:>5} MHz {d}·{z} E {ef:>5} MHz   {pct:>4.1}% {d}busy{z}",
        d = st.dim,
        z = st.reset,
        pf = s.pcpu_freq_mhz,
        ef = s.ecpu_freq_mhz,
        pct = s.cpu_usage_pct * 100.0,
    );

    // GPU
    println!(
        "  {d}GPU{z}   {gf:>5} MHz                {pct:>4.1}% {d}busy{z}",
        d = st.dim,
        z = st.reset,
        gf = s.gpu_freq_mhz,
        pct = s.gpu_active * 100.0,
    );

    // Power
    println!(
        "  {d}Pwr{z}   CPU {cpu:>4.1}W {d}·{z} GPU {gpu:>4.1}W {d}·{z} ANE {ane:>4.1}W {d}·{z} pkg {b}{all:>4.1}W{z}",
        d = st.dim,
        z = st.reset,
        b = st.bold,
        cpu = s.cpu_power,
        gpu = s.gpu_power,
        ane = s.ane_power,
        all = s.all_power,
    );

    // RAM
    let ram_frac = if s.ram_total > 0 {
        s.ram_used as f64 / s.ram_total as f64
    } else {
        0.0
    };
    println!(
        "  {d}RAM{z}   {used:>5.1} / {total:.1} GiB  {bar}  {pct:.0}%",
        d = st.dim,
        z = st.reset,
        used = gib(s.ram_used),
        total = gib(s.ram_total),
        bar = bar(ram_frac, 0.0, 1.0, 18),
        pct = ram_frac * 100.0,
    );

    if s.swap_total > 0 {
        println!(
            "  {d}Swap{z}  {used} / {total}",
            d = st.dim,
            z = st.reset,
            used = human_bytes(s.swap_used),
            total = human_bytes(s.swap_total),
        );
    }
    println!();
}
