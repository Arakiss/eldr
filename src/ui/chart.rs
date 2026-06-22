//! Owned graphics primitives for the responsive TUI — braille area charts, gradient
//! bars, and a column compositor. No `ratatui`, no plotting crate: just Unicode and
//! ANSI bytes we emit ourselves, sized to whatever width the panel hands us.
//!
//! Gradients (`bar_grad`, `fire_bar`) interpolate RGB and only emit truecolor codes
//! when the terminal advertises 24-bit colour (`st.truecolor`); on ansi16 they fall
//! back to a solid per-zone colour so a basic terminal never sees garbage.

use crate::ui::style::{Style, visible_len};

// Braille cell: base codepoint + a 2-wide × 4-tall dot grid. `DOT[sub_col][dot_row]`
// is the bit to set for the dot at horizontal half `sub_col` (0 = left, 1 = right) and
// vertical position `dot_row` (0 = top … 3 = bottom).
const BRAILLE_BASE: u32 = 0x2800;
const DOT: [[u8; 4]; 2] = [
    [0x01, 0x02, 0x04, 0x40], // left column
    [0x08, 0x10, 0x20, 0x80], // right column
];

/// A filled "mountain" area chart in braille. `w_cells` wide × `h_rows` tall cells give
/// `2·w_cells` horizontal samples and `4·h_rows` vertical levels. Renders the tail of
/// `vals` (newest at the right), each column filled from the baseline up to its value
/// scaled across `[lo, hi]`. Returns exactly `h_rows` lines, each `w_cells` visible
/// columns wide (top row first), painted `color`.
pub fn braille_area(
    vals: &[f64],
    lo: f64,
    hi: f64,
    w_cells: usize,
    h_rows: usize,
    color: &str,
    st: &Style,
) -> Vec<String> {
    let w_cells = w_cells.max(1);
    let h_rows = h_rows.max(1);
    let sub_cols = w_cells * 2;
    let levels = h_rows * 4;
    let span = (hi - lo).max(f64::MIN_POSITIVE);

    // grid[row][cell] accumulates dot bits; row 0 is the top.
    let mut grid = vec![vec![0u8; w_cells]; h_rows];

    // Right-align the last `sub_cols` samples; the left side stays empty if we have fewer.
    let n = vals.len();
    let take = n.min(sub_cols);
    let pad = sub_cols - take;
    for x in pad..sub_cols {
        let v = vals[(n - take) + (x - pad)];
        let frac = ((v - lo) / span).clamp(0.0, 1.0);
        let ht = (frac * levels as f64).round() as usize; // filled levels from the bottom
        let cell_col = x / 2;
        let sub = x % 2;
        for l in 0..ht {
            let pos_from_top = levels - 1 - l;
            grid[pos_from_top / 4][cell_col] |= DOT[sub][pos_from_top % 4];
        }
    }

    grid.into_iter()
        .map(|row| {
            let mut s = String::with_capacity(w_cells * 3 + color.len() + st.reset.len());
            s.push_str(color);
            for bits in row {
                s.push(char::from_u32(BRAILLE_BASE + bits as u32).unwrap_or(' '));
            }
            s.push_str(st.reset);
            s
        })
        .collect()
}

// Health ramp stops (green → amber → red) and the fire ramp (ember → flame). RGB matches
// the brand palette in `style.rs`.
const C_GREEN: (u8, u8, u8) = (39, 201, 63);
const C_AMBER: (u8, u8, u8) = (255, 189, 46);
const C_RED: (u8, u8, u8) = (255, 95, 86);
const C_FIRE_LO: (u8, u8, u8) = (120, 34, 12);
const C_FIRE_HI: (u8, u8, u8) = (255, 176, 64);

