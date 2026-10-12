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

use std::borrow::Cow;
use std::collections::HashMap;
use std::hash::{BuildHasher, BuildHasherDefault, Hasher, RandomState};
use std::ops::Range;
use std::sync::{Arc, LazyLock};

use icu_properties::CodePointMapData;
use icu_properties::props::{BidiClass, BidiMirroringGlyph, BidiPairedBracketType, Script};
use lawh_core::board::{MAX_BOARD_TEXT_CHARS, MAX_OBJECTS};
use parley::fontique::{Blob, Collection, CollectionOptions, SourceCache};
use parley::setting::{FontFeature, Tag};
use parley::{
    Alignment, AlignmentOptions, FontContext, FontFamily, FontFamilyName, FontFeatures, Layout,
    LayoutContext, PositionedLayoutItem, StyleProperty,
};

use parley_data::Properties;

use crate::budget::{self, GlyphSums};
use crate::glyphs::GlyphTable;

/// Em size text is laid out at, in layout units. Drawing scales layout units
/// to [`crate::TEXT_SIZE_UNITS`] per em, so layouts don't depend on zoom.
pub(crate) const LAYOUT_EM: f32 = 20.0;

/// Family used when the bundled font list is somehow empty.
const FALLBACK_FAMILY: &str = "Inter";

/// Font features turned off, so every character shows as itself whatever is around it, as typed:
/// the cursor of a Vim editor moves over what is on screen, a glyph per character. Inter's
/// contextual alternates (`calt`) join `->`, `=>` and the like into one arrow glyph and raise
/// brackets, `:` and other punctuation next to capitals or between digits; automatic fractions
/// (`frac`, `numr`, `dnom`) shrink the digits around U+2044 FRACTION SLASH. `calt` also made
/// laying out brackets and most other punctuation 3 to 20 times slower, `)`, `]` and `}` worst
/// (`calibrate_text_layout`, 2026-10-11). Inter has no `liga`, `clig` or `rlig`, and the shaper
/// leaves `dlig` and `case` off. `ccmp` stays: it only joins a letter and its combining marks.
const FEATURES_OFF: [FontFeature; 4] = [
    FontFeature::new(Tag::new(b"calt"), 0),
    FontFeature::new(Tag::new(b"frac"), 0),
    FontFeature::new(Tag::new(b"numr"), 0),
    FontFeature::new(Tag::new(b"dnom"), 0),
];

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
    /// Places where parley starts another run to shape within a line, because the script or
    /// the bidi level changes there, or may ([`Splits`]): what makes laying text out costly.
    pub(crate) changes: usize,
    /// Paired brackets (`Bidi_Paired_Bracket`, such as `(` and `]`) when the text holds a
    /// character that can make a right-to-left level ([`Brackets`]), else 0. The bidi algorithm
    /// gives each pair the direction of what is inside it or around it (rule N0), which can start
    /// runs [`Splits`] doesn't count, so each counts as one more possible change.
    pub(crate) brackets: usize,
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
            brackets: 0,
            lines: 1,
            longest_line: 0,
        };
        let mut line = 0;
        let mut after_cr = false;
        let mut splits = Splits::new(text);
        for c in text.chars() {
            facts.chars += 1;
            if !c.is_ascii() {
                facts.non_ascii += 1;
            }
            splits.push(c);
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
        (facts.changes, facts.brackets) = splits.finish();
        facts
    }

    /// The estimated cost of laying this text out and planning it ([`budget::layout`]).
    pub(crate) fn layout_cost(&self) -> f64 {
        budget::layout(
            self.chars,
            self.non_ascii,
            self.changes + self.brackets,
            self.lines,
        )
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

/// Counts where parley starts another run to shape, character by character, without laying the
/// text out: where the script changes or the bidi level does. Each character's script and bidi
/// class come from parley's own tables.
///
/// As in parley, characters of no particular script (spaces, digits, punctuation, combining
/// marks) take the script before them. Bidi levels follow the bidi algorithm for a text without
/// explicit embeddings, in short: the text's direction is that of its first strong character
/// outside isolates; left-to-right letters are at level 0 in a left-to-right text and 2 in a
/// right-to-left one, right-to-left letters at 1, Arabic digits at 2, and European digits go
/// with left-to-right letters before them, else are at 2. Neutral characters between two of the
/// same direction (digits at 2 count as right-to-left) take that direction, the others the
/// text's; line and segment breaks are at the text's level; marks go with the character before
/// them. A test checks this against the runs parley lays out
/// (`changes_are_where_parley_starts_a_run`).
///
/// Explicit embeddings, overrides and isolates (U+202A to U+202E, U+2066 to U+2069) aren't
/// modelled: after one, letters stay at its level but a segment break goes back to the text's
/// (bidi rule L1), and so on. From the first of them on, every character counts as a split, the
/// most a character can split. Real text almost never holds these characters.
///
/// Paired brackets are counted apart (see [`Facts::brackets`]).
struct Splits {
    ascii: &'static [Class; 128],
    brackets: Brackets,
    /// The level of the text's direction: 1 for right-to-left, else 0.
    base_level: u8,
    /// The level of left-to-right letters.
    ltr_level: u8,
    count: usize,
    /// The script of the run so far, once a character has one.
    script: Option<Script>,
    /// The level and direction (right-to-left) of the last character that isn't neutral.
    before: Option<(u8, bool)>,
    /// Neutral characters came after it.
    neutrals: bool,
    /// The last strong character was left-to-right (or the text is, before one).
    strong_ltr: bool,
    /// The class of the last character, once there is one: another of the same class splits
    /// nothing.
    last: Option<Class>,
    /// An explicit embedding, override or isolate came: every character from it on counts as a
    /// split.
    explicit: bool,
}

/// Paired brackets so far, and whether a character that can make a right-to-left level came:
/// right-to-left, an Arabic digit or an explicit embedding, override or isolate. Only then can
/// the brackets start runs ([`Facts::brackets`]).
#[derive(Default)]
struct Brackets {
    count: usize,
    rtl: bool,
}

/// What [`Splits`] needs to know of a character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Class {
    /// Its script, unless it takes the script of the text around it.
    script: Option<Script>,
    bidi: Bidi,
    /// It is a paired bracket.
    bracket: bool,
}

