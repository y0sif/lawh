//! Positions in a block's text, as byte offsets: graphemes, lines, columns and Vim's word
//! motions. Pure functions over `&str`, for Insert and block-Normal mode.
//!
//! A step is one grapheme (an extended grapheme cluster), so `é` written as `e` and a combining
//! accent, or a family emoji joined with ZWJ, moves and deletes as one. Offsets stay on grapheme
//! boundaries. Lines break where the board counts line breaks (see
//! [`MAX_TEXT_LINES`](crate::board::MAX_TEXT_LINES)): `\n`, `\r`, CRLF (one break), vertical
//! tab, form feed, next line, line separator and paragraph separator. Each is one grapheme.
//!
//! Every key in Insert and block-Normal mode runs some of these, so they look only around the
//! cursor: the graphemes next to it, its line, and the lines a motion crosses. None splits the
//! whole text into lines or graphemes.

use unicode_segmentation::{GraphemeCursor, UnicodeSegmentation};

use crate::board::Shape;

/// The text of a text object or the source of a math object; `None` for other kinds.
pub(super) fn content(shape: &Shape) -> Option<&str> {
    match shape {
        Shape::Text { text, .. } => Some(text),
        Shape::Math { src, .. } => Some(src),
        Shape::Ellipse { .. } | Shape::Rect { .. } | Shape::Line { .. } | Shape::Arrow { .. } => {
            None
        }
    }
}

/// `shape` with `text` as its text or source; `None` for kinds without one.
pub(super) fn with_content(shape: &Shape, text: String) -> Option<Shape> {
    match shape {
        Shape::Text { at, .. } => Some(Shape::Text { at: *at, text }),
        Shape::Math { at, .. } => Some(Shape::Math { at: *at, src: text }),
        Shape::Ellipse { .. } | Shape::Rect { .. } | Shape::Line { .. } | Shape::Arrow { .. } => {
            None
        }
    }
}

/// One line of a text: the bytes of its content, without the line break after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Line {
    pub(super) start: usize,
    /// Where the line break after the line starts, or the end of the text.
    pub(super) end: usize,
}

impl Line {
    pub(super) fn is_empty(self) -> bool {
        self.start == self.end
    }
}

/// The grapheme breaks a line.
fn is_break(grapheme: &str) -> bool {
    matches!(
        grapheme,
        "\n" | "\r" | "\r\n" | "\u{B}" | "\u{C}" | "\u{85}" | "\u{2028}" | "\u{2029}"
    )
}

