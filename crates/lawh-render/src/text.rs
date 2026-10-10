//! Text layout with parley and the bundled text font, cached by string.
//!
//! A frame hashes each text once ([`TextShaper::key`]) and looks it up by that hash from then
//! on, comparing it with the text cached under it. For every text it has seen, the cache keeps
//! what the frame budget needs without laying the text out or walking its layout: how many
//! characters and lines it has ([`Facts`]), its size once laid out (kept after the layout itself
//! is dropped), and its layout with a [`Plan`] of where its glyphs are and what they cost to draw.
//!
//! The cache is bounded by approximate bytes ([`MAX_TEXT_CACHE_BYTES`], room for every text of
//! a board at lawh-core's text caps): the least recently used layouts go first, then the least
//! recently used entries. Nothing is dropped at the end of a frame, so a frame that draws nothing
//! doesn't cost the next one its layouts.

use std::collections::HashMap;
use std::hash::{BuildHasher, BuildHasherDefault, Hasher, RandomState};
use std::ops::Range;
use std::sync::Arc;

use lawh_core::board::{MAX_BOARD_TEXT_CHARS, MAX_OBJECTS};
use parley::fontique::{Blob, Collection, CollectionOptions, SourceCache};
use parley::{
    Alignment, AlignmentOptions, FontContext, FontFamily, FontFamilyName, Layout, LayoutContext,
    PositionedLayoutItem, StyleProperty,
};

use crate::budget::GlyphSums;
use crate::glyphs::GlyphTable;

/// Em size text is laid out at, in layout units. Drawing scales layout units
/// to [`crate::TEXT_SIZE_UNITS`] per em, so layouts don't depend on zoom.
pub(crate) const LAYOUT_EM: f32 = 20.0;

/// Family used when the bundled font list is somehow empty.
const FALLBACK_FAMILY: &str = "Inter";

/// Most glyphs in one [`Chunk`] of a [`Plan`].
const CHUNK_GLYPHS: u32 = 64;
/// Most a [`Chunk`]'s glyph origins may spread across, in ems: a frame counts a chunk drawn
/// whole when any of it may be, so at the closest zoom (an em of 400 to 1,000 pixels) wide
/// chunks would count many glyphs off screen.
const CHUNK_EMS: f32 = 2.0;

/// Bytes a cache entry takes besides its text and its layout: the map slot and its fields.
const ENTRY_BYTES: usize = 160;
/// Bytes a parley layout and its [`Plan`] take, estimated: for any layout, per character (line
/// breaks aside) and per line. Measured with `calibrate_text_memory` (resident memory after
/// laying out a board of the costliest texts lawh-core's caps allow, each kind in a new
/// process, Linux `x86_64`, 2026-10-11) and rounded up.
const LAYOUT_BYTES: usize = 4096;
const LAYOUT_BYTES_PER_CHAR: usize = 448;
const LAYOUT_BYTES_PER_LINE: usize = 288;

/// The most bytes one character takes in UTF-8.
const MAX_UTF8_BYTES: usize = 4;

/// What every text of a board at lawh-core's text caps takes in the cache, laid out: at most
/// [`MAX_OBJECTS`] texts of [`MAX_BOARD_TEXT_CHARS`] characters together, each character a line
/// break or not, and a first line for each text.
const FULL_BOARD_BYTES: usize = MAX_OBJECTS * (ENTRY_BYTES + LAYOUT_BYTES + LAYOUT_BYTES_PER_LINE)
    + MAX_BOARD_TEXT_CHARS * (MAX_UTF8_BYTES + max(LAYOUT_BYTES_PER_CHAR, LAYOUT_BYTES_PER_LINE));

const fn max(a: usize, b: usize) -> usize {
    if a > b { a } else { b }
}

/// Most memory one renderer's text cache may take, in bytes, estimated from the characters and
/// lines of the texts it holds: room for every text of a board at lawh-core's text caps
/// (`lawh_core::board::MAX_BOARD_TEXT_CHARS`, `MAX_OBJECTS`), laid out, and half as much again
/// for texts it drew before (earlier versions of an edited text, the sources of formulas drawn
/// while they compiled). About 30 MB; an everyday board takes well under a megabyte.
pub const MAX_TEXT_CACHE_BYTES: usize = FULL_BOARD_BYTES / 2 * 3;

/// A text's hash, from [`TextShaper::key`]. The cache looks texts up by it and compares the
/// text cached under it, so a frame hashes each text once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextKey(pub(crate) u64);