/// Bidi classes, as far as [`Splits`] tells them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bidi {
    /// Strong left-to-right.
    Ltr,
    /// Strong right-to-left, Arabic letters included.
    Rtl,
    /// A European digit.
    European,
    /// An Arabic digit.
    Arabic,
    /// A line or segment break.
    Break,
    /// A nonspacing mark, or a character the bidi algorithm drops (such as a zero-width
    /// joiner): it goes with the character before it.
    Attached,
    /// An explicit embedding, override or isolate.
    Explicit,
    Neutral,
}

impl Class {
    fn of(c: char) -> Self {
        let properties = Properties::get(c);
        let script = properties.script();
        let bidi = match properties.bidi_class() {
            BidiClass::LeftToRight => Bidi::Ltr,
            BidiClass::RightToLeft | BidiClass::ArabicLetter => Bidi::Rtl,
            BidiClass::EuropeanNumber => Bidi::European,
            BidiClass::ArabicNumber => Bidi::Arabic,
            BidiClass::ParagraphSeparator | BidiClass::SegmentSeparator => Bidi::Break,
            BidiClass::NonspacingMark | BidiClass::BoundaryNeutral => Bidi::Attached,
            BidiClass::LeftToRightEmbedding
            | BidiClass::RightToLeftEmbedding
            | BidiClass::LeftToRightOverride
            | BidiClass::RightToLeftOverride
            | BidiClass::PopDirectionalFormat
            | BidiClass::LeftToRightIsolate
            | BidiClass::RightToLeftIsolate
            | BidiClass::FirstStrongIsolate
            | BidiClass::PopDirectionalIsolate => Bidi::Explicit,
            _ => Bidi::Neutral,
        };
        let bracket = const { CodePointMapData::<BidiMirroringGlyph>::new() }
            .get(c)
            .paired_bracket_type
            != BidiPairedBracketType::None;
        Self {
            script: (!matches!(script, Script::Common | Script::Inherited | Script::Unknown))
                .then_some(script),
            bidi,
            bracket,
        }
    }
}

/// The [`Class`] of each ASCII character, looked up once.
static ASCII: LazyLock<[Class; 128]> =
    LazyLock::new(|| std::array::from_fn(|i| Class::of(u8::try_from(i).map_or('\0', char::from))));

