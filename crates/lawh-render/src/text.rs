//! Text layout with parley and the bundled text font, cached by string.

use std::collections::HashMap;
use std::sync::Arc;

use parley::fontique::{Blob, Collection, CollectionOptions, SourceCache};
use parley::{
    Alignment, AlignmentOptions, FontContext, FontFamily, FontFamilyName, Layout, LayoutContext,
    StyleProperty,
};

/// Em size text is laid out at, in layout units. Drawing scales layout units
/// to [`crate::TEXT_SIZE_UNITS`] per em, so layouts don't depend on zoom.
pub(crate) const LAYOUT_EM: f32 = 20.0;

/// Family used when the bundled font list is somehow empty.
const FALLBACK_FAMILY: &str = "Inter";

/// Lays out text with the bundled text font (`lawh_math::fonts()[0]`) only.
///
/// Layouts are cached by string. Strings not used during a frame are dropped
/// at its end, so the cache holds about one board's worth of text.
pub(crate) struct TextShaper {
    fonts: FontContext,
    layouts: LayoutContext<()>,
    family: &'static str,
    cache: HashMap<String, Cached>,
    frame: u64,
}

struct Cached {
    layout: Layout<()>,
    last_used: u64,
}

impl TextShaper {
    pub(crate) fn new() -> Self {
        let mut fonts = FontContext {
            collection: Collection::new(CollectionOptions {
                shared: false,
                system_fonts: false,
            }),
            source_cache: SourceCache::default(),
        };
        let asset = lawh_math::fonts().first();
        if let Some(asset) = asset {
            let families = fonts
                .collection
                .register_fonts(Blob::new(Arc::new(asset.data)), None);
            if families.is_empty() {
                tracing::warn!(family = asset.family, "bundled text font failed to load");
            }
        } else {
            tracing::warn!("no bundled text font");
        }
        Self {
            fonts,
            layouts: LayoutContext::new(),
            family: asset.map_or(FALLBACK_FAMILY, |asset| asset.family),
            cache: HashMap::new(),
            frame: 0,
        }
    }

    /// The layout of `text` in layout units, origin at the top-left of the
    /// text box, y down. Hard line breaks start new lines; nothing wraps.
    pub(crate) fn layout(&mut self, text: &str) -> Option<&Layout<()>> {
        if !self.cache.contains_key(text) {
            let layout = self.build(text);
            self.cache.insert(
                text.to_owned(),
                Cached {
                    layout,
                    last_used: self.frame,
                },
            );
        }
        let cached = self.cache.get_mut(text)?;
        cached.last_used = self.frame;
        Some(&cached.layout)
    }

    /// Drops layouts the frame didn't use and starts the next frame.
    pub(crate) fn end_frame(&mut self) {
        let frame = self.frame;
        self.cache.retain(|_, cached| cached.last_used == frame);
        self.frame = self.frame.wrapping_add(1);
    }

    fn build(&mut self, text: &str) -> Layout<()> {
        // No quantizing: positions stay fractional so text scales smoothly.
        let mut builder = self
            .layouts
            .ranged_builder(&mut self.fonts, text, 1.0, false);
        builder.push_default(StyleProperty::FontFamily(FontFamily::Single(
            FontFamilyName::named(self.family),
        )));
        builder.push_default(StyleProperty::FontSize(LAYOUT_EM));
        let mut layout = builder.build(text);
        layout.break_all_lines(None);
        layout.align(Alignment::Start, AlignmentOptions::default());
        layout
    }
}
