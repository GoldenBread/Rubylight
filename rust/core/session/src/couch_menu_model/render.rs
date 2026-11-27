//! Draws a page into the 1x panel the encoder composites (section 6.5) in
//! the Rubylight 7.0 look: a rounded obsidian `STAGE` shell with a specular
//! `EDGE_TOP` top rim and `HAIRLINE` border, a header with the `"R"` badge,
//! rounded row cards with optically centred Spleen typography, and footer
//! keycap pills. Colours come from [`super::theme_70`] only.
use super::model::{Line, Menu, Snapshot, Style, Widget, fit};
use super::theme_70::{CARD, ERROR, FOCUS, HAIRLINE, INK, MUTED, Rgb, STAGE};
use super::widgets::{self, Canvas, Tone};
use butterpollo_input_policy::couch_menu::{
    Frame,
    font::Font,
    layout::{COLUMNS, Layout},
};

/// Text columns a slider's rail takes.
const RAIL_COLUMNS: u32 = 5;

/// One page's geometry at 1x.
struct Grid {
    font: &'static Font,
    pad: [u32; 2],
    /// Width of the canvas.
    width: u32,
    /// Cell height over 32: the widgets' unit.
    u: f32,
}

impl Grid {
    /// Left edge of text column `column`.
    fn x(&self, column: u32) -> u32 {
        self.pad[0] + column * self.font.width
    }
    /// Top of text line `line`.
    fn y(&self, line: usize) -> u32 {
        self.pad[1] + line as u32 * self.font.height
    }
    /// Optical text baseline offset inside a row card so Spleen ascenders,
    /// umlauts (`ü`, `ö`, `ä`) and descenders (`g`, `p`, `y`) sit evenly inside
    /// the card.
    fn text_y(&self, y: u32) -> u32 {
        y.saturating_sub((1. * self.u).round() as u32)
    }
}

/// Draws `menu`'s current page for `snap` at `layout`'s font, centred in the
/// target.
pub fn render(menu: &Menu, snap: &Snapshot, layout: &Layout) -> Frame {
    let lines = menu.lines(snap);
    let count = lines.len() as u32;
    let rect = layout.panel(count);
    let [width, height] = layout.size_1x(count);
    let font = layout.font.font();
    let grid = Grid {
        font,
        pad: layout.padding(),
        width,
        u: font.height as f32 / 32.,
    };
    let mut canvas = Canvas::new(width, height, STAGE);
    canvas.panel_shell(12. * grid.u);
    for (index, line) in lines.iter().enumerate() {
        let y = grid.y(index);
        match line.style {
            Style::Title => header(&mut canvas, &grid, y, &line.text),
            Style::Row { focus, locked } => row(&mut canvas, &grid, y, line, focus, locked),
            Style::Info => {
                let info_y = y + (1.5 * grid.u).round() as u32;
                text(
                    &mut canvas,
                    font,
                    [grid.x(0), info_y],
                    &line.text,
                    MUTED,
                    false,
                );
            }
            Style::Note => {
                text(
                    &mut canvas,
                    font,
                    [grid.x(0), grid.text_y(y)],
                    &line.text,
                    MUTED,
                    false,
                );
            }
            Style::Hint => hint_bar(&mut canvas, &grid, y, &line.text),
            Style::Message => {
                let msg_y = y.saturating_sub((3. * grid.u).round() as u32);
                text(
                    &mut canvas,
                    font,
                    [grid.x(0), msg_y],
                    &line.text,
                    INK,
                    false,
                );
            }
            Style::Error => {
                let err_y = y.saturating_sub((3. * grid.u).round() as u32);
                text(
                    &mut canvas,
                    font,
                    [grid.x(0), err_y],
                    &line.text,
                    ERROR,
                    false,
                );
            }
        }
    }
    Frame {
        origin: rect.origin,
        size: rect.size,
        target: layout.target,
        scale: layout.scale,
        rgba: canvas.rgba.into(),
    }
}