/// Hashes keys that are hashes already ([`TextKey`]) as themselves.
#[derive(Default)]
pub(crate) struct KeyHasher(u64);

impl Hasher for KeyHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = self.0.rotate_left(8) ^ u64::from(*byte);
        }
    }

    fn write_u64(&mut self, n: u64) {
        self.0 = n;
    }
}

/// A map keyed by [`TextKey`] hashes.
pub(crate) type KeyMap<V> = HashMap<u64, V, BuildHasherDefault<KeyHasher>>;

/// What a text is made of, counted without laying it out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Facts {
    pub(crate) chars: usize,
    /// Characters outside ASCII, which cost more to lay out.
    pub(crate) non_ascii: usize,
    /// Places where the text moves from one Unicode block to another, Latin and combining
    /// marks counted as one block ([`block`]): a cheap stand-in for where its script or
    /// direction changes, which is what makes laying text out costly.
    pub(crate) changes: usize,
    /// Lines, as lawh-core counts them (`lawh_core::board::MAX_TEXT_LINES`).
    pub(crate) lines: usize,
    /// Characters in the longest line.
    pub(crate) longest_line: usize,
}

impl Facts {
    fn of(text: &str) -> Self {
        let mut facts = Self {
            chars: 0,
            non_ascii: 0,
            changes: 0,
            lines: 1,
            longest_line: 0,
        };
        let mut line = 0;
        let mut after_cr = false;
        let mut last_block = 0;
        for c in text.chars() {
            facts.chars += 1;
            if !c.is_ascii() {
                facts.non_ascii += 1;
            }
            let block = block(c);
            if block != last_block {
                facts.changes += 1;
                last_block = block;
            }
            match c {
                // CRLF breaks a line once.
                '\n' if after_cr => {}
                '\n' | '\r' | '\u{B}' | '\u{C}' | '\u{85}' | '\u{2028}' | '\u{2029}' => {
                    facts.longest_line = facts.longest_line.max(line);
                    line = 0;
                    facts.lines += 1;
                }
                _ => line += 1,
            }
            after_cr = c == '\r';
        }
        facts.longest_line = facts.longest_line.max(line);
        facts
    }

    /// The estimated bytes of this text's layout and plan. A line break counts as a line, not
    /// as a character.
    fn layout_bytes(&self) -> usize {
        let breaks = self.lines - 1;
        LAYOUT_BYTES
            + self.chars.saturating_sub(breaks) * LAYOUT_BYTES_PER_CHAR
            + self.lines * LAYOUT_BYTES_PER_LINE
    }
}

/// The Unicode block of 128 code points `c` is in, with Latin (and ASCII), combining marks and
/// the other code points below Greek as block 0.
fn block(c: char) -> u32 {
    let c = u32::from(c);
    if c < 0x370 { 0 } else { c >> 7 }
}

/// What the cache knows about a text, from [`TextShaper::known`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Known {
    pub(crate) facts: Facts,
    /// Width and height of its layout box in layout units, once it has been laid out.
    pub(crate) size: Option<(f32, f32)>,
    /// Its layout is cached.
    pub(crate) shaped: bool,
}

/// A text's layout and its plan.
pub(crate) struct Shaped {
    pub(crate) layout: Layout<()>,
    pub(crate) plan: Plan,
}

/// Where a layout's glyphs are and what they cost to draw, gathered once when it is laid out:
/// a frame estimates drawing any part of the text from this, without walking the layout (parley
/// hands out a line's glyph runs only by walking every glyph in them).
pub(crate) struct Plan {
    lines: Vec<PlanLine>,
    /// Every line's chunks, line after line.
    chunks: Vec<Chunk>,
    /// Glyphs and glyph runs in all.
    glyphs: u32,
    runs: u32,
}

#[derive(Debug, Clone, Copy)]
struct PlanLine {
    baseline: f32,
    /// Index of the line's first chunk.
    first_chunk: u32,
    /// Glyphs and glyph runs before this line.
    glyphs_before: u32,
    runs_before: u32,
}

/// Up to [`CHUNK_GLYPHS`] glyphs that follow each other on one line, in visual order, whose
/// origins are at most [`CHUNK_EMS`] apart.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Chunk {
    /// The smallest and largest x of the glyphs' origins, in layout units.
    pub(crate) x0: f32,
    pub(crate) x1: f32,
    /// The bounds of the glyphs' outlines in layout units, y down: `[x0, y0, x1, y1]`.
    pub(crate) ink: [f32; 4],
    /// The index of the chunk's line in the layout.
    pub(crate) line: u32,
    /// Glyph runs with glyphs in the chunk. A run across chunks counts in each.
    pub(crate) runs: u32,
    pub(crate) glyphs: GlyphSums,
}

