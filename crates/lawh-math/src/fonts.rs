//! Bundled fonts. lawh never loads system fonts, so math and text look the
//! same everywhere and a board can't probe what is installed.

use std::fmt;
use std::sync::OnceLock;

use typst::foundations::Bytes;
use typst::text::Font;

/// A font file embedded in the binary.
#[derive(Clone, Copy)]
pub struct FontAsset {
    /// Family name as the font reports it, which is also how Typst selects it.
    pub family: &'static str,
    /// The raw font file (TTF or OTF).
    pub data: &'static [u8],
    /// Face index inside `data` (0 for single-face files).
    pub index: u32,
}

impl fmt::Debug for FontAsset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FontAsset")
            .field("family", &self.family)
            .field("bytes", &self.data.len())
            .field("index", &self.index)
            .finish()
    }
}

/// Typst family name of the text font (index 0 in [`fonts`]).
pub(crate) const TEXT_FAMILY: &str = "Inter";
/// Typst family name of the math font (index 1 in [`fonts`]).
pub(crate) const MATH_FAMILY: &str = "Libertinus Math";

static FONTS: [FontAsset; 2] = [
    FontAsset {
        family: TEXT_FAMILY,
        data: include_bytes!("../../../assets/fonts/Inter-Regular.ttf"),
        index: 0,
    },
    FontAsset {
        family: MATH_FAMILY,
        data: include_bytes!("../../../assets/fonts/LibertinusMath-Regular.otf"),
        index: 0,
    },
];

/// The bundled fonts: index 0 is Inter (text), index 1 is Libertinus Math.
/// [`crate::MathItem::Glyphs::font`] indexes this slice.
pub fn fonts() -> &'static [FontAsset] {
    &FONTS
}

/// The bundled fonts parsed for Typst, parsed once per process.
pub(crate) fn typst_fonts() -> &'static [Font] {
    static PARSED: OnceLock<Vec<Font>> = OnceLock::new();
    PARSED.get_or_init(|| {
        fonts()
            .iter()
            .filter_map(|asset| {
                let font = Font::new(Bytes::new(asset.data), asset.index);
                if font.is_none() {
                    tracing::warn!(family = asset.family, "bundled font failed to parse");
                }
                font
            })
            .collect()
    })
}

/// Index into [`fonts`] of the asset Typst's `font` was loaded from.
///
/// Fonts are matched by the address of their embedded bytes, so the check is
/// exact and cheap. Every font Typst sees comes from [`typst_fonts`], which
/// wraps these statics without copying.
pub(crate) fn asset_index(font: &Font) -> Option<usize> {
    let data = font.data().as_ptr();
    fonts()
        .iter()
        .position(|asset| std::ptr::eq(asset.data.as_ptr(), data) && asset.index == font.index())
}

/// The bundled fonts parsed by ttf-parser, by [`fonts`] index, parsed once
/// per process; `None` for a font that fails to parse.
fn faces() -> &'static [Option<ttf_parser::Face<'static>>] {
    static FACES: OnceLock<Vec<Option<ttf_parser::Face<'static>>>> = OnceLock::new();
    FACES.get_or_init(|| {
        fonts()
            .iter()
            .map(|asset| ttf_parser::Face::parse(asset.data, asset.index).ok())
            .collect()
    })
}

fn face(index: usize) -> Option<&'static ttf_parser::Face<'static>> {
    faces().get(index)?.as_ref()
}

/// Number of glyphs in the bundled font at `index` into [`fonts`], or `None`
/// if there is no such font.
pub(crate) fn glyph_count(index: usize) -> Option<u16> {
    face(index).map(ttf_parser::Face::number_of_glyphs)
}

/// The bounds of glyph `id`'s outline in the bundled font at `index` into
/// [`fonts`], in ems, y up, from the glyph origin. `None` for a glyph with no
/// outline (a space), or no such font or glyph.
///
/// For the CFF math font this outlines the glyph, so callers look each glyph
/// up once.
pub(crate) fn glyph_box(index: usize, id: u32) -> Option<kurbo::Rect> {
    let face = face(index)?;
    let bounds = face.glyph_bounding_box(ttf_parser::GlyphId(u16::try_from(id).ok()?))?;
    let em = f64::from(face.units_per_em());
    Some(kurbo::Rect::new(
        f64::from(bounds.x_min) / em,
        f64::from(bounds.y_min) / em,
        f64::from(bounds.x_max) / em,
        f64::from(bounds.y_max) / em,
    ))
}

#[cfg(test)]
mod tests {
    use typst::text::FontFlags;

    use super::*;

    #[test]
    fn every_bundled_font_parses_with_its_declared_family() {
        assert_eq!(typst_fonts().len(), fonts().len());
        for (i, (asset, font)) in fonts().iter().zip(typst_fonts()).enumerate() {
            assert_eq!(font.info().family, asset.family);
            assert_eq!(asset_index(font), Some(i));
            assert!(glyph_count(i).is_some_and(|n| n > 0));
        }
        assert_eq!(glyph_count(fonts().len()), None);
    }

    #[test]
    fn glyph_boxes_are_in_ems_and_spaces_have_none() {
        for i in 0..fonts().len() {
            let id = |c: char| u32::from(face(i).unwrap().glyph_index(c).unwrap().0);
            // Capital H: on the baseline, under an em tall.
            let h = glyph_box(i, id('H')).unwrap();
            assert!(h.y0.abs() < 0.02 && (0.5..1.0).contains(&h.y1), "{h:?}");
            assert!(h.x0 >= 0.0 && h.x1 < 1.0, "{h:?}");
            assert_eq!(glyph_box(i, id(' ')), None);
        }
        assert_eq!(glyph_box(fonts().len(), 1), None);
        assert_eq!(glyph_box(0, u32::from(u16::MAX) + 1), None);
    }

    #[test]
    fn text_font_is_static_and_math_font_has_a_math_table() {
        let [text, math] = typst_fonts() else {
            panic!("expected two bundled fonts");
        };
        assert!(!text.info().flags.contains(FontFlags::VARIABLE));
        assert!(math.info().flags.contains(FontFlags::MATH));
    }
}