/// The character starts a line break. Every break is a grapheme of its own (CR and LF together
/// make one), so the characters that break lines are exactly where the graphemes do.
fn is_break_char(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r' | '\u{B}' | '\u{C}' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}

/// The line `byte` is on; a line's end (its break) counts as on it. It looks from `byte` back
/// and forward to the nearest line breaks only.
pub(super) fn line_at(text: &str, byte: usize) -> Line {
    let mut byte = text.floor_char_boundary(byte);
    // Between the CR and the LF of one break: on the line after it.
    if byte > 0 && text.get(byte - 1..=byte) == Some("\r\n") {
        byte += 1;
    }
    let start = text
        .get(..byte)
        .unwrap_or_default()
        .char_indices()
        .rev()
        .find(|&(_, c)| is_break_char(c))
        .map_or(0, |(i, c)| i + c.len_utf8());
    let end = text
        .get(byte..)
        .unwrap_or_default()
        .find(is_break_char)
        .map_or(text.len(), |i| byte + i);
    Line { start, end }
}

/// The line after `line`, or `None` on the last line.
fn next_line(text: &str, line: Line) -> Option<Line> {
    (line.end < text.len()).then(|| line_at(text, next_grapheme(text, line.end)))
}

/// The line before `line`, or `None` on the first line.
fn prev_line(text: &str, line: Line) -> Option<Line> {
    (line.start > 0).then(|| line_at(text, prev_grapheme(text, line.start)))
}

/// `byte` starts a line: the text's start, or just after a line break.
fn at_line_start(text: &str, byte: usize) -> bool {
    byte == 0
        || text
            .get(prev_grapheme(text, byte)..byte)
            .is_some_and(is_break)
}

/// `byte` ends a line: the text's end, or a line break.
fn at_line_end(text: &str, byte: usize) -> bool {
    byte >= text.len()
        || text
            .get(byte..next_grapheme(text, byte))
            .is_some_and(is_break)
}

/// Where the grapheme at `byte` ends; `byte` at the end of the text.
pub(super) fn next_grapheme(text: &str, byte: usize) -> usize {
    text.get(byte..)
        .and_then(|rest| rest.graphemes(true).next())
        .map_or(byte, |g| byte + g.len())
}

/// Where the grapheme before `byte` starts; `byte` at the start of the text.
pub(super) fn prev_grapheme(text: &str, byte: usize) -> usize {
    text.get(..byte)
        .and_then(|before| before.graphemes(true).next_back())
        .map_or(byte, |g| byte - g.len())
}

/// The grapheme boundary at `byte`, or the nearest after it. It looks only at the graphemes
/// around `byte`, so typing stays cheap in a long text.
pub(super) fn ceil_boundary(text: &str, byte: usize) -> usize {
    if byte >= text.len() {
        return text.len();
    }
    if text.is_char_boundary(byte) {
        // The whole text is the chunk, so the cursor never asks for more context.
        let mut cursor = GraphemeCursor::new(byte, text.len(), true);
        match cursor.is_boundary(text, 0) {
            Ok(true) => return byte,
            Ok(false) => {
                if let Ok(Some(next)) = cursor.next_boundary(text, 0) {
                    return next;
                }
            }
            Err(_) => {}
        }
    }
    text.grapheme_indices(true)
        .map(|(i, _)| i)
        .find(|&i| i >= byte)
        .unwrap_or(text.len())
}

/// The grapheme boundary at `byte`, or the nearest before it. Like [`ceil_boundary`], it looks
/// only at the graphemes around `byte`.
pub(super) fn floor_boundary(text: &str, byte: usize) -> usize {
    if byte >= text.len() {
        return text.len();
    }
    let byte = text.floor_char_boundary(byte);
    // The whole text is the chunk, so the cursor never asks for more context.
    let mut cursor = GraphemeCursor::new(byte, text.len(), true);
    match cursor.is_boundary(text, 0) {
        Ok(true) => return byte,
        Ok(false) => {
            if let Ok(Some(prev)) = cursor.prev_boundary(text, 0) {
                return prev;
            }
        }
        Err(_) => {}
    }
    text.grapheme_indices(true)
        .map(|(i, _)| i)
        .take_while(|&i| i <= byte)
        .last()
        .unwrap_or(0)
}

/// Where each grapheme of `line` starts.
fn starts(text: &str, line: Line) -> Vec<usize> {
    text.get(line.start..line.end)
        .unwrap_or_default()
        .grapheme_indices(true)
        .map(|(i, _)| line.start + i)
        .collect()
}

/// Graphemes between the start of `byte`'s line and `byte`: its column.
pub(super) fn column(text: &str, byte: usize) -> usize {
    let line = line_at(text, byte);
    starts(text, line).iter().take_while(|&&i| i < byte).count()
}

/// The offset at column `column` of `line`, or the nearest the line has: its end when
/// `past_end` (Insert mode), else its last grapheme.
fn at_column(text: &str, line: Line, column: usize, past_end: bool) -> usize {
    let starts = starts(text, line);
    let last = if past_end {
        starts.len()
    } else {
        starts.len().saturating_sub(1)
    };
    starts.get(column.min(last)).copied().unwrap_or(line.end)
}

/// Where block-Normal mode keeps `byte`: on a grapheme, the last of its line at most; at the
/// line's start when the line is empty.
pub(super) fn on_grapheme(text: &str, byte: usize) -> usize {
    let byte = floor_boundary(text, byte);
    if at_line_end(text, byte) && !at_line_start(text, byte) {
        prev_grapheme(text, byte)
    } else {
        byte
    }
}

/// Where the cursor goes when Insert mode ends at `byte`: one grapheme back, as in Vim, unless
/// at the start of a line.
pub(super) fn after_insert(text: &str, byte: usize) -> usize {
    let back = if at_line_start(text, byte) {
        byte
    } else {
        prev_grapheme(text, byte)
    };
    on_grapheme(text, back)
}

/// `count` graphemes left of `byte`, within its line.
pub(super) fn left(text: &str, byte: usize, count: u32) -> usize {
    let mut at = byte;
    for _ in 0..count {
        if at_line_start(text, at) {
            break;
        }
        at = prev_grapheme(text, at);
    }
    at
}

/// `count` graphemes right of `byte`, within its line: up to its end when `past_end`, else up
/// to its last grapheme.
pub(super) fn right(text: &str, byte: usize, count: u32, past_end: bool) -> usize {
    let mut at = byte;
    for _ in 0..count {
        if at_line_end(text, at) {
            break;
        }
        let next = next_grapheme(text, at);
        if next <= at || (!past_end && at_line_end(text, next)) {
            break;
        }
        at = next;
    }
    at
}

/// `count` lines below `byte` (above when `up`), at column `want` or the nearest the line has;
/// `None` when there is no line that way. A count past the first or last line stops there.
pub(super) fn vertical(
    text: &str,
    byte: usize,
    count: u32,
    up: bool,
    want: usize,
    past_end: bool,
) -> Option<usize> {
    let step = |line| {
        if up {
            prev_line(text, line)
        } else {
            next_line(text, line)
        }
    };
    let mut line = step(line_at(text, byte))?;
    for _ in 1..count {
        match step(line) {
            Some(next) => line = next,
            None => break,
        }
    }
    Some(at_column(text, line, want, past_end))
}

/// The start of `byte`'s line.
pub(super) fn line_start(text: &str, byte: usize) -> usize {
    line_at(text, byte).start
}

/// The end of `byte`'s line: where its break starts, or the end of the text.
pub(super) fn line_end(text: &str, byte: usize) -> usize {
    line_at(text, byte).end
}

/// The first grapheme of `byte`'s line that is not blank, or the line's end.
pub(super) fn first_non_blank(text: &str, byte: usize) -> usize {
    let line = line_at(text, byte);
    text.get(line.start..line.end)
        .unwrap_or_default()
        .grapheme_indices(true)
        .find(|(_, g)| class(g) != Class::Blank)
        .map_or(line.end, |(i, _)| line.start + i)
}

/// `$` with a count: the last grapheme of the line `count - 1` lines below `byte`, or of the
/// last line.
pub(super) fn last_of_line(text: &str, byte: usize, count: u32) -> usize {
    let mut line = line_at(text, byte);
    for _ in 1..count {
        match next_line(text, line) {
            Some(next) => line = next,
            None => break,
        }
    }
    on_grapheme(text, line.end)
}

/// A grapheme's class for word motions, as Vim's `cls()`: blanks separate words; a word is a
/// run of letters, digits and `_`, or a run of other characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Blank,
    Punctuation,
    Word,
}

