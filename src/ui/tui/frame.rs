//! The frame chrome shared by every tab: the home/clear sequence, the identity header,
//! the tab strip, the bottom-pinned footer, and the per-line clip that keeps content
//! inside the panel width. [`render_sized`] is the testable core; [`render`] feeds it the
//! live terminal size. The body itself is dispatched to [`super::views`].

use super::fmt::{fmt_uptime, human_status, level_color};
use super::{Hist, Ident, TABS, Ui, views};
use crate::sensors::snapshot::Snapshot;
use crate::ui::style::{Style, fit};
use crate::ui::term;

/// A "draw one clipped line" sink: clips to the panel width, clears to end of line, and
/// breaks. Bodies receive this so they never have to think about the column budget.
pub(super) type LineFn<'a> = dyn Fn(String, &mut String) + 'a;
/// A "draw one blank line" sink (clear to end of line + newline).
pub(super) type BlankFn<'a> = dyn Fn(&mut String) + 'a;

pub(super) fn render(s: &Snapshot, h: &Hist, ui: &Ui, id: &Ident) -> String {
    let (cols, rows) = term::size();
    render_sized(s, h, ui, id, cols, rows)
}

/// The frame, sized explicitly. [`render`] calls this with the live terminal size; tests
/// drive it with fixed dimensions to exercise wide and narrow layouts without a tty.
pub(super) fn render_sized(
    s: &Snapshot,
    h: &Hist,
    ui: &Ui,
    id: &Ident,
    cols: u16,
    rows: u16,
) -> String {
    render_styled(Style::detect(), s, h, ui, id, cols, rows)
}

/// The frame with an explicit palette. Splitting the palette out keeps `render_sized` the
/// production entry point while letting tests (and the docs-asset generator) force the
/// truecolor or ansi16 palette without a tty.
pub(super) fn render_styled(
    st: Style,
    s: &Snapshot,
    h: &Hist,
    ui: &Ui,
    id: &Ident,
    cols: u16,
    rows: u16,
) -> String {
    // Use the whole terminal, up to a sane ceiling. Multi-column bodies kick in past
    // ~112 columns; below that everything degrades to one stacked column.
    let w = (cols as usize).clamp(56, 400);
    let ncols = (w / 56).clamp(1, 4);
    let d = st.dim;
    let z = st.reset;
    let b = st.bold;

    let mut f = String::with_capacity(8192);
    f.push_str(term::home());
    let rule = "─".repeat(w.saturating_sub(2));

    let line = |row: String, out: &mut String| {
        out.push_str(&fit(&row, w));
        out.push_str(term::clear_eol());
        out.push('\n');
    };
    let blank = |out: &mut String| {
        out.push_str(term::clear_eol());
        out.push('\n');
    };
    let pad = |left: usize, right: usize| " ".repeat(w.saturating_sub(left + right).max(1));

    // ---- header (identical on every tab) ----
    let (head, _sub) = human_status(s);
    let lc = level_color(&st, s.level);
    let ident = if id.label.is_empty() {
        s.chip.clone()
    } else {
        id.label.clone()
    };
    let ver = env!("CARGO_PKG_VERSION");
    let right = format!("{head}  ·  up {}", fmt_uptime(s.uptime_secs));
    line(
        format!(
            " {b}eldr{z} {b}{fire}v{ver}{z}  {d}{ident}{z}{sp}{lc}●{z} {b}{head}{z} {d}· up {up}{z}",
            fire = st.fire,
            // visible prefix before ident: " eldr v{ver}  " = 9 + ver.len()
            sp = pad(
                9 + ver.len() + ident.chars().count(),
                right.chars().count() + 2
            ),
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
    let speed = format!("↻ {:.1}s", ui.interval_ms as f64 / 1000.0);
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
    // Build the body into its own buffer so it can be clamped: a tab that over-estimates
    // its height (the tall-chart tabs) must never push the header/footer off-screen and
    // scroll the panel. The header is 4 lines; reserve the footer too.
    let rows = rows as usize;
    let footer_lines = 1 + if ui.help { 2 } else { 1 };
    let mut body = String::new();
    match ui.tab {
        0 => views::body_overview(s, h, id, &st, w, ncols, rows, &line, &blank, &mut body),
        1 => views::body_cpu(s, h, &st, w, ncols, rows, &line, &blank, &mut body),
        2 => views::body_cooling(s, h, &st, w, ncols, rows, &line, &blank, &mut body),
        3 => views::body_memory(s, &st, w, ncols, rows, &line, &blank, &mut body),
        4 => views::body_energy(s, h, &st, w, ncols, rows, &line, &blank, &mut body),
        5 => views::body_battery(s, &st, w, ncols, rows, &line, &blank, &mut body),
        6 => views::body_network(s, h, &st, w, ncols, rows, &line, &blank, &mut body),
        _ => views::body_storage(s, id, &st, w, ncols, rows, &line, &blank, &mut body),
    }
    clamp_lines(&mut body, rows.saturating_sub(4 + footer_lines));
    f.push_str(&body);

    // ---- footer pinned to the bottom (pad with the real row count) ----
    let target = rows.saturating_sub(footer_lines);
    while f.matches('\n').count() < target {
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
                " {d}q{z} Quit {d}·{z} {d}←→/Tab{z} Views {d}·{z} {d}1-8{z} Jump {d}·{z} {d}space{z} Pause {d}·{z} {d}+−{z} Speed {d}·{z} {d}?{z} Help{paused}"
            ),
            &mut f,
        );
    }

    // Drop the final newline: writing a '\n' on the last terminal row scrolls the panel up
    // by one, pushing the header (with the version) off the top. Leaving the last line
    // without a trailing newline keeps the cursor on the bottom row — no scroll.
    if f.ends_with('\n') {
        f.pop();
    }
    f.push_str(term::clear_eos());
    f
}

/// Keep at most `max` newline-terminated lines of `s`, dropping the rest. The safety net
/// that stops an over-tall body from scrolling the header and footer off the panel.
fn clamp_lines(s: &mut String, max: usize) {
    let mut count = 0;
    let mut cut = None;
    for (i, b) in s.bytes().enumerate() {
        if b == b'\n' {
            count += 1;
            if count == max {
                cut = Some(i + 1);
                break;
            }
        }
    }
    if let Some(c) = cut {
        s.truncate(c);
    }
}
