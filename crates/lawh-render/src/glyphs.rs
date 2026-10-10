//! How complex each glyph of the bundled fonts is, for the frame budget.
//!
//! Glyphs differ a lot: Inter's `x` is 17 outline elements about 3.7 ems around, while some of
//! Libertinus Math's large operators and private glyphs have over a thousand elements or reach
//! 31 ems around. What a glyph costs to draw follows its outline, so the budget looks each glyph
//! up here instead of assuming a typical one. Outlines are read from the font once per glyph, on
//! first use, into a table as long as the font's glyph count.

use lawh_math::fonts;
use ttf_parser::{Face, GlyphId, OutlineBuilder};

/// The outline of one glyph, in ems.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct GlyphStats {
    /// Straight segments in the outline.
    pub(crate) lines: f64,
    /// Curved segments in the outline.
    pub(crate) curves: f64,
    /// Outline length measured along x plus along y, control points included: what the
    /// rasterizer's tiles follow.
    pub(crate) outline_em: f64,
    /// Area of the outline's bounding box.
    pub(crate) area_em: f64,
}

impl GlyphStats {
    pub(crate) fn elements(&self) -> f64 {
        self.lines + self.curves
    }
}

/// Glyph outline statistics for [`lawh_math::fonts`], by font index and glyph id.
pub(crate) struct GlyphTable {
    fonts: Vec<FontTable>,
}

struct FontTable {
    face: Option<Face<'static>>,
    units_per_em: f64,
    /// One slot per glyph id the font has, filled on first use.
    stats: Vec<Option<GlyphStats>>,
}

impl GlyphTable {
    pub(crate) fn new() -> Self {
        let fonts = fonts()
            .iter()
            .map(|asset| {
                let face = Face::parse(asset.data, asset.index)
                    .inspect_err(|err| {
                        tracing::warn!(family = asset.family, %err, "bundled font failed to parse");
                    })
                    .ok();
                let units_per_em = face
                    .as_ref()
                    .map_or(1.0, |face| f64::from(face.units_per_em().max(1)));
                let count = face.as_ref().map_or(0, Face::number_of_glyphs);
                FontTable {
                    face,
                    units_per_em,
                    stats: vec![None; usize::from(count)],
                }
            })
            .collect();
        Self { fonts }
    }

    /// The outline statistics of glyph `id` of font `font` (an index into
    /// [`lawh_math::fonts`]); empty for a glyph the font doesn't have.
    pub(crate) fn stats(&mut self, font: usize, id: u32) -> GlyphStats {
        let Some(table) = self.fonts.get_mut(font) else {
            return GlyphStats::default();
        };
        let Some(slot) = usize::try_from(id)
            .ok()
            .and_then(|id| table.stats.get_mut(id))
        else {
            return GlyphStats::default();
        };
        if let Some(stats) = slot {
            return *stats;
        }
        let stats = match (&table.face, u16::try_from(id)) {
            (Some(face), Ok(id)) => measure(face, GlyphId(id), table.units_per_em),
            _ => GlyphStats::default(),
        };
        *slot = Some(stats);
        stats
    }

    /// The glyph of each font that scores highest `by`, as `(font, id)`, for calibration.
    #[cfg(test)]
    pub(crate) fn most_complex(&mut self, by: impl Fn(&GlyphStats) -> f64) -> Vec<(usize, u32)> {
        (0..self.fonts.len())
            .map(|font| {
                let count = u32::try_from(self.fonts[font].stats.len()).unwrap();
                let best = (0..count)
                    .max_by(|a, b| by(&self.stats(font, *a)).total_cmp(&by(&self.stats(font, *b))))
                    .unwrap_or(0);
                (font, best)
            })
            .collect()
    }
}

/// Reads one glyph's outline.
fn measure(face: &Face<'_>, id: GlyphId, units_per_em: f64) -> GlyphStats {
    let mut walk = Walk::default();
    let Some(bbox) = face.outline_glyph(id, &mut walk) else {
        return GlyphStats::default();
    };
    let width = f64::from(bbox.x_max) - f64::from(bbox.x_min);
    let height = f64::from(bbox.y_max) - f64::from(bbox.y_min);
    GlyphStats {
        lines: walk.lines,
        curves: walk.curves,
        outline_em: walk.outline / units_per_em,
        area_em: (width * height).abs() / (units_per_em * units_per_em),
    }
}

/// Totals an outline as the font draws it.
#[derive(Default)]
struct Walk {
    lines: f64,
    curves: f64,
    outline: f64,
    at: (f32, f32),
    start: (f32, f32),
}

impl Walk {
    fn step(&mut self, to: (f32, f32)) {
        self.outline += f64::from((to.0 - self.at.0).abs()) + f64::from((to.1 - self.at.1).abs());
        self.at = to;
    }
}

impl OutlineBuilder for Walk {
    fn move_to(&mut self, x: f32, y: f32) {
        self.at = (x, y);
        self.start = (x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.lines += 1.0;
        self.step((x, y));
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.curves += 1.0;
        self.step((x1, y1));
        self.step((x, y));
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.curves += 1.0;
        self.step((x1, y1));
        self.step((x2, y2));
        self.step((x, y));
    }

    fn close(&mut self) {
        self.lines += 1.0;
        let start = self.start;
        self.step(start);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyphs_are_measured_from_the_fonts() {
        let mut table = GlyphTable::new();
        let face = Face::parse(fonts()[0].data, 0).unwrap();
        let x = face.glyph_index('x').unwrap();
        let stats = table.stats(0, u32::from(x.0));
        // Inter's x: a closed outline a few ems around, a fraction of an em in area.
        assert!(stats.elements() > 4.0, "{stats:?}");
        assert!(
            stats.outline_em > 2.0 && stats.outline_em < 6.0,
            "{stats:?}"
        );
        assert!(stats.area_em > 0.1 && stats.area_em < 0.5, "{stats:?}");
        // Cached: the same answer again.
        assert_eq!(table.stats(0, u32::from(x.0)), stats);
    }

    #[test]
    fn unknown_glyphs_and_fonts_are_empty() {
        let mut table = GlyphTable::new();
        assert_eq!(table.stats(0, u32::MAX), GlyphStats::default());
        assert_eq!(table.stats(0, 70_000), GlyphStats::default());
        assert_eq!(table.stats(99, 1), GlyphStats::default());
    }
}