/// The class of a grapheme, from its first character.
fn class(grapheme: &str) -> Class {
    match grapheme.chars().next() {
        None => Class::Blank,
        Some(c) if c.is_whitespace() => Class::Blank,
        Some(c) if c.is_alphanumeric() || c == '_' => Class::Word,
        Some(_) => Class::Punctuation,
    }
}

/// What a step of a [`Walk`] did, as Vim's `inc()` and `dec()` report it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Moved within the line.
    Within,
    /// Moved forward onto the line's end.
    OntoLineEnd,
    /// Moved to the next line's start or the previous line's end.
    Crossed,
    /// At the start or end of the text: did not move.
    Stuck,
}

/// A walk over the positions Vim's word motions see: every grapheme of each line, and each
/// line's end (where Vim's cursor stands on the line's NUL), which counts as blank.
struct Walk<'a> {
    text: &'a str,
    /// The line the walk is on.
    line: Line,
    byte: usize,
}

impl<'a> Walk<'a> {
    fn new(text: &'a str, byte: usize) -> Self {
        let byte = floor_boundary(text, byte);
        Self {
            text,
            line: line_at(text, byte),
            byte,
        }
    }

    fn current(&self) -> Line {
        self.line
    }

    /// On the text's last line.
    fn on_last_line(&self) -> bool {
        self.line.end >= self.text.len()
    }

    /// One position forward: Vim's `inc()`.
    fn inc(&mut self) -> Step {
        let line = self.current();
        if self.byte < line.end {
            let next = next_grapheme(self.text, self.byte);
            self.byte = if next > self.byte {
                next.min(line.end)
            } else {
                line.end
            };
            return if self.byte == line.end {
                Step::OntoLineEnd
            } else {
                Step::Within
            };
        }
        match next_line(self.text, line) {
            Some(next) => {
                self.line = next;
                self.byte = next.start;
                Step::Crossed
            }
            None => Step::Stuck,
        }
    }

