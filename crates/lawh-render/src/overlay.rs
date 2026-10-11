//! What the window draws over the board while editing: the cursor, a Visual selection, the
//! caret in a text block and the status bar. Plain data the app builds from the engine each
//! frame and hands to [`Renderer::render_with`](crate::Renderer::render_with).
//!
//! Sizes of everything drawn for it are in logical pixels times the window's scale factor, not
//! in grid units, so it stays readable at any zoom.

use kurbo::{Point, Rect};
use lawh_core::ObjectId;

use crate::viewport::MAX_SCALE_FACTOR;

/// Height of the status bar in logical pixels: one line of [`STATUS_FONT_PX`] text with room
/// above and below.
pub(crate) const STATUS_BAR_PX: f64 = 24.0;

/// Size of the status bar's text in logical pixels.
pub(crate) const STATUS_FONT_PX: f64 = 13.0;

/// What to draw over the board for one frame.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Overlay<'a> {
    /// Where the board cursor is.
    pub cursor: OverlayCursor<'a>,
    /// The objects of a Visual selection, each highlighted. Empty outside Visual mode.
    pub selection: &'a [ObjectId],
    /// The Visual rectangle in grid units, drawn as a see-through fill and its edge.
    pub visual: Option<Rect>,
    /// The caret in a text or math block, in Insert or block-Normal mode. A math block with the
    /// caret in it is drawn as its source, in the board's text style, until the caret leaves.
    pub caret: Option<Caret<'a>>,
    /// The status line at the bottom of the window.
    pub status: StatusLine<'a>,
    /// The command line, while it is open: the text after `:` and the cursor as a byte offset
    /// in it. Shown in place of [`StatusLine::left`].
    pub command_line: Option<(&'a str, usize)>,
}

/// Where the board cursor is, for [`Overlay::cursor`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum OverlayCursor<'a> {
    /// A grid point, in grid units: drawn as a small cross.
    Grid(Point),
    /// The current object: drawn as an outline around its bounds.
    Object(&'a ObjectId),
    /// Nothing drawn.
    #[default]
    None,
}

/// The caret in a text or math block, for [`Overlay::caret`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caret<'a> {
    /// The text or math object the caret is in.
    pub id: &'a ObjectId,
    /// A byte offset in its text (or math source), on a grapheme boundary.
    pub byte: usize,
    /// Block-Normal mode: a see-through block over the grapheme at `byte`. Otherwise (Insert
    /// mode) a thin bar before it.
    pub block: bool,
}

impl<'a> From<&'a lawh_core::Cursor> for OverlayCursor<'a> {
    /// The engine's cursor (`Engine::cursor`), as drawn.
    fn from(cursor: &'a lawh_core::Cursor) -> Self {
        match cursor {
            lawh_core::Cursor::Grid(point) => Self::Grid(*point),
            lawh_core::Cursor::Object(id) => Self::Object(id),
        }
    }
}

impl<'a> From<&'a lawh_core::engine::TextCursor> for Caret<'a> {
    /// The engine's text cursor (`Engine::text_cursor`), as drawn.
    fn from(cursor: &'a lawh_core::engine::TextCursor) -> Self {
        Self {
            id: &cursor.id,
            byte: cursor.byte,
            block: cursor.block,
        }
    }
}

/// The status line, for [`Overlay::status`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatusLine<'a> {
    /// Text at the left: the mode or a message.
    pub left: &'a str,
    /// Text at the right: pending keys, the file name, the cursor's position.
    pub right: &'a str,
    /// `left` is an error message, drawn in the error colour.
    pub error: bool,
}

/// Height of the status bar in physical pixels at `scale_factor`, so the app can keep the
/// cursor out from under it. A scale factor that isn't a number above zero counts as 1, and
/// one past what a viewport may have as that most.
pub fn status_bar_height_px(scale_factor: f64) -> u32 {
    let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor.min(MAX_SCALE_FACTOR)
    } else {
        1.0
    };
    whole_px(STATUS_BAR_PX * scale)
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a status bar is at most 24 logical pixels times the largest scale factor, 32"
)]
fn whole_px(px: f64) -> u32 {
    px.round().max(0.0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_bar_grows_with_the_scale_factor_within_limits() {
        assert_eq!(status_bar_height_px(1.0), 24);
        assert_eq!(status_bar_height_px(2.0), 48);
        assert_eq!(status_bar_height_px(1.25), 30);
        for garbage in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(status_bar_height_px(garbage), 24, "{garbage}");
        }
        assert_eq!(status_bar_height_px(1e9), 24 * 32);
    }

    #[test]
    fn the_engine_cursors_convert() {
        let id = ObjectId::new("t1").unwrap();
        let grid = lawh_core::Cursor::Grid(Point::new(1.0, 2.0));
        assert_eq!(
            OverlayCursor::from(&grid),
            OverlayCursor::Grid(Point::new(1.0, 2.0))
        );
        let object = lawh_core::Cursor::Object(id.clone());
        assert_eq!(OverlayCursor::from(&object), OverlayCursor::Object(&id));
        let text = lawh_core::engine::TextCursor {
            id: id.clone(),
            byte: 3,
            block: true,
        };
        assert_eq!(
            Caret::from(&text),
            Caret {
                id: &id,
                byte: 3,
                block: true
            }
        );
    }
}