/// Draws `chars` from (`x`, `y`) in Spleen glyphs with subtle diagonal-curve
/// anti-aliasing at 16x32 and a crisp fractional semibold stroke when `bold` is
/// set (avoiding 2-px smearing while keeping counters open).
fn text(canvas: &mut Canvas, font: &Font, [x, y]: [u32; 2], chars: &str, ink: Rgb, bold: bool) {
    let limit = canvas.width - 1;
    let smooth = font.height >= 32;
    for (column, c) in chars.chars().enumerate() {
        let left = x + column as u32 * font.width;
        if left + font.width > limit {
            break;
        }
        let lit = |gx: i32, gy: i32| -> bool {
            gx >= 0
                && gy >= 0
                && (gx as u32) < font.width
                && (gy as u32) < font.height
                && font.pixel(c, gx as u32, gy as u32)
        };
        for gy in 0..font.height {
            for gx in 0..font.width {
                let (ix, iy) = (gx as i32, gy as i32);
                if font.pixel(c, gx, gy) {
                    canvas.fill([left + gx, y + gy, 1, 1], ink);
                    if bold && !lit(ix + 1, iy) && !lit(ix + 2, iy) && left + gx + 1 < limit {
                        canvas.blend((left + gx + 1) as i64, (y + gy) as i64, ink, 0.55);
                    }
                } else if smooth {
                    let mut steps = 0u32;
                    for (dx, dy) in [(-1, -1), (1, -1), (-1, 1), (1, 1)] {
                        if lit(ix + dx, iy) && lit(ix, iy + dy) && !lit(ix + dx, iy + dy) {
                            steps += 1;
                        }
                    }
                    if steps > 0 {
                        let cov = (steps as f32 * 0.24).min(0.36);
                        canvas.blend((left + gx) as i64, (y + gy) as i64, ink, cov);
                    }
                }
            }
        }
    }
}

/// Renders the footer hint bar (`"Enter/A: select  Esc/B: back"`) with rounded
/// keycap pills around each chord/button label, falling back to plain text if
/// unformatted.
fn hint_bar(canvas: &mut Canvas, grid: &Grid, y: u32, raw: &str) {
    let parts: Vec<(&str, &str)> = raw
        .split("  ")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|seg| seg.split_once(':'))
        .map(|(k, v)| (k.trim(), v.trim()))
        .collect();
    if parts.len() < 2 {
        text(
            canvas,
            grid.font,
            [grid.x(0), grid.text_y(y)],
            raw,
            MUTED,
            false,
        );
        return;
    }
    let font = grid.font;
    let u = grid.u;
    let pad_x = (8. * u).round();
    let cap_h = (26. * u).round();
    let cap_y = y as f32 + ((font.height as f32 - cap_h) / 2.).round() + (1. * u).round();
    let text_y = y;
    let mut cursor = grid.x(0) as f32;
    let max_right = (canvas.width - grid.pad[0] / 2) as f32;
    for (key, action) in parts {
        let key_w = (key.chars().count() as u32 * font.width) as f32;
        let pill_w = key_w + 2. * pad_x;
        let act_w = (action.chars().count() as u32 * font.width) as f32;
        if cursor + pill_w + (8. * u).round() + act_w > max_right {
            break;
        }
        widgets::keycap(canvas, [cursor, cap_y, pill_w, cap_h], u);
        text(
            canvas,
            font,
            [(cursor + pad_x).round() as u32, text_y],
            key,
            INK,
            false,
        );
        let act_x = (cursor + pill_w + (8. * u).round()).round() as u32;
        text(canvas, font, [act_x, text_y], action, MUTED, false);
        cursor = act_x as f32 + act_w + (18. * u).round();
    }
}

/// The header strip: the `"R"` badge, the title in semibold INK, and a hairline
/// rule under the strip across the container.
fn header(canvas: &mut Canvas, grid: &Grid, y: u32, title: &str) {
    let cell = grid.font.height as f32;
    let size = (cell * 0.75).round();
    let badge_y = y as f32 + ((cell - size) / 2.).round();
    widgets::badge(canvas, [grid.x(0) as f32, badge_y], size);
    let title = fit(title, COLUMNS as usize - 2);
    let title_y = y.saturating_sub((1.5 * grid.u).round() as u32);
    text(canvas, grid.font, [grid.x(2), title_y], &title, INK, true);
    let rule = y + grid.font.height - 1;
    canvas.fill([1, rule, grid.width - 2, 1], HAIRLINE);
}