    /// One position back: Vim's `dec()`. From a line's start it goes to the previous line's
    /// end.
    fn dec(&mut self) -> Step {
        let line = self.current();
        if self.byte > line.start {
            let prev = prev_grapheme(self.text, self.byte);
            self.byte = if prev < self.byte {
                prev.max(line.start)
            } else {
                line.start
            };
            return Step::Within;
        }
        let Some(prev) = prev_line(self.text, line) else {
            return Step::Stuck;
        };
        self.line = prev;
        self.byte = prev.end;
        Step::Crossed
    }

    fn class(&self) -> Class {
        if self.byte >= self.current().end {
            return Class::Blank;
        }
        self.text
            .get(self.byte..)
            .and_then(|rest| rest.graphemes(true).next())
            .map_or(Class::Blank, class)
    }

    /// On an empty line, which word motions stop at as Vim's do.
    fn on_empty_line(&self) -> bool {
        self.current().is_empty()
    }
}

/// `w`, `count` times: the start of the next word, crossing lines and stopping on empty lines.
/// At the last word it goes to the text's last grapheme. Vim's `fwd_word()`.
pub(super) fn word_forward(text: &str, byte: usize, count: u32) -> usize {
    let mut walk = Walk::new(text, byte);
    'count: for _ in 0..count {
        let start = walk.class();
        let last_line = walk.on_last_line();
        match walk.inc() {
            Step::Stuck => break,
            Step::OntoLineEnd | Step::Crossed if last_line => break,
            _ => {}
        }
        if start != Class::Blank {
            while walk.class() == start {
                if walk.inc() == Step::Stuck {
                    break 'count;
                }
            }
        }
        while walk.class() == Class::Blank {
            if walk.on_empty_line() {
                break;
            }
            if walk.inc() == Step::Stuck {
                break 'count;
            }
        }
    }
    on_grapheme(text, walk.byte)
}

/// `b`, `count` times: the start of this word or the one before, crossing lines and stopping
/// on empty lines. Vim's `bck_word()`.
pub(super) fn word_backward(text: &str, byte: usize, count: u32) -> usize {
    let mut walk = Walk::new(text, byte);
    'count: for _ in 0..count {
        if walk.dec() == Step::Stuck {
            break;
        }
        let mut on_empty_line = false;
        while walk.class() == Class::Blank {
            if walk.on_empty_line() {
                on_empty_line = true;
                break;
            }
            if walk.dec() == Step::Stuck {
                break 'count;
            }
        }
        if !on_empty_line {
            let class = walk.class();
            while walk.class() == class {
                if walk.dec() == Step::Stuck {
                    break 'count;
                }
            }
            // Overshot by one.
            walk.inc();
        }
    }
    on_grapheme(text, walk.byte)
}