impl Plan {
    fn new(layout: &Layout<()>, table: &mut GlyphTable) -> Self {
        let width = CHUNK_EMS * LAYOUT_EM;
        let mut lines = Vec::with_capacity(layout.len());
        let mut chunks = Vec::new();
        let (mut glyphs, mut runs) = (0, 0);
        for (index, line) in layout.lines().enumerate() {
            lines.push(PlanLine {
                baseline: line.metrics().baseline,
                first_chunk: to_u32(chunks.len()),
                glyphs_before: glyphs,
                runs_before: runs,
            });
            // The chunk being filled, its glyph count and the last run it counted.
            let mut open: Option<(Chunk, u32, u32)> = None;
            let mut run = 0;
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };
                run += 1;
                for glyph in glyph_run.positioned_glyphs() {
                    if let Some((chunk, count, _)) = open
                        && (count >= CHUNK_GLYPHS
                            || glyph.x.max(chunk.x1) - glyph.x.min(chunk.x0) > width)
                    {
                        chunks.push(chunk);
                        open = None;
                    }
                    // Text is laid out with the bundled text font only, `fonts()[0]`.
                    let stats = table.stats(0, glyph.id);
                    let [left, bottom, right, top] = stats.ink_em.map(|v| v * LAYOUT_EM);
                    let ink = [
                        glyph.x + left,
                        glyph.y - top,
                        glyph.x + right,
                        glyph.y - bottom,
                    ];
                    let (chunk, count, counted) = open.get_or_insert((
                        Chunk {
                            x0: glyph.x,
                            x1: glyph.x,
                            ink,
                            line: to_u32(index),
                            runs: 0,
                            glyphs: GlyphSums::default(),
                        },
                        0,
                        0,
                    ));
                    chunk.x0 = chunk.x0.min(glyph.x);
                    chunk.x1 = chunk.x1.max(glyph.x);
                    chunk.ink = [
                        chunk.ink[0].min(ink[0]),
                        chunk.ink[1].min(ink[1]),
                        chunk.ink[2].max(ink[2]),
                        chunk.ink[3].max(ink[3]),
                    ];
                    if *counted != run {
                        chunk.runs += 1;
                        *counted = run;
                    }
                    chunk.glyphs.add(stats);
                    *count += 1;
                    glyphs += 1;
                }
            }
            chunks.extend(open.map(|(chunk, ..)| chunk));
            runs += run;
        }
        chunks.shrink_to_fit();
        Self {
            lines,
            chunks,
            glyphs,
            runs,
        }
    }

    /// The lines whose baseline `y` maps into `y0..=y1`, for a `y` that grows with the baseline.
    pub(crate) fn lines_within(&self, y: impl Fn(f32) -> f64, y0: f64, y1: f64) -> Range<usize> {
        let first = self.lines.partition_point(|line| y(line.baseline) < y0);
        let end = self.lines.partition_point(|line| y(line.baseline) <= y1);
        first..end.max(first)
    }

    /// The chunks of `lines`.
    pub(crate) fn chunks(&self, lines: Range<usize>) -> &[Chunk] {
        let (start, end) = (self.before(lines.start).0, self.before(lines.end).0);
        self.chunks.get(start..end).unwrap_or_default()
    }

    /// The glyphs and glyph runs on line `line`: what walking it goes over.
    pub(crate) fn on_line(&self, line: usize) -> (f64, f64) {
        let (_, glyphs_from, runs_from) = self.before(line);
        let (_, glyphs_to, runs_to) = self.before(line + 1);
        (
            f64::from(glyphs_to.saturating_sub(glyphs_from)),
            f64::from(runs_to.saturating_sub(runs_from)),
        )
    }

    /// Chunks, glyphs and glyph runs before line `line` (all of them past the last).
    fn before(&self, line: usize) -> (usize, u32, u32) {
        self.lines
            .get(line)
            .map_or((self.chunks.len(), self.glyphs, self.runs), |line| {
                (
                    line.first_chunk as usize,
                    line.glyphs_before,
                    line.runs_before,
                )
            })
    }
}

