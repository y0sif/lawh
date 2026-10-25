//! Every colour lawh draws with, in one place: the board's (paper, ink, grid, the colours of
//! math that failed or is compiling, the faint outlines of objects left out of a frame) and the
//! overlay's (cursor, selection, status bar, jump labels).
//!
//! The window and every export read the same [`Palette`], so a theme restyles both. Colours are
//! sRGB, as `vello_cpu` paints them; [`Palette::rgba8`] gives a [`Paint`]'s as bytes, for the
//! exporters that don't draw with `vello_cpu`.

use vello_cpu::color::{AlphaColor, Srgb};

use crate::scene::Paint;

/// Typst's default text colour, which math is drawn in unless it asks for another; drawn in
/// [`Palette::ink`] instead, so formulas match the board's text.
const TYPST_BLACK: [u8; 4] = [0, 0, 0, 255];

/// The colours of the board and of the overlay drawn over it while editing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    /// Board background.
    pub paper: AlphaColor<Srgb>,
    /// Shapes, text and math.
    pub ink: AlphaColor<Srgb>,
    /// Grid dots.
    pub grid: AlphaColor<Srgb>,
    /// The source of math that failed to compile, and error messages on the status line.
    pub error: AlphaColor<Srgb>,
    /// The source of math that is still compiling, as a placeholder.
    pub pending: AlphaColor<Srgb>,
    /// The outline of an object drawn in its place when the frame budget is spent.
    pub outline: AlphaColor<Srgb>,
    /// The cursor: the grid cursor's cross, the outline around the current object, and the
    /// caret in a text block.
    pub cursor: AlphaColor<Srgb>,
    /// The block caret of block-Normal mode, see-through so the character under it still shows.
    pub cursor_block: AlphaColor<Srgb>,
    /// The outline around each object of a Visual selection.
    pub selected: AlphaColor<Srgb>,
    /// The Visual rectangle: over its area, and along its edge.
    pub visual_fill: AlphaColor<Srgb>,
    pub visual_edge: AlphaColor<Srgb>,
    /// The status bar, and the line along its top. Its text is [`Self::ink`], or
    /// [`Self::error`] for an error. The `:sym` rows above it are the same, with the selected
    /// row in [`Self::cursor_block`].
    pub status_bar: AlphaColor<Srgb>,
    pub status_edge: AlphaColor<Srgb>,
    /// Jump labels are boxes of [`Self::cursor`] with the letters left to type in this colour,
    /// and the letters typed so far in [`Self::label_typed`], dimmer.
    pub label_left: AlphaColor<Srgb>,
    pub label_typed: AlphaColor<Srgb>,
}

impl Palette {
    /// The light theme: warm off-white paper and near-black ink, blue for where the cursor is,
    /// amber for what is selected, and a status bar a shade darker than the paper.
    pub const LIGHT: Palette = Palette {
        paper: AlphaColor::from_rgb8(0xFA, 0xFA, 0xF7),
        ink: AlphaColor::from_rgb8(0x1F, 0x1F, 0x1F),
        grid: AlphaColor::from_rgb8(0xD6, 0xD6, 0xD0),
        error: AlphaColor::from_rgb8(0xC0, 0x39, 0x2B),
        pending: AlphaColor::from_rgb8(0x9A, 0x9A, 0x94),
        outline: AlphaColor::from_rgb8(0xC8, 0xC8, 0xC2),
        cursor: AlphaColor::from_rgb8(0x25, 0x63, 0xEB),
        // The cursor's blue at 35 %.
        cursor_block: AlphaColor::from_rgba8(0x25, 0x63, 0xEB, 0x59),
        selected: AlphaColor::from_rgb8(0xD9, 0x77, 0x06),
        // The selection's amber at 10 % over its area, and at 70 % along its edge.
        visual_fill: AlphaColor::from_rgba8(0xD9, 0x77, 0x06, 0x1A),
        visual_edge: AlphaColor::from_rgba8(0xD9, 0x77, 0x06, 0xB3),
        status_bar: AlphaColor::from_rgb8(0xEE, 0xEE, 0xE9),
        status_edge: AlphaColor::from_rgb8(0xD6, 0xD6, 0xD0),
        // The paper colour, and the paper colour at 50 %.
        label_left: AlphaColor::from_rgb8(0xFA, 0xFA, 0xF7),
        label_typed: AlphaColor::from_rgba8(0xFA, 0xFA, 0xF7, 0x80),
    };

    /// The colour `paint` stands for.
    pub(crate) fn color(&self, paint: Paint) -> AlphaColor<Srgb> {
        match paint {
            Paint::Ink => self.ink,
            Paint::Error => self.error,
            Paint::Pending => self.pending,
            Paint::Rgba(rgba) => {
                let [r, g, b, a] = rgba;
                AlphaColor::from_rgba8(r, g, b, a)
            }
        }
    }

    /// The colour `paint` stands for as sRGB bytes, red, green, blue and alpha (not
    /// premultiplied).
    pub(crate) fn rgba8(&self, paint: Paint) -> [u8; 4] {
        match paint {
            // A colour of its own keeps its exact bytes.
            Paint::Rgba(rgba) => rgba,
            _ => self.color(paint).to_rgba8().to_u8_array(),
        }
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self::LIGHT
    }
}

/// How math drawn in the Typst colour `rgba` is painted: Typst's default black is the board's
/// ink, any other colour its own.
pub(crate) fn math_paint(rgba: [u8; 4]) -> Paint {
    if rgba == TYPST_BLACK {
        Paint::Ink
    } else {
        Paint::Rgba(rgba)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_resolve_to_the_palette_and_typst_black_is_ink() {
        let palette = Palette::LIGHT;
        assert_eq!(palette.rgba8(Paint::Ink), [0x1F, 0x1F, 0x1F, 0xFF]);
        assert_eq!(palette.rgba8(Paint::Error), [0xC0, 0x39, 0x2B, 0xFF]);
        assert_eq!(palette.rgba8(Paint::Pending), [0x9A, 0x9A, 0x94, 0xFF]);
        assert_eq!(math_paint([0, 0, 0, 255]), Paint::Ink);
        let red = [255, 0, 0, 128];
        assert_eq!(math_paint(red), Paint::Rgba(red));
        assert_eq!(palette.rgba8(Paint::Rgba(red)), red);
        assert_eq!(
            palette.color(Paint::Rgba(red)),
            AlphaColor::from_rgba8(255, 0, 0, 128)
        );
        // The labels' letters are the paper colour.
        assert_eq!(palette.label_left, palette.paper);
    }
}
