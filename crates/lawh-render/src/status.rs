//! The status bar's text: the part of each string that is shown, and its layouts, cached by
//! content.
//!
//! A status string can be long: the command line holds up to
//! `lawh_core::engine::MAX_COMMAND_LINE_CHARS` characters, and a message can name anything.
//! Only a window of [`STATUS_WINDOW_CHARS`] characters of each is laid out and drawn, around
//! the command line's cursor, so the status bar costs about the same every frame whatever it
//! holds. The few strings shown lately are kept laid out ([`STATUS_CACHE_ENTRIES`]), so a frame
//! lays out only a string that changed.

use parley::Layout;

use crate::text::TextShaper;

/// Most characters of one status string laid out and drawn: about 1,800 logical pixels of
/// 13-pixel text, wider than most windows. Laying out this many costs at most about 0.5 ms,
/// and only when the string changes; drawing them, about 0.1 ms.
pub(crate) const STATUS_WINDOW_CHARS: usize = 256;

/// Status strings kept laid out: the left and right of the status line and the command line,
/// with room for the ones shown a moment ago.
const STATUS_CACHE_ENTRIES: usize = 8;

/// Shown where a status string is cut.
const ELLIPSIS: char = '\u{2026}';

/// Which part of a long string a window keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Keep {
    /// The start: messages and modes, read from the left.
    Start,
    /// The end: the right of the status line, whose last part (the cursor position) matters
    /// most.
    End,
    /// The characters around a byte offset: the command line's cursor.
    Around(usize),
}

/// The status string to lay out for `text`: `prefix`, then at most [`STATUS_WINDOW_CHARS`]
/// characters of `text` chosen by `keep`, with an ellipsis where it is cut. Line breaks, tabs
/// and other control characters show as spaces, so it stays one line. With the byte offset in
/// it of the [`Keep::Around`] offset (clamped into `text` and down to a character boundary),
/// or of its end.
pub(crate) fn window(prefix: &str, text: &str, keep: Keep) -> (String, usize) {
    let total = text.chars().count();
    let cursor = match keep {
        Keep::Around(byte) => {
            let mut byte = byte.min(text.len());
            while !text.is_char_boundary(byte) {
                byte -= 1;
            }
            text.get(..byte).map_or(0, |before| before.chars().count())
        }
        Keep::Start => 0,
        Keep::End => total,
    };
    let shown = total.min(STATUS_WINDOW_CHARS);
    let start = match keep {
        Keep::Start => 0,
        Keep::End => total - shown,
        Keep::Around(_) => cursor
            .saturating_sub(STATUS_WINDOW_CHARS / 2)
            .min(total - shown),
    };
    let end = start + shown;
    let mut out = String::with_capacity(prefix.len() + shown + 2 * ELLIPSIS.len_utf8());
    out.push_str(prefix);
    if start > 0 {
        out.push(ELLIPSIS);
    }
    let mut at = None;
    for (index, c) in text.chars().enumerate().skip(start).take(shown) {
        if index == cursor {
            at = Some(out.len());
        }
        out.push(shown_as(c));
    }
    let at = at.unwrap_or(out.len());
    if end < total {
        out.push(ELLIPSIS);
    }
    (out, at)
}

/// How a character of a status string shows: control characters and line or paragraph
/// separators as spaces.
fn shown_as(c: char) -> char {
    if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') {
        ' '
    } else {
        c
    }
}

struct Entry {
    text: String,
    layout: Layout<()>,
    last_used: u64,
}

/// Layouts of the status strings shown lately, by content.
#[derive(Default)]
pub(crate) struct StatusText {
    entries: Vec<Entry>,
    tick: u64,
    /// Strings laid out since this cache was made, for tests.
    pub(crate) built: u64,
}

impl StatusText {
    /// The slot of `text`'s layout, laid out with `shaper` unless it is cached. The slot holds it
    /// until [`STATUS_CACHE_ENTRIES`] other strings have been looked up since.
    pub(crate) fn shape(&mut self, shaper: &mut TextShaper, text: &str) -> usize {
        self.tick += 1;
        if let Some(slot) = self.entries.iter().position(|entry| entry.text == text) {
            if let Some(entry) = self.entries.get_mut(slot) {
                entry.last_used = self.tick;
            }
            return slot;
        }
        self.built += 1;
        let entry = Entry {
            text: text.to_owned(),
            layout: shaper.build(text),
            last_used: self.tick,
        };
        if self.entries.len() < STATUS_CACHE_ENTRIES {
            self.entries.push(entry);
            return self.entries.len() - 1;
        }
        let oldest = self
            .entries
            .iter()
            .enumerate()
            .min_by_key(|(_, entry)| entry.last_used)
            .map_or(0, |(slot, _)| slot);
        if let Some(slot) = self.entries.get_mut(oldest) {
            *slot = entry;
        }
        oldest
    }

