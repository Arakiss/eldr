//! Human-readable one-shot output: `eldr now`. The terse `check` line and the
//! `status` panel are layered in once the full Snapshot is populated (M2).

use crate::sensors::snapshot::Snapshot;
use crate::ui::style::{Style, bar, gib, human_bytes};

/// `eldr now` — a one-shot snapshot of the machine.
pub fn now(s: &Snapshot) {
    let st = Style::detect();
    println!();
    println!(
        "  {b}eldr{z}  {chip} {d}({model}){z}  {b}{p}P{z}+{b}{e}E{z}",
        b = st.bold,
        z = st.reset,
        d = st.dim,
        chip = s.chip,
        model = s.mac_model,
        p = s.p_cores,
        e = s.e_cores,
    );

    // RAM
    let ram_frac = if s.ram_total > 0 {
        s.ram_used as f64 / s.ram_total as f64
    } else {
        0.0
    };
    println!(
        "  {d}RAM{z}   {used:>5.1} / {total:<5.1} GiB  {bar}  {pct:.0}%",
        d = st.dim,
        z = st.reset,
        used = gib(s.ram_used),
        total = gib(s.ram_total),
        bar = bar(ram_frac, 0.0, 1.0, 22),
        pct = ram_frac * 100.0,
    );

    // Swap (only when configured)
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
