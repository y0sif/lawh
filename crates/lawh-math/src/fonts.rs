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
        }
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