struct Entry {
    text: Box<str>,
    facts: Facts,
    size: Option<(f32, f32)>,
    shaped: Option<Box<Shaped>>,
    /// When it was last looked at, in [`TextShaper::tick`]s.
    last_used: u64,
}

impl Entry {
    fn new(text: &str) -> Self {
        Self {
            text: text.into(),
            facts: Facts::of(text),
            size: None,
            shaped: None,
            last_used: 0,
        }
    }

    /// The estimated bytes it takes.
    fn bytes(&self) -> usize {
        ENTRY_BYTES
            + self.text.len()
            + self
                .shaped
                .as_ref()
                .map_or(0, |_| self.facts.layout_bytes())
    }
}

/// Lays out text with the bundled text font (`lawh_math::fonts()[0]`) only,
/// and caches what it laid out (see the module docs).
pub(crate) struct TextShaper {
    fonts: FontContext,
    layouts: LayoutContext<()>,
    family: &'static str,
    hasher: RandomState,
    cache: KeyMap<Entry>,
    /// Estimated bytes of everything in `cache`.
    bytes: usize,
    limit: usize,
    /// Counts lookups, for least-recently-used order.
    tick: u64,
    /// The height of a line and the widest advance of the font, in layout units: the size of a
    /// text not laid out yet is estimated from them.
    line_height: f32,
    max_advance: f32,
    /// Texts hashed and laid out since this shaper was made, for logs and tests.
    pub(crate) hashed: u64,
    pub(crate) built: u64,
}

impl TextShaper {
    pub(crate) fn new() -> Self {
        Self::with_limit(MAX_TEXT_CACHE_BYTES)
    }

    /// A shaper whose cache holds at most `limit` estimated bytes.
    pub(crate) fn with_limit(limit: usize) -> Self {
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
        let max_advance = asset
            .and_then(|asset| ttf_parser::Face::parse(asset.data, asset.index).ok())
            .map_or(LAYOUT_EM * 2.0, |face| {
                let widest = (0..face.number_of_glyphs())
                    .filter_map(|id| face.glyph_hor_advance(ttf_parser::GlyphId(id)))
                    .max()
                    .unwrap_or(0);
                f32::from(widest) / f32::from(face.units_per_em().max(1)) * LAYOUT_EM
            });
        let mut shaper = Self {
            fonts,
            layouts: LayoutContext::new(),
            family: asset.map_or(FALLBACK_FAMILY, |asset| asset.family),
            hasher: RandomState::new(),
            cache: KeyMap::default(),
            bytes: 0,
            limit,
            tick: 0,
            line_height: LAYOUT_EM * 1.5,
            max_advance,
            hashed: 0,
            built: 0,
        };
        let line = shaper.build("x").height();
        if line.is_finite() && line > 0.0 {
            shaper.line_height = line;
        }
        shaper
    }

    /// The key to look `text` up by: its hash.
    pub(crate) fn key(&mut self, text: &str) -> TextKey {
        self.hashed += 1;
        TextKey(self.hasher.hash_one(text))
    }

    /// What the cache knows about `text`, counting it on first sight. Never lays it out.
    pub(crate) fn known(&mut self, key: TextKey, text: &str) -> Known {
        let entry = self.entry(key, text);
        Known {
            facts: entry.facts,
            size: entry.size,
            shaped: entry.shaped.is_some(),
        }
    }

    /// The size of `text`'s layout box in layout units: of its layout if it has been laid out
    /// before, or else at most the widest glyph per character of its longest line by its lines.
    pub(crate) fn size(&self, known: &Known) -> (f32, f32) {
        known.size.unwrap_or_else(|| {
            let facts = &known.facts;
            (
                facts.longest_line as f32 * self.max_advance,
                facts.lines as f32 * self.line_height,
            )
        })
    }

    /// Lays `text` out unless its layout is cached, and returns the size of its layout box in
    /// layout units.
    pub(crate) fn lay_out(
        &mut self,
        key: TextKey,
        text: &str,
        glyphs: &mut GlyphTable,
    ) -> (f32, f32) {
        let entry = self.entry(key, text);
        if let (Some(size), Some(_)) = (entry.size, &entry.shaped) {
            return size;
        }
        let layout = self.build(text);
        let plan = Plan::new(&layout, glyphs);
        let size = (layout.width(), layout.height());
        self.built += 1;
        if let Some(entry) = self.cache.get_mut(&key.0) {
            entry.size = Some(size);
            if entry
                .shaped
                .replace(Box::new(Shaped { layout, plan }))
                .is_none()
            {
                self.bytes += entry.facts.layout_bytes();
            }
        }
        self.shrink(key);
        size
    }