/// `e`, `count` times: the end of this word or the next, crossing lines. Vim's `end_word()`.
pub(super) fn word_end(text: &str, byte: usize, count: u32) -> usize {
    let mut walk = Walk::new(text, byte);
    'count: for _ in 0..count {
        let start = walk.class();
        if walk.inc() == Step::Stuck {
            break;
        }
        if walk.class() == start && start != Class::Blank {
            while walk.class() == start {
                if walk.inc() == Step::Stuck {
                    break 'count;
                }
            }
        } else {
            while walk.class() == Class::Blank {
                if walk.inc() == Step::Stuck {
                    break 'count;
                }
            }
            let class = walk.class();
            while walk.class() == class {
                if walk.inc() == Step::Stuck {
                    break 'count;
                }
            }
        }
        // Overshot by one.
        walk.dec();
    }
    on_grapheme(text, walk.byte)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where `motion` goes, `steps` times from `from`.
    fn walk(
        text: &str,
        from: usize,
        motion: impl Fn(&str, usize) -> usize,
        steps: usize,
    ) -> Vec<usize> {
        let mut at = from;
        let mut seen = Vec::new();
        for _ in 0..steps {
            at = motion(text, at);
            seen.push(at);
        }
        seen
    }

    /// The lines of `text` from splitting all of it into graphemes: the oracle for
    /// [`line_at`], which looks only around its byte.
    fn lines(text: &str) -> Vec<Line> {
        let mut lines = Vec::new();
        let mut start = 0;
        for (i, g) in text.grapheme_indices(true) {
            if is_break(g) {
                lines.push(Line { start, end: i });
                start = i + g.len();
            }
        }
        lines.push(Line {
            start,
            end: text.len(),
        });
        lines
    }

    #[test]
    fn lines_break_where_the_board_counts_breaks() {
        let l = |start, end| Line { start, end };
        assert_eq!(lines(""), [l(0, 0)]);
        assert_eq!(lines("ab\ncd"), [l(0, 2), l(3, 5)]);
        assert_eq!(lines("a\r\nb\rc\n"), [l(0, 1), l(3, 4), l(5, 6), l(7, 7)]);
        assert_eq!(lines("a\u{2028}b"), [l(0, 1), l(4, 5)]);
        assert_eq!(line_at("ab\ncd", 2), l(0, 2));
        assert_eq!(line_at("ab\ncd", 3), l(3, 5));
    }

    #[test]
    fn line_at_finds_the_line_that_splitting_the_text_finds() {
        let texts = [
            "",
            "abc",
            "ab\ncd\n",
            "\n\n",
            "a\r\nb\rc\n\r\n",
            "\u{B}x\u{C}y\u{85}z\u{2028}\u{2029}é\u{301}",
            "\r\n\u{301}q\n\u{1F468}\u{200D}\u{1F469}",
        ];
        for text in texts {
            let all = lines(text);
            for byte in 0..=text.len() + 1 {
                let floor = text.floor_char_boundary(byte);
                let expected = all
                    .iter()
                    .copied()
                    .find(|l| floor <= l.end)
                    .unwrap_or(all[all.len() - 1]);
                assert_eq!(line_at(text, byte), expected, "{text:?} at {byte}");
            }
        }
    }

    #[test]
    fn graphemes_step_whole() {
        // `e` + combining acute, then a family emoji joined with ZWJ.
        let text = "e\u{301}\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}x";
        assert_eq!(next_grapheme(text, 0), 3);
        assert_eq!(next_grapheme(text, 3), 21);
        assert_eq!(prev_grapheme(text, 21), 3);
        assert_eq!(prev_grapheme(text, 3), 0);
        assert_eq!(prev_grapheme(text, 0), 0);
        assert_eq!(next_grapheme(text, text.len()), text.len());
        assert_eq!(floor_boundary(text, 2), 0);
        assert_eq!(ceil_boundary(text, 2), 3);
        assert_eq!(ceil_boundary(text, 1), 3);
        assert_eq!(ceil_boundary(text, 3), 3);
        assert_eq!(ceil_boundary(text, 7), 21);
        assert_eq!(ceil_boundary(text, 99), text.len());
        assert_eq!(column(text, 21), 2);
    }

    #[test]
    fn block_positions_stay_on_a_grapheme() {
        assert_eq!(on_grapheme("abc", 3), 2);
        assert_eq!(on_grapheme("ab\n\ncd", 3), 3);
        assert_eq!(on_grapheme("ab\ncd", 2), 1);
        assert_eq!(on_grapheme("", 0), 0);
        assert_eq!(after_insert("abc", 3), 2);
        assert_eq!(after_insert("ab\ncd", 3), 3);
    }

    #[test]
    fn vertical_moves_keep_the_column() {
        let text = "abcd\nx\nabc";
        assert_eq!(vertical(text, 3, 1, false, 3, false), Some(5));
        assert_eq!(vertical(text, 5, 1, false, 3, false), Some(9));
        assert_eq!(vertical(text, 5, 1, false, 3, true), Some(10));
        assert_eq!(vertical(text, 10, 9, true, 3, false), Some(3));
        assert_eq!(vertical(text, 10, 1, false, 3, false), None);
        assert_eq!(vertical(text, 0, 1, true, 0, false), None);
        // Past the end in Insert mode.
        assert_eq!(vertical(text, 0, 2, false, 9, true), Some(10));
    }

    #[test]
    fn words_follow_vim() {
        // Positions:   0123456789012345
        let text = "foo.bar  baz\n\n  q_1";
        assert_eq!(
            walk(text, 0, |t, b| word_forward(t, b, 1), 6),
            [3, 4, 9, 13, 16, 18]
        );
        assert_eq!(
            walk(text, 18, |t, b| word_backward(t, b, 1), 6),
            [16, 13, 9, 4, 3, 0]
        );
        assert_eq!(
            walk(text, 0, |t, b| word_end(t, b, 1), 5),
            [2, 3, 6, 11, 18]
        );
        assert_eq!(word_forward(text, 0, 4), 13);
        assert_eq!(word_backward(text, 0, 1), 0);
        // Blanks at the end: `w` stops on the last grapheme.
        assert_eq!(word_forward("ab  ", 0, 1), 3);
    }
}