/// The [`Class`] of `c`, from `ascii` for an ASCII character.
fn class(ascii: &[Class; 128], c: char) -> Class {
    ascii
        .get(u32::from(c) as usize)
        .copied()
        .unwrap_or_else(|| Class::of(c))
}

impl Splits {
    fn new(text: &str) -> Self {
        let ascii = &*ASCII;
        // As in parley, strong characters between an isolate and its end (or the end of the
        // text) don't count (bidi rule P2).
        let mut isolates = 0_usize;
        let rtl = text
            .chars()
            .find_map(|c| match c {
                '\u{2066}'..='\u{2068}' => {
                    isolates += 1;
                    None
                }
                '\u{2069}' => {
                    isolates = isolates.saturating_sub(1);
                    None
                }
                _ if isolates > 0 => None,
                _ => match class(ascii, c).bidi {
                    Bidi::Ltr => Some(false),
                    Bidi::Rtl => Some(true),
                    _ => None,
                },
            })
            .unwrap_or(false);
        Self {
            ascii,
            brackets: Brackets::default(),
            base_level: u8::from(rtl),
            ltr_level: if rtl { 2 } else { 0 },
            count: 0,
            script: None,
            before: None,
            neutrals: false,
            strong_ltr: !rtl,
            last: None,
            explicit: false,
        }
    }

    /// Takes the next character of the text.
    fn push(&mut self, c: char) {
        let class = class(self.ascii, c);
        self.brackets.count += usize::from(class.bracket);
        self.brackets.rtl |= matches!(class.bidi, Bidi::Rtl | Bidi::Arabic | Bidi::Explicit);
        if self.explicit {
            self.count += 1;
            return;
        }
        if self.last.replace(class) == Some(class) {
            return;
        }
        let new_script = class
            .script
            .is_some_and(|script| self.script.replace(script).is_some_and(|old| old != script));
        let (level, rtl) = match class.bidi {
            Bidi::Ltr => {
                self.strong_ltr = true;
                (self.ltr_level, false)
            }
            Bidi::Rtl => {
                self.strong_ltr = false;
                (1, true)
            }
            Bidi::European if self.strong_ltr => (self.ltr_level, false),
            Bidi::European | Bidi::Arabic => (2, true),
            Bidi::Break => (self.base_level, self.rtl()),
            Bidi::Attached if !self.neutrals && self.before.is_some() => {
                self.count += usize::from(new_script);
                return;
            }
            // The first explicit code: it and every character after it count.
            Bidi::Explicit => {
                self.count += 1;
                self.explicit = true;
                return;
            }
            Bidi::Attached | Bidi::Neutral => {
                self.count += usize::from(new_script);
                self.neutrals = true;
                return;
            }
        };
        let last = if self.neutrals {
            Some(self.neutrals_before(rtl))
        } else {
            self.before.map(|(level, _)| level)
        };
        if new_script || last.is_some_and(|last| last != level) {
            self.count += 1;
        }
        self.before = Some((level, rtl));
        self.neutrals = false;
    }

    /// The text's direction is right-to-left.
    fn rtl(&self) -> bool {
        self.base_level == 1
    }

    /// Settles the neutral characters before one resolved to `next_rtl` (or the end of the
    /// text): counts the split at the first of them, and returns their level.
    fn neutrals_before(&mut self, next_rtl: bool) -> u8 {
        let before_rtl = self.before.map_or(self.rtl(), |(_, rtl)| rtl);
        let level = if before_rtl != next_rtl {
            self.base_level
        } else if next_rtl {
            1
        } else {
            self.ltr_level
        };
        if self.before.is_some_and(|(before, _)| before != level) {
            self.count += 1;
        }
        level
    }

    /// The splits in the whole text, and its paired brackets if it holds a character that can
    /// make a right-to-left level ([`Facts::brackets`]).
    fn finish(mut self) -> (usize, usize) {
        if self.neutrals {
            self.neutrals_before(self.rtl());
        }
        let brackets = if self.brackets.rtl {
            self.brackets.count
        } else {
            0
        };
        (self.count, brackets)
    }
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