    /// The cached layout of `text`, if there is one. Doesn't count as a use: the frame looked
    /// the text up with [`Self::known`] first.
    pub(crate) fn shaped(&self, key: TextKey, text: &str) -> Option<&Shaped> {
        self.cache
            .get(&key.0)
            .filter(|entry| *entry.text == *text)
            .and_then(|entry| entry.shaped.as_deref())
    }

    /// Estimated bytes of everything cached.
    #[cfg(test)]
    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }

    /// The entry of `text`, made on first sight, marked as just used.
    fn entry(&mut self, key: TextKey, text: &str) -> &mut Entry {
        self.tick += 1;
        if self
            .cache
            .get(&key.0)
            .is_none_or(|entry| *entry.text != *text)
        {
            let entry = Entry::new(text);
            self.bytes += entry.bytes();
            // Another text with the same hash makes way.
            if let Some(old) = self.cache.insert(key.0, entry) {
                self.bytes = self.bytes.saturating_sub(old.bytes());
            }
            self.shrink(key);
        }
        let tick = self.tick;
        // Always there: `shrink` keeps `key`.
        let entry = self.cache.entry(key.0).or_insert_with(|| Entry::new(text));
        entry.last_used = tick;
        entry
    }

    /// Drops the least recently used layouts, then entries, other than `keep`'s until the cache
    /// is within seven eighths of its limit, once it is over it.
    fn shrink(&mut self, keep: TextKey) {
        if self.bytes <= self.limit {
            return;
        }
        let target = self.limit / 8 * 7;
        let before = (self.cache.len(), self.bytes);
        // Layouts first: they take most of the bytes, and an entry without one still knows
        // its text's size.
        for layouts_only in [true, false] {
            let mut order: Vec<(u64, u64)> = self
                .cache
                .iter()
                .filter(|(key, entry)| **key != keep.0 && (!layouts_only || entry.shaped.is_some()))
                .map(|(key, entry)| (entry.last_used, *key))
                .collect();
            order.sort_unstable();
            for (_, key) in order {
                if self.bytes <= target {
                    break;
                }
                if layouts_only {
                    if let Some(entry) = self.cache.get_mut(&key)
                        && entry.shaped.take().is_some()
                    {
                        self.bytes = self.bytes.saturating_sub(entry.facts.layout_bytes());
                    }
                } else if let Some(entry) = self.cache.remove(&key) {
                    self.bytes = self.bytes.saturating_sub(entry.bytes());
                }
            }
        }
        tracing::debug!(
            entries_before = before.0,
            bytes_before = before.1,
            entries = self.cache.len(),
            bytes = self.bytes,
            "text cache over its limit; dropped the least recently used layouts"
        );
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

#[expect(
    clippy::cast_possible_truncation,
    reason = "a plan holds at most a few hundred thousand chunks"
)]
fn to_u32(n: usize) -> u32 {
    n as u32
}

#[cfg(test)]
mod tests {
    use lawh_core::board::{MAX_TEXT_CHARS, MAX_TEXT_LINES};

    use super::*;

    fn lay_out(shaper: &mut TextShaper, glyphs: &mut GlyphTable, text: &str) -> (f32, f32) {
        let key = shaper.key(text);
        shaper.lay_out(key, text, glyphs)
    }

    fn known(shaper: &mut TextShaper, text: &str) -> Known {
        let key = shaper.key(text);
        shaper.known(key, text)
    }

    /// `n` characters of `alphabet` after `tag`, in lines of `per_line` characters.
    fn cycled(tag: usize, n: usize, per_line: usize, alphabet: &str) -> String {
        let mut out = format!("{tag} ");
        let mut column = out.len();
        let mut chars = alphabet.chars().cycle();
        while out.chars().count() < n {
            if column + 1 >= per_line {
                out.push('\n');
                column = 0;
            } else {
                out.push(chars.next().unwrap());
                column += 1;
            }
        }
        out
    }

