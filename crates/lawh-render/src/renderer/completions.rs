//! `:sym` completions ([`Overlay::completions`](crate::Overlay::completions)): the matching
//! symbols in a panel over the board, along the top of the status bar, as Vim's popup menu sits
//! over the command line: best first, from the top down, each row its name and then its value
//! in a column, the selected row highlighted.
//!
//! Names and values are laid out in the status bar's font, which falls back to the math font
//! for symbols the text font lacks (`crate::text::TextShaper::build_status`), and kept laid out
//! with the status bar's strings ([`crate::status`]), so a frame lays out only rows that
//! changed.

use kurbo::{Affine, Rect};
use lawh_core::engine::{SYM_ROWS, Symbol};

use super::ui::{STATUS_EDGE_PX, STATUS_PAD_PX, fill_layout};
use super::{CURSOR_BLOCK, INK, Renderer, STATUS_BAR, STATUS_EDGE};
use crate::Viewport;
use crate::overlay::{STATUS_FONT_PX, status_bar_height_px};
use crate::status::{Keep, STATUS_CACHE_ENTRIES, window};
use crate::text::LAYOUT_EM;

/// Height of one row, in logical pixels: a line of the status bar's text with room above and
/// below.
const ROW_PX: f64 = 20.0;

/// Space between the names and the values, in logical pixels.
const VALUE_GAP_PX: f64 = 16.0;

// The rows' names and values stay laid out with the status bar's own strings (the left, the
// right and the command line), so none of them is laid out again every frame.
const _: () = assert!(2 * SYM_ROWS + 3 <= STATUS_CACHE_ENTRIES);

impl Renderer {
    /// Draws up to `lawh_core::engine::SYM_ROWS` rows of `rows` (best first) in a panel just
    /// above the status bar, `selected` highlighted. When fewer fit above the status bar, the
    /// rows shown are scrolled to keep the selected one.
    pub(super) fn draw_completions(
        &mut self,
        vp: &Viewport,
        rows: &[Symbol],
        selected: Option<usize>,
    ) {
        let sf = vp.scale_factor;
        let (width, height) = (f64::from(vp.width), f64::from(vp.height));
        let bar_top = (height - f64::from(status_bar_height_px(sf))).max(0.0);
        let row_height = ROW_PX * sf;
        let shown = rows
            .len()
            .min(SYM_ROWS)
            .min(rows_that_fit(bar_top, row_height));
        if shown == 0 {
            return;
        }
        let selected = selected.filter(|selected| *selected < rows.len().min(SYM_ROWS));
        let first = selected.map_or(0, |selected| (selected + 1).saturating_sub(shown));
        let Some(rows) = rows.get(first..first + shown) else {
            return;
        };
        let slots: Vec<(usize, usize)> = rows
            .iter()
            .map(|row| {
                let name = window("", &row.name, Keep::Start).0;
                let value = window("", &row.value, Keep::Start).0;
                (
                    self.status.shape(&mut self.text, &name),
                    self.status.shape(&mut self.text, &value),
                )
            })
            .collect();
        let px_per_layout_unit = STATUS_FONT_PX * sf / f64::from(LAYOUT_EM);
        let widest = |pick: fn(&(usize, usize)) -> usize| {
            slots
                .iter()
                .filter_map(|slot| self.status.layout(pick(slot)))
                .map(|layout| f64::from(layout.width()) * px_per_layout_unit)
                .fold(0.0, f64::max)
        };
        let (names, values) = (widest(|slot| slot.0), widest(|slot| slot.1));
        let pad = STATUS_PAD_PX * sf;
        let value_x = pad + names + VALUE_GAP_PX * sf;
        let panel_width = (value_x + values + pad).min(width);
        let top = bar_top - row_height * count(shown);

        let edge = (STATUS_EDGE_PX * sf).round().max(1.0);
        self.fill_screen(Rect::new(0.0, top, panel_width, bar_top), STATUS_BAR);
        self.fill_screen(
            Rect::new(0.0, top, panel_width, (top + edge).min(bar_top)),
            STATUS_EDGE,
        );
        self.fill_screen(
            Rect::new((panel_width - edge).max(0.0), top, panel_width, bar_top),
            STATUS_EDGE,
        );
        if let Some(row) = selected.and_then(|selected| selected.checked_sub(first)) {
            let y = top + row_height * count(row);
            self.fill_screen(Rect::new(0.0, y, panel_width, y + row_height), CURSOR_BLOCK);
        }
        for (row, (name, value)) in slots.into_iter().enumerate() {
            let row_top = top + row_height * count(row);
            for (slot, x) in [(name, pad), (value, value_x)] {
                let Some(layout) = self.status.layout(slot) else {
                    continue;
                };
                // Centred in its row.
                let y =
                    row_top + (row_height - f64::from(layout.height()) * px_per_layout_unit) / 2.0;
                let transform = Affine::translate((x, y)) * Affine::scale(px_per_layout_unit);
                fill_layout(&mut self.ctx, &mut self.resources, layout, transform, INK);
            }
        }
    }
}