    /// The layout in `slot`, from [`Self::shape`].
    pub(crate) fn layout(&self, slot: usize) -> Option<&Layout<()>> {
        self.entries.get(slot).map(|entry| &entry.layout)
    }

    /// The strings cached.
    #[cfg(test)]
    pub(crate) fn texts(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|entry| entry.text.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_strings_are_shown_whole_with_their_cursor() {
        assert_eq!(
            window(":", "ci 3", Keep::Around(4)),
            (":ci 3".to_owned(), 5)
        );
        assert_eq!(
            window(":", "ci 3", Keep::Around(1)),
            (":ci 3".to_owned(), 2)
        );
        assert_eq!(window("", "NORMAL", Keep::Start), ("NORMAL".to_owned(), 0));
        assert_eq!(window("", "", Keep::End), (String::new(), 0));
        // Past the end, or inside a character: clamped to a boundary.
        assert_eq!(window(":", "ab", Keep::Around(99)), (":ab".to_owned(), 3));
        assert_eq!(
            window(":", "\u{E9}x", Keep::Around(1)),
            (":\u{E9}x".to_owned(), 1)
        );
        // One line, whatever the string holds.
        assert_eq!(
            window("", "a\nb\tc\r\nd\u{2028}e", Keep::Start).0,
            "a b c  d e"
        );
    }

    #[test]
    fn long_strings_show_a_window_around_the_cursor() {
        let long = "x".repeat(5_064);
        for (keep, cursor_at) in [
            (Keep::Around(5_064), Some(STATUS_WINDOW_CHARS)),
            (Keep::Around(0), Some(0)),
            (Keep::Around(2_500), Some(STATUS_WINDOW_CHARS / 2)),
            (Keep::Start, None),
            (Keep::End, None),
        ] {
            let (shown, at) = window(":", &long, keep);
            let chars = shown.chars().count();
            assert!(chars <= STATUS_WINDOW_CHARS + 3, "{keep:?}: {chars}");
            let x = shown.chars().filter(|c| *c == 'x').count();
            assert_eq!(x, STATUS_WINDOW_CHARS, "{keep:?}");
            if let Some(cursor_at) = cursor_at {
                // The cursor stands where it stood in the text, counted from the window.
                let before = shown
                    .get(..at)
                    .unwrap()
                    .chars()
                    .filter(|c| *c == 'x')
                    .count();
                assert_eq!(before, cursor_at, "{keep:?}");
            }
            assert_eq!(
                shown.starts_with(":\u{2026}"),
                keep != Keep::Start && keep != Keep::Around(0)
            );
        }
        // The right of the status line keeps its end.
        let right = format!("{}  3,4", "f".repeat(1_000));
        assert!(window("", &right, Keep::End).0.ends_with("  3,4"));
    }

    #[test]
    fn the_cache_lays_out_only_strings_that_changed() {
        let mut shaper = TextShaper::new();
        let mut status = StatusText::default();
        let a = status.shape(&mut shaper, "NORMAL");
        let b = status.shape(&mut shaper, "area.lawh  0,0");
        assert_ne!(a, b);
        for _ in 0..3 {
            assert_eq!(status.shape(&mut shaper, "NORMAL"), a);
            assert_eq!(status.shape(&mut shaper, "area.lawh  0,0"), b);
        }
        assert_eq!(status.built, 2);
        // Many others push the least recently used out; the one in use stays.
        for i in 0..20 {
            status.shape(&mut shaper, "NORMAL");
            let slot = status.shape(&mut shaper, &format!("area.lawh  {i},0"));
            assert!(status.layout(slot).unwrap().width() > 0.0);
        }
        assert!(status.entries.len() <= STATUS_CACHE_ENTRIES);
        let built = status.built;
        assert_eq!(status.shape(&mut shaper, "NORMAL"), a);
        assert_eq!(status.built, built);
    }
}