    /// Glyph runs in all.
    #[cfg(test)]
    pub(crate) fn glyph_runs(&self) -> u32 {
        self.runs
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

/// Lays out board text with the bundled text font (`lawh_math::fonts()[0]`) only,
/// and caches what it laid out (see the module docs). Status strings ([`Self::build_status`])
/// fall back to the bundled math font for characters the text font lacks.
pub(crate) struct TextShaper {
    fonts: FontContext,
    layouts: LayoutContext<()>,
    family: &'static str,
    /// The families of status strings: the text font, then every other bundled font.
    status_families: Vec<FontFamilyName<'static>>,
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
    /// How far below the top of its box a one-line text's baseline is, in layout units.
    baseline: f32,
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
        // Every bundled font is registered, but board text names only the text font, and the
        // collection has no fallback families (no system fonts): board text never falls back to
        // another font, so its glyphs are all Inter's (`Plan` looks their outlines up there).
        // Status strings name the math font after it, for the symbols `:sym` lists.
        for asset in lawh_math::fonts() {
            let families = fonts
                .collection
                .register_fonts(Blob::new(Arc::new(asset.data)), None);
            if families.is_empty() {
                tracing::warn!(family = asset.family, "bundled font failed to load");
            }
        }
        let asset = lawh_math::fonts().first();
        if asset.is_none() {
            tracing::warn!("no bundled text font");
        }
        let family = asset.map_or(FALLBACK_FAMILY, |asset| asset.family);
        let mut status_families = vec![FontFamilyName::named(family)];
        status_families.extend(
            lawh_math::fonts()
                .iter()
                .filter(|other| other.family != family)
                .map(|other| FontFamilyName::named(other.family)),
        );
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
            family,
            status_families,
            hasher: RandomState::new(),
            cache: KeyMap::default(),
            bytes: 0,
            limit,
            tick: 0,
            line_height: LAYOUT_EM * 1.5,
            max_advance,
            baseline: LAYOUT_EM,
            hashed: 0,
            built: 0,
        };
        let x = shaper.build("x");
        let line = x.height();
        if line.is_finite() && line > 0.0 {
            shaper.line_height = line;
        }
        let baseline = x.lines().next().map(|line| line.metrics().baseline);
        if let Some(baseline) = baseline.filter(|v| v.is_finite() && *v > 0.0) {
            shaper.baseline = baseline;
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

    /// The height of one line of text, in layout units.
    pub(crate) fn line_height(&self) -> f32 {
        self.line_height
    }

    /// How far below the top of its box the baseline of a text's first line is, in layout
    /// units: the text font's ascender (0.97 em for Inter).
    pub(crate) fn baseline(&self) -> f32 {
        self.baseline
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

    /// Lays `text` out as board text is laid out, without caching it, in the text font only.
    pub(crate) fn build(&mut self, text: &str) -> Layout<()> {
        let family = FontFamily::Single(FontFamilyName::named(self.family));
        self.build_in(text, family)
    }

    /// Lays a status string out, without caching it (`crate::status` has its own cache): as
    /// board text, but characters the text font lacks fall back to the math font, so `:sym`
    /// rows show symbols such as `⨁` and `⟶`.
    pub(crate) fn build_status(&mut self, text: &str) -> Layout<()> {
        let families = std::mem::take(&mut self.status_families);
        let layout = self.build_in(text, FontFamily::List(Cow::Borrowed(&families)));
        self.status_families = families;
        layout
    }

    fn build_in(&mut self, text: &str, family: FontFamily<'_>) -> Layout<()> {
        // No quantizing: positions stay fractional so text scales smoothly.
        let mut builder = self
            .layouts
            .ranged_builder(&mut self.fonts, text, 1.0, false);
        builder.push_default(StyleProperty::FontFamily(family));
        builder.push_default(StyleProperty::FontSize(LAYOUT_EM));
        builder.push_default(FontFeatures::from(&FEATURES_OFF));
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

    /// The glyph runs parley lays `text` out in.
    fn glyph_runs(shaper: &mut TextShaper, glyphs: &mut GlyphTable, text: &str) -> usize {
        lay_out(shaper, glyphs, text);
        let key = shaper.key(text);
        shaper.shaped(key, text).unwrap().plan.glyph_runs() as usize
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
        // Combining marks and digits take the script before them; Han is one script however
        // far apart its characters are. Hebrew after Latin changes script and direction,
        // Hebrew after Armenian too (in the same 128 code points), an Arabic-Indic digit after
        // an Arabic letter only the bidi level.
        assert_eq!(Facts::of("e\u{301}te\u{301}").changes, 0);
        assert_eq!(Facts::of("a1b2c3").changes, 0);
        assert_eq!(Facts::of("\u{6F22}\u{5B57}\u{6587}").changes, 0);
        assert_eq!(Facts::of("a\u{5D0}a\u{5D0}").changes, 3);
        assert_eq!(Facts::of("\u{580}\u{5D0}\u{580}").changes, 2);
        assert_eq!(Facts::of("\u{627}\u{661}\u{627}").changes, 2);
    }

    #[test]
    fn changes_are_where_parley_starts_a_run() {
        let mut glyphs = GlyphTable::new();
        let mut shaper = TextShaper::new();
        for text in [
            "lorem ipsum, dolor 12.5% sit",
            "\u{6F22}\u{5B57}\u{4EEE}\u{540D}\u{3058}\u{308A}\u{6587}",
            "\u{3B1}\u{3B2}\u{3B3} \u{3B4}\u{3B5}",
            "\u{43F}\u{440}\u{438} \u{43C}\u{438}\u{440}",
            "ab \u{5D0}\u{5D1} cd",
            "a\u{5D0}a\u{5D0}",
            "\u{580}\u{5D0}\u{580}\u{5D0}",
            "\u{627}\u{661}\u{627}\u{661}",
            "\u{5D0}1\u{5D0}1",
            "\u{500}\u{531}\u{500}\u{531}",
            "\u{633}\u{644}\u{627}\u{645} \u{661}\u{662}\u{663} 45",
            "a\u{3B1}\u{430}\u{6F22}\u{627}",
            "e\u{301}te\u{301} (x)",
            "area \u{5D0}\u{5D1} is 12 or \u{661}\u{662}",
            "\u{5D0}\u{5D1} area 12 \u{5D2}",
            "\u{5E9}\u{5B8}\u{5C1}\u{5DC}\u{5D5}\u{5B9}\u{5DD}world",
            "a (\u{5D0}\u{5D1}) b",
            "\u{5D0} (cd) \u{5D1}",
            "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{FE0F} family",
        ] {
            let facts = Facts::of(text);
            let runs = glyph_runs(&mut shaper, &mut glyphs, text);
            assert_eq!(facts.changes + 1, runs, "{text:?}");
        }

        // From an explicit embedding, override or isolate on, every character counts, which is
        // no fewer than the runs parley starts. In a right-to-left embedding or isolate, letters
        // stay at its level and a tab goes back to the text's (bidi rule L1): a run each.
        let mut texts: Vec<String> = ['\u{202B}', '\u{2067}']
            .into_iter()
            .map(|code| {
                let mut text = String::from(code);
                text.extend("a\t".chars().cycle().take(4995));
                text
            })
            .collect();
        // Strong characters in an isolate don't set the text's direction: this one is
        // right-to-left, so its digits and spaces take turns.
        texts.push("1 2 3 4 5 6 \u{2066}a\u{2069}\u{5D0}".to_owned());
        // Short texts of explicit codes, tabs, letters of both directions, digits, brackets and
        // spaces, drawn from a fixed seed.
        let alphabet: Vec<char> = ('\u{202A}'..='\u{202E}')
            .chain('\u{2066}'..='\u{2069}')
            .chain("\tab\u{5D0}\u{5D1}\u{627}12\u{661}\u{662}()[] ".chars())
            .collect();
        let mut seed = 0x2545_F491_4F6C_DD1D_u64;
        let mut below = |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            usize::try_from(seed % u64::try_from(n).unwrap()).unwrap()
        };
        texts.extend((0..4000).map(|_| {
            let len = 1 + below(16);
            (0..len).map(|_| alphabet[below(alphabet.len())]).collect()
        }));
        let mut under = Vec::new();
        for text in &texts {
            let facts = Facts::of(text);
            let runs = glyph_runs(&mut shaper, &mut glyphs, text);
            if facts.changes + facts.brackets + 1 < runs {
                under.push(format!("{text:?}: {runs} runs, {facts:?}"));
            }
        }
        assert!(
            under.is_empty(),
            "{} of {} texts have more runs than counted:\n{}",
            under.len(),
            texts.len(),
            under.join("\n")
        );
    }

    /// Every character shows as its own glyph, the same whatever is around it ([`FEATURES_OFF`]).
    #[test]
    fn characters_show_as_themselves_whatever_is_around_them() {
        let mut glyphs = GlyphTable::new();
        let mut shaper = TextShaper::new();
        let mut ids = |text: &str| {
            lay_out(&mut shaper, &mut glyphs, text);
            let key = shaper.key(text);
            let layout = &shaper.shaped(key, text).unwrap().layout;
            let mut ids = Vec::new();
            for line in layout.lines() {
                for item in line.items() {
                    if let PositionedLayoutItem::GlyphRun(run) = item {
                        ids.extend(run.positioned_glyphs().map(|glyph| glyph.id));
                    }
                }
            }
            ids
        };
        // No arrows from `->` or `=>`.
        assert_eq!(ids("a->b=>c<-d").len(), 10);
        // No raised brackets next to capitals, or `:` between digits.
        let (lower, upper) = (ids("(a)"), ids("(A)"));
        assert_eq!((lower[0], lower[2]), (upper[0], upper[2]));
        assert_eq!(ids("12:30")[2], ids("a:")[1]);
        // No small digits around a fraction slash.
        assert_eq!(ids("1\u{2044}2")[0], ids("1")[0]);
    }

    /// In a text with a right-to-left character, the bidi algorithm resolves pairs of brackets on
    /// their own, which can start more runs than `changes` counts: each paired bracket counts as
    /// one more.
    #[test]
    fn paired_brackets_in_right_to_left_text_count_as_changes() {
        let mut glyphs = GlyphTable::new();
        let mut shaper = TextShaper::new();
        let nested = "\u{5D0}((a))b".repeat(50);
        let facts = Facts::of(&nested);
        let laid_out = glyph_runs(&mut shaper, &mut glyphs, &nested);
        assert!(laid_out > facts.changes + 1, "{laid_out} runs, {facts:?}");
        for text in [
            nested.as_str(),
            "a [\u{5D0}] {b} (\u{661}) \u{2329}c\u{232A}",
            "\u{627} (x) \u{FF08}y\u{FF09}",
            "x\u{202B}(a)\u{202C}",
        ] {
            let facts = Facts::of(text);
            assert!(facts.brackets > 0, "{text:?}");
            let laid_out = glyph_runs(&mut shaper, &mut glyphs, text);
            assert!(
                laid_out <= facts.changes + facts.brackets + 1,
                "{text:?}: {laid_out} runs, {facts:?}"
            );
        }
        // Without a right-to-left character, brackets split nothing.
        assert_eq!(Facts::of("f(g(x)) [a] {b} \u{6F22}(c)").brackets, 0);
        assert_eq!(Facts::of("f(x) \u{5D0}").brackets, 2);
    }

    /// The bundled font each glyph run of `layout` is drawn from, by index into
    /// `lawh_math::fonts`.
    fn run_fonts(layout: &Layout<()>) -> Vec<usize> {
        let mut fonts = Vec::new();
        for line in layout.lines() {
            for item in line.items() {
                if let PositionedLayoutItem::GlyphRun(run) = item {
                    let data = run.run().font().data.data().as_ptr();
                    fonts.push(
                        lawh_math::fonts()
                            .iter()
                            .position(|asset| std::ptr::eq(asset.data.as_ptr(), data))
                            .unwrap(),
                    );
                }
            }
        }
        fonts
    }

    /// Board text is drawn in the text font only, as the frame budget assumes; status strings
    /// fall back to the math font for symbols the text font lacks.
    #[test]
    fn only_status_strings_fall_back_to_the_math_font() {
        // U+2A01 N-ARY CIRCLED PLUS OPERATOR: in Libertinus Math, not in Inter.
        let table = GlyphTable::new();
        assert!(table.char_glyph(0, '\u{2A01}').is_none());
        assert!(table.char_glyph(1, '\u{2A01}').is_some());
        let mut shaper = TextShaper::new();
        let text = "plus.circle.big \u{2A01}";
        // The symbol is Inter's missing-glyph box on the board.
        assert!(run_fonts(&shaper.build(text)).iter().all(|font| *font == 0));
        assert_eq!(run_fonts(&shaper.build_status(text)), [0, 1]);
        // Inter has Greek (a run of its own, a script of its own): no fallback.
        let greek = run_fonts(&shaper.build_status("alpha \u{3B1}"));
        assert!(!greek.is_empty() && greek.iter().all(|font| *font == 0));
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