/// How many rows `row_height` pixels high fit in `space` pixels.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a whole number of rows from 0 to the image height, at most 65,535 pixels"
)]
fn rows_that_fit(space: f64, row_height: f64) -> usize {
    if row_height > 0.0 {
        (space / row_height).floor().max(0.0) as usize
    } else {
        0
    }
}

/// `n`, a row count of at most `SYM_ROWS`, as a float.
#[expect(
    clippy::cast_precision_loss,
    reason = "a handful of rows is exact as a float"
)]
fn count(n: usize) -> f64 {
    n as f64
}

#[cfg(test)]
mod tests {
    use kurbo::Point;
    use lawh_core::Board;

    use super::*;
    use crate::overlay::Overlay;

    fn rows(n: usize) -> Vec<Symbol> {
        (0..n)
            .map(|i| Symbol {
                name: format!("arrow.r.{i}"),
                value: "\u{2A01}".to_owned(),
            })
            .collect()
    }

    /// Draws `rows` with `selected` over an empty board through `vp`, and returns the image.
    fn draw(
        renderer: &mut Renderer,
        vp: &Viewport,
        rows: &[Symbol],
        selected: Option<usize>,
    ) -> Vec<u8> {
        let overlay = Overlay {
            command_line: Some(("sym arrow", 9)),
            completions: Some((rows, selected)),
            ..Overlay::default()
        };
        renderer.render_with(&Board::new(), vp, &overlay).to_vec()
    }

    /// The rows of column 2 of `image` (`width` wide) from `y0` up to the top, and what each is:
    /// the panel, the selected row's highlight, or paper.
    fn column(image: &[u8], width: u16, y0: u32) -> Vec<char> {
        let panel = STATUS_BAR.to_rgba8().to_u8_array();
        (0..y0)
            .rev()
            .map(|y| {
                let i = usize::try_from((y * u32::from(width) + 2) * 4).unwrap();
                let px = &image[i..i + 4];
                if px == panel {
                    'p'
                } else if u16::from(px[2]) > u16::from(px[0]) + 40 {
                    's'
                } else {
                    '.'
                }
            })
            .collect()
    }

    #[test]
    fn at_most_sym_rows_are_drawn_above_the_status_bar_the_selected_highlighted() {
        let (width, height) = (400_u16, 400_u16);
        let vp = Viewport::new(Point::ZERO, 1.0, width, height, 1.0);
        let bar_top = u32::from(height) - status_bar_height_px(1.0);
        let row = 20;
        let mut renderer = Renderer::new_trusted();
        let image = draw(&mut renderer, &vp, &rows(20), Some(2));
        let up = column(&image, width, bar_top);
        // Counted up from the status bar: the panel's rows (the second from the top selected),
        // its top edge, then paper.
        let top = SYM_ROWS * row;
        assert!(up[..top - row * 3].iter().all(|c| *c == 'p'), "{up:?}");
        assert!(
            up[top - row * 3 + 1..top - row * 2 - 1]
                .iter()
                .all(|c| *c == 's')
        );
        assert!(up[top + 1..].iter().all(|c| *c == '.'), "{up:?}");
        // Nothing selected: no highlight.
        let image = draw(&mut renderer, &vp, &rows(3), None);
        let up = column(&image, width, bar_top);
        assert!(up[..3 * row - 1].iter().all(|c| *c == 'p'), "{up:?}");
        assert!(up[3 * row + 1..].iter().all(|c| *c == '.'), "{up:?}");
        // No rows: no panel.
        let image = draw(&mut renderer, &vp, &[], None);
        assert!(column(&image, width, bar_top).iter().all(|c| *c == '.'));
    }

    #[test]
    fn a_short_window_shows_the_rows_that_fit_with_the_selected_one() {
        // Room for two rows above the status bar.
        let vp = Viewport::new(Point::ZERO, 1.0, 300, 24 + 45, 1.0);
        let mut renderer = Renderer::new_trusted();
        let image = draw(&mut renderer, &vp, &rows(8), Some(6));
        let up = column(&image, 300, 45);
        // The selected row is the lower of the two shown.
        assert!(up[1..19].iter().all(|c| *c == 's'), "{up:?}");
        assert!(up[21..39].iter().all(|c| *c == 'p'), "{up:?}");
    }

    #[test]
    fn the_same_rows_again_are_not_laid_out_again() {
        let vp = Viewport::new(Point::ZERO, 1.0, 400, 400, 1.0);
        let mut renderer = Renderer::new_trusted();
        let all = rows(SYM_ROWS);
        draw(&mut renderer, &vp, &all, Some(0));
        let built = renderer.status.built;
        // Moving the selection lays nothing out.
        for selected in 1..SYM_ROWS {
            draw(&mut renderer, &vp, &all, Some(selected));
        }
        assert_eq!(renderer.status.built, built);
    }
}