    #[test]
    fn facts_count_lines_as_lawh_core_does() {
        let facts = Facts::of("ab\r\ncd\ref\u{B}g\u{2028}hijk");
        assert_eq!(
            (
                facts.chars,
                facts.lines,
                facts.longest_line,
                facts.non_ascii
            ),
            (16, 5, 4, 1)
        );
        assert_eq!(Facts::of("").lines, 1);
        // Latin with combining marks is one block; Hebrew and CJK are others.
        assert_eq!(Facts::of("e\u{301}te\u{301}").changes, 0);
        assert_eq!(Facts::of("a\u{5D0}a\u{5D0}").changes, 3);
        assert_eq!(Facts::of("\u{6F22}\u{5B57}").changes, 2);
    }

    #[test]
    fn a_text_not_laid_out_is_estimated_no_smaller_than_its_layout() {
        let mut glyphs = GlyphTable::new();
        let mut shaper = TextShaper::new();
        for text in [
            "x",
            "lorem ipsum\ndolor sit amet",
            "WWWWWWWW\n\n\n",
            "\u{6F22}\u{5B57}\u{5D0}a\u{301}",
        ] {
            let before = known(&mut shaper, text);
            let estimated = shaper.size(&before);
            let (width, height) = lay_out(&mut shaper, &mut glyphs, text);
            assert!(
                estimated.0 >= width && estimated.1 >= height,
                "{text:?}: estimated {estimated:?}, laid out {width} x {height}"
            );
            // Once laid out, its real size.
            let after = known(&mut shaper, text);
            assert_eq!(shaper.size(&after), (width, height));
        }
    }

    #[test]
    fn the_cache_drops_the_least_recently_used_layouts_first() {
        let mut glyphs = GlyphTable::new();
        let texts: Vec<String> = (0..4)
            .map(|i| format!("text {i} {}", "lorem ipsum ".repeat(20)))
            .collect();
        // Room for three of them laid out, and a half.
        let one = ENTRY_BYTES + texts[0].len() + Facts::of(&texts[0]).layout_bytes();
        let limit = one * 7 / 2;
        let mut shaper = TextShaper::with_limit(limit);
        for text in &texts[..3] {
            lay_out(&mut shaper, &mut glyphs, text);
        }
        // Looking the first up again leaves the second least recently used.
        assert!(known(&mut shaper, &texts[0]).shaped);
        lay_out(&mut shaper, &mut glyphs, &texts[3]);
        assert!(shaper.bytes() <= limit, "{} > {limit}", shaper.bytes());
        let kept: Vec<bool> = texts
            .iter()
            .map(|text| {
                let key = TextKey(shaper.hasher.hash_one(text));
                shaper.shaped(key, text).is_some()
            })
            .collect();
        assert_eq!(kept, [true, false, true, true]);
        // It still knows the size of the text it dropped the layout of, and lays it out again
        // when asked.
        let dropped = known(&mut shaper, &texts[1]);
        assert!(dropped.size.is_some() && !dropped.shaped);
        let built = shaper.built;
        lay_out(&mut shaper, &mut glyphs, &texts[1]);
        assert_eq!(shaper.built, built + 1);
    }

    /// Makes the `i`th text of `n` characters.
    type Make = fn(usize, usize) -> String;

    #[test]
    fn a_board_at_the_text_caps_fits_in_the_cache() {
        const { assert!(FULL_BOARD_BYTES <= MAX_TEXT_CACHE_BYTES) };
        let cases: [(usize, Make); 3] = [
            (MAX_TEXT_CHARS, |i, n| {
                cycled(i, n, 11, "a\u{3B1}\u{430}\u{6F22}\u{627}")
            }),
            (MAX_TEXT_LINES, |i, _| {
                format!("{i}{}", "\n".repeat(MAX_TEXT_LINES - 2))
            }),
            (MAX_BOARD_TEXT_CHARS / MAX_OBJECTS, |i, n| {
                cycled(i, n, usize::MAX, "lorem ipsum ")
            }),
        ];
        let mut glyphs = GlyphTable::new();
        for (per_text, text) in cases {
            let count = (MAX_BOARD_TEXT_CHARS / per_text).min(MAX_OBJECTS);
            let texts: Vec<String> = (0..count).map(|i| text(i, per_text)).collect();
            let mut shaper = TextShaper::new();
            for text in &texts {
                lay_out(&mut shaper, &mut glyphs, text);
            }
            assert!(shaper.bytes() <= MAX_TEXT_CACHE_BYTES, "{}", shaper.bytes());
            assert!(
                texts.iter().all(|text| known(&mut shaper, text).shaped),
                "a layout of a board at the caps was dropped"
            );
            assert_eq!(shaper.built, u64::try_from(count).unwrap());
        }
    }
}