/// A row card inset in its line, the label left and the value right.
fn row(canvas: &mut Canvas, grid: &Grid, y: u32, line: &Line, focus: bool, locked: bool) {
    let font = grid.font;
    let u = grid.u;
    let tone = match (focus, locked) {
        (_, true) => Tone::Locked,
        (true, false) => Tone::Focused,
        (false, false) => Tone::Normal,
    };
    let gap = 2. * u;
    let card = [
        (grid.pad[0] / 2) as f32,
        y as f32 + gap,
        (grid.width - grid.pad[0]) as f32,
        font.height as f32 - 2. * gap,
    ];
    let radius = 6. * u;
    if focus && locked {
        // A locked row cannot take the focus fill: a thin ruby outline and a
        // marker at the card's left edge show the focus, the text stays muted.
        let ring = (2. * u).max(1.);
        canvas.rounded(card, radius, FOCUS);
        let [x, y, w, h] = card;
        let inner = [x + ring, y + ring, w - 2. * ring, h - 2. * ring];
        canvas.rounded(inner, radius - ring, CARD);
        let inset = 5. * u;
        let marker = [x + ring + u, y + inset, 3. * u, h - 2. * inset];
        canvas.rounded(marker, 1.5 * u, FOCUS);
    } else {
        let fill = if tone == Tone::Focused { FOCUS } else { CARD };
        canvas.rounded(card, radius, fill);
    }
    let (label_ink, value_ink) = widgets::inks(tone);
    let chars: Vec<char> = line.text.chars().collect();
    let split = chars.len().saturating_sub(line.value);
    let label: String = chars[..split].iter().collect::<String>().trim_end().into();
    let value: String = chars[split..].iter().collect();
    let right = grid.x(COLUMNS);
    let cy = y as f32 + font.height as f32 / 2.;
    let text_y = grid.text_y(y);
    // The first column the label may not reach.
    let label_end = match line.widget {
        _ if line.value == 0 => COLUMNS + 1,
        Widget::Switch(on) => {
            let left = widgets::switch(canvas, right as f32, cy, u, on, tone);
            ((left - grid.x(0) as f32) / font.width as f32).floor() as u32
        }
        Widget::Slider {
            value: at,
            min,
            max,
        } => {
            let shown = unwrap_chevrons(&value).unwrap_or(&value);
            let columns = shown.chars().count() as u32;
            text(
                canvas,
                font,
                [grid.x(COLUMNS - columns), text_y],
                shown,
                value_ink,
                false,
            );
            let start = COLUMNS.saturating_sub(columns + 1 + RAIL_COLUMNS);
            let span = (max - min).max(1) as f32;
            let fraction = (at - min) as f32 / span;
            let rail = [grid.x(start) as f32, (RAIL_COLUMNS * font.width) as f32];
            widgets::slider(canvas, rail, cy, u, fraction, tone);
            start
        }
        Widget::Choice if unwrap_chevrons(&value).is_some() => {
            let n = line.value as u32;
            let middle = |column: u32| (grid.x(column) + font.width / 2) as f32;
            widgets::chevron(canvas, [middle(COLUMNS - n), cy], u, false, value_ink);
            widgets::chevron(canvas, [middle(COLUMNS - 1), cy], u, true, value_ink);
            let inner = unwrap_chevrons(&value).unwrap_or_default();
            text(
                canvas,
                font,
                [grid.x(COLUMNS - n + 2), text_y],
                inner,
                value_ink,
                false,
            );
            COLUMNS - n
        }
        Widget::Link => {
            let middle = (grid.x(COLUMNS - 1) + font.width / 2) as f32;
            widgets::chevron(canvas, [middle, cy], u, true, value_ink);
            COLUMNS - 1
        }
        _ => {
            let n = line.value as u32;
            text(
                canvas,
                font,
                [grid.x(COLUMNS - n), text_y],
                &value,
                value_ink,
                false,
            );
            COLUMNS - n
        }
    };
    let room = label_end.saturating_sub(1) as usize;
    let label = if label.chars().count() > room && line.value > 0 {
        fit(&label, room)
    } else {
        label
    };
    let label_x = if focus && locked {
        grid.x(0) + (6. * u).round() as u32
    } else {
        grid.x(0)
    };
    text(canvas, font, [label_x, text_y], &label, label_ink, false);
}

/// `value` without the model's `< ` and ` >`, if it has them.
fn unwrap_chevrons(value: &str) -> Option<&str> {
    value.strip_prefix("< ")?.strip_suffix(" >")
}

#[cfg(test)]
mod tests;