fn lerp_rgb(a: (u8, u8, u8), b: (u8, u8, u8), t: f64) -> (u8, u8, u8) {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * t).round() as u8;
    (mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

/// Health ramp: green at `t=0` → amber at `t=0.5` → red at `t=1`.
fn health_rgb(t: f64) -> (u8, u8, u8) {
    if t < 0.5 {
        lerp_rgb(C_GREEN, C_AMBER, t * 2.0)
    } else {
        lerp_rgb(C_AMBER, C_RED, (t - 0.5) * 2.0)
    }
}

/// Fire ramp: dark ember at `t=0` → bright flame at `t=1`.
fn fire_rgb(t: f64) -> (u8, u8, u8) {
    lerp_rgb(C_FIRE_LO, C_FIRE_HI, t)
}

fn truecolor(r: u8, g: u8, b: u8) -> String {
    format!("\x1b[38;2;{r};{g};{b}m")
}

fn frac_of(v: f64, lo: f64, hi: f64) -> f64 {
    let span = (hi - lo).max(f64::MIN_POSITIVE);
    ((v - lo) / span).clamp(0.0, 1.0)
}

/// A `w`-cell bar whose filled run is a per-cell RGB gradient along `ramp` (cell `i`
/// takes the ramp at `i/(w-1)`, so a fuller bar reaches deeper into the ramp). Always
/// returns exactly `w` visible columns.
fn grad_cells(filled: usize, w: usize, st: &Style, ramp: impl Fn(f64) -> (u8, u8, u8)) -> String {
    let filled = filled.min(w);
    let mut s = String::with_capacity(w * 20);
    for i in 0..filled {
        let t = if w > 1 {
            i as f64 / (w - 1) as f64
        } else {
            0.0
        };
        let (r, g, b) = ramp(t);
        s.push_str(&truecolor(r, g, b));
        s.push('█');
    }
    s.push_str(st.dim);
    for _ in filled..w {
        s.push('░');
    }
    s.push_str(st.reset);
    s
}

/// Solid-colour fallback bar (ansi16 / non-tty): one colour for the whole filled run.
fn solid_cells(filled: usize, w: usize, color: &str, st: &Style) -> String {
    let filled = filled.min(w);
    format!(
        "{color}{}{}{}{}",
        "█".repeat(filled),
        st.dim,
        "░".repeat(w - filled),
        st.reset,
    )
}

/// Health bar (green → amber → red by fill fraction) for state signals: heat, memory,
/// storage. Truecolor gets the smooth gradient; ansi16 falls back to a solid zone
/// colour. Exactly `w` visible columns wide.
pub fn bar_grad(v: f64, lo: f64, hi: f64, w: usize, st: &Style) -> String {
    let frac = frac_of(v, lo, hi);
    let filled = (frac * w as f64).round() as usize;
    if st.truecolor {
        grad_cells(filled, w, st, health_rgb)
    } else {
        let zone = if frac < 0.6 {
            st.green
        } else if frac < 0.85 {
            st.yellow
        } else {
            st.red
        };
        solid_cells(filled, w, zone, st)
    }
}

/// Activity bar with the fire gradient (ember → flame) for CPU, power, fans. Truecolor
/// gets the gradient; ansi16 falls back to the solid fire accent. Exactly `w` columns.
pub fn fire_bar(v: f64, lo: f64, hi: f64, w: usize, st: &Style) -> String {
    let frac = frac_of(v, lo, hi);
    let filled = (frac * w as f64).round() as usize;
    if st.truecolor {
        grad_cells(filled, w, st, fire_rgb)
    } else {
        solid_cells(filled, w, st.fire, st)
    }
}

/// Place rendered blocks side by side. Each block is a list of lines; the compositor
/// pads every line to its block's widest visible line (ANSI-aware), inserts `gutter`
/// spaces between blocks, and resets colour after each cell so a coloured run can't
/// bleed into the next column. Short blocks are padded with blank cells. Returns one
/// line per row of the tallest block.
pub fn columns(blocks: &[Vec<String>], gutter: usize, st: &Style) -> Vec<String> {
    if blocks.is_empty() {
        return Vec::new();
    }
    let widths: Vec<usize> = blocks
        .iter()
        .map(|b| b.iter().map(|l| visible_len(l)).max().unwrap_or(0))
        .collect();
    let height = blocks.iter().map(|b| b.len()).max().unwrap_or(0);
    let gap = " ".repeat(gutter);
    (0..height)
        .map(|row| {
            let mut line = String::new();
            for (i, block) in blocks.iter().enumerate() {
                if i > 0 {
                    line.push_str(&gap);
                }
                let cell = block.get(row).map(String::as_str).unwrap_or("");
                line.push_str(cell);
                let vis = visible_len(cell);
                if vis < widths[i] {
                    line.push_str(&" ".repeat(widths[i] - vis));
                }
                line.push_str(st.reset);
            }
            line
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::style::visible_len;

    #[test]
    fn braille_area_has_exact_shape() {
        let st = Style::plain();
        let vals: Vec<f64> = (0..40).map(|i| i as f64).collect();
        let rows = braille_area(&vals, 0.0, 40.0, 10, 3, "", &st);
        assert_eq!(rows.len(), 3);
        for r in &rows {
            assert_eq!(visible_len(r), 10);
        }
    }

    #[test]
    fn braille_area_survives_empty_and_flat() {
        let st = Style::plain();
        assert_eq!(braille_area(&[], 0.0, 1.0, 8, 1, "", &st).len(), 1);
        let flat = braille_area(&[0.5, 0.5, 0.5], 0.0, 1.0, 4, 2, "", &st);
        assert_eq!(flat.len(), 2);
        for r in &flat {
            assert_eq!(visible_len(r), 4);
        }
    }

    #[test]
    fn bars_are_exact_width() {
        let st = Style::plain();
        for w in [1usize, 8, 17, 40] {
            assert_eq!(visible_len(&bar_grad(0.4, 0.0, 1.0, w, &st)), w);
            assert_eq!(visible_len(&fire_bar(0.9, 0.0, 1.0, w, &st)), w);
        }
        // Truecolor path keeps the same visible width despite the per-cell escapes.
        let tc = Style::color();
        assert_eq!(visible_len(&bar_grad(0.7, 0.0, 1.0, 24, &tc)), 24);
        assert_eq!(visible_len(&fire_bar(0.3, 0.0, 1.0, 24, &tc)), 24);
    }

    #[test]
    fn columns_align_by_visible_width() {
        let st = Style::plain();
        let a = vec!["aa".to_string(), "a".to_string()];
        let b = vec!["bbb".to_string()];
        let out = columns(&[a, b], 2, &st);
        assert_eq!(out.len(), 2);
        // Row 0: "aa" (2) + gutter(2) + "bbb" (3) = 7 visible columns.
        assert_eq!(visible_len(&out[0]), 7);
        // Row 1: "a" padded to 2 + gutter(2) + blank padded to 3 = 7.
        assert_eq!(visible_len(&out[1]), 7);
    }
}
