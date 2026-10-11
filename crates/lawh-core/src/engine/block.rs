//! Block-Normal mode: Vim's Normal mode inside a text block.
//!
//! The cursor sits on a grapheme, never past the last one of its line (on an empty line, at
//! its start). `h l` move by graphemes within the line, `j k` by lines keeping the column, `0`
//! and `$` to the line's ends, `w b e` by Vim's words across lines, all with counts. `x`
//! deletes graphemes into the unnamed register. `i a I A` enter Insert mode, `<Esc>` goes back
//! to the board with the block as the current object. `u`, `<C-r>`, `.` and `:` work as on the
//! board; the command line comes back here unless its command moved the cursor off the block.
//! Other keys do nothing.

use super::normal::{Lookup, Pending, lookup};
use super::register::Register;
use super::text_motion::{
    first_non_blank, last_of_line, left, line_end, line_start, next_grapheme, on_grapheme, right,
    word_backward, word_end, word_forward,
};
use super::undo::Edit;
use super::work::TextPosition;
use super::{Effect, Engine, Measure, Mode};
use crate::key::{Key, KeyCode};

/// Where `i a I A` start typing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InsertAt {
    /// `i`: before the cursor's grapheme.
    Cursor,
    /// `a`: after it.
    After,
    /// `I`: before the line's first grapheme that is not blank.
    FirstNonBlank,
    /// `A`: at the line's end.
    LineEnd,
}

/// A block-Normal command, run with a count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockCommand {
    Left,
    Right,
    Down,
    Up,
    LineStart,
    LineEnd,
    WordNext,
    WordBack,
    WordEnd,
    /// `x`.
    Delete,
    Insert(InsertAt),
    /// `<Esc>`: back to the board.
    Leave,
    Undo,
    Redo,
    /// `.`.
    Repeat,
    CommandLine,
}

/// Block-Normal commands by their keys, after any count.
const BLOCK: &[(&[Key], BlockCommand)] = &[
    (&[Key::char('h')], BlockCommand::Left),
    (&[Key::char('l')], BlockCommand::Right),
    (&[Key::char('j')], BlockCommand::Down),
    (&[Key::char('k')], BlockCommand::Up),
    (&[Key::char('0')], BlockCommand::LineStart),
    (&[Key::char('$')], BlockCommand::LineEnd),
    (&[Key::char('w')], BlockCommand::WordNext),
    (&[Key::char('b')], BlockCommand::WordBack),
    (&[Key::char('e')], BlockCommand::WordEnd),
    (&[Key::char('x')], BlockCommand::Delete),
    (&[Key::char('i')], BlockCommand::Insert(InsertAt::Cursor)),
    (&[Key::char('a')], BlockCommand::Insert(InsertAt::After)),
    (
        &[Key::char('I')],
        BlockCommand::Insert(InsertAt::FirstNonBlank),
    ),
    (&[Key::char('A')], BlockCommand::Insert(InsertAt::LineEnd)),
    (&[Key::named(KeyCode::Esc)], BlockCommand::Leave),
    (&[Key::char('u')], BlockCommand::Undo),
    (&[Key::ctrl('r')], BlockCommand::Redo),
    (&[Key::char('.')], BlockCommand::Repeat),
    (&[Key::char(':')], BlockCommand::CommandLine),
];

impl Engine {
    pub(super) fn block_key(&mut self, key: Key, measure: &mut dyn Measure) -> Vec<Effect> {
        if self.pending.is_empty() {
            self.repeat.start_in_block();
        }
        if self.pending.count_digit(key) {
            return Vec::new();
        }
        self.repeat.push(key);
        let count = self.pending.count.take();
        self.pending = Pending::default();
        match lookup(BLOCK, &[key]) {
            Lookup::Found(command) => self.run_block(command, count, measure),
            Lookup::Prefix | Lookup::Nothing => Vec::new(),
        }
    }

    /// Runs a block-Normal command. Those that go through the block's text (motions, `x`,
    /// `i a I A`) do nothing once the key has run out of work, and count what they went
    /// through after they ran (see [`MAX_KEY_WORK`](super::MAX_KEY_WORK)).
    fn run_block(
        &mut self,
        command: BlockCommand,
        count: Option<u32>,
        measure: &mut dyn Measure,
    ) -> Vec<Effect> {
        let walks_text = !matches!(
            command,
            BlockCommand::Leave
                | BlockCommand::Undo
                | BlockCommand::Redo
                | BlockCommand::Repeat
                | BlockCommand::CommandLine
        );
        if !walks_text {
            return self.run_block_command(command, count, measure);
        }
        if self.out_of_work() {
            return Vec::new();
        }
        let Some(before) = self.text_position() else {
            return self.run_block_command(command, count, measure);
        };
        let effects = self.run_block_command(command, count, measure);
        let after = self.text_position();
        let moved = after.map_or(0, |a| a.byte.abs_diff(before.byte));
        let line = |p: TextPosition| p.line.end - p.line.start;
        let lines = line(before).saturating_add(after.map_or(0, line));
        // Bytes walked a grapheme at a time, and bytes split into graphemes.
        let (steps, split) = match command {
            BlockCommand::Left if count.unwrap_or(1) <= 1 => (moved, 0),
            BlockCommand::Left => (0, before.byte - before.line.start),
            BlockCommand::Right | BlockCommand::WordNext => (moved, 0),
            BlockCommand::WordBack | BlockCommand::WordEnd => (moved, lines.saturating_add(moved)),
            BlockCommand::Down | BlockCommand::Up => (0, lines),
            BlockCommand::Delete => {
                let deleted = after.map_or(0, |a| a.len.abs_diff(before.len));
                (moved.saturating_add(deleted), 0)
            }
            BlockCommand::Insert(InsertAt::FirstNonBlank) => (0, line(before)),
            _ => (0, 0),
        };
        let work = self.text_work(before, steps, split);
        self.charge(work);
        effects
    }

    fn run_block_command(
        &mut self,
        command: BlockCommand,
        count: Option<u32>,
        measure: &mut dyn Measure,
    ) -> Vec<Effect> {
        let n = count.unwrap_or(1);
        match command {
            BlockCommand::Left => self.block_move(|t, b| left(t, b, n)),
            BlockCommand::Right => self.block_move(|t, b| right(t, b, n, false)),
            BlockCommand::Down => self.line_up_down(false, n, false),
            BlockCommand::Up => self.line_up_down(true, n, false),
            BlockCommand::LineStart => self.block_move(line_start),
            BlockCommand::LineEnd => {
                self.block_move(|t, b| last_of_line(t, b, n));
                if let Some(block) = &mut self.block {
                    block.want = Some(usize::MAX);
                }
            }
            BlockCommand::WordNext => self.block_move(|t, b| word_forward(t, b, n)),
            BlockCommand::WordBack => self.block_move(|t, b| word_backward(t, b, n)),
            BlockCommand::WordEnd => self.block_move(|t, b| word_end(t, b, n)),
            BlockCommand::Delete => {
                if self.delete_graphemes(n) {
                    self.repeat.finish(count);
                }
            }
            BlockCommand::Insert(at) => {
                let byte = self.block_text().zip(self.block.as_ref()).map(|(text, b)| {
                    let line = (line_start(text, b.byte), line_end(text, b.byte));
                    match at {
                        InsertAt::Cursor => b.byte,
                        InsertAt::After => next_grapheme(text, b.byte).min(line.1),
                        InsertAt::FirstNonBlank => first_non_blank(text, b.byte),
                        InsertAt::LineEnd => line.1,
                    }
                });
                if let Some(byte) = byte {
                    self.insert_in_block(byte, count);
                }
            }
            BlockCommand::Leave => {
                self.block = None;
                self.mode = Mode::Normal;
            }
            BlockCommand::Undo => self.undo(n),
            BlockCommand::Redo => self.redo(n),
            BlockCommand::Repeat => return self.repeat_change(count, measure),
            BlockCommand::CommandLine => self.open_command_line(),
        }
        Vec::new()
    }

    /// Moves the block cursor to where `to` says from its text and byte, on a grapheme, and
    /// forgets the wanted column.
    fn block_move(&mut self, to: impl FnOnce(&str, usize) -> usize) {
        let moved = self
            .block_text()
            .zip(self.block.as_ref())
            .map(|(text, b)| on_grapheme(text, to(text, b.byte)));
        if let (Some(byte), Some(block)) = (moved, &mut self.block) {
            block.byte = byte;
            block.want = None;
        }
    }

    /// `x`: deletes `count` graphemes from the cursor, up to the end of its line, into the
    /// unnamed register, as one undo step. True when the text changed.
    fn delete_graphemes(&mut self, count: u32) -> bool {
        let Some((from, to, deleted)) =
            self.block_text().zip(self.block.as_ref()).map(|(text, b)| {
                let end = line_end(text, b.byte);
                let mut to = b.byte;
                for _ in 0..count {
                    if to >= end {
                        break;
                    }
                    to = next_grapheme(text, to).min(end);
                }
                let deleted = text.get(b.byte..to).unwrap_or_default().to_owned();
                (b.byte, to, deleted)
            })
        else {
            return false;
        };
        let Some(block) = &self.block else {
            return false;
        };
        let id = block.id.clone();
        let Some(before) = self.board.get(&id).map(|o| o.shape.clone()) else {
            return false;
        };
        if from == to {
            return false;
        }
        let cursor = self.cursor.clone();
        if !self.splice(from, to, "") {
            return false;
        }
        self.register = Some(Register::Text(deleted));
        let byte = self
            .block_text()
            .map_or(from, |text| on_grapheme(text, from));
        if let Some(block) = &mut self.block {
            block.byte = byte;
        }
        let Some(after) = self.board.get(&id).map(|o| o.shape.clone()) else {
            return false;
        };
        let edit = Edit::Replace { id, before, after };
        self.commit_at(vec![edit], cursor, Some(from), Some(byte));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_key_is_bound_twice() {
        for (i, (keys, _)) in BLOCK.iter().enumerate() {
            assert!(
                BLOCK.iter().skip(i + 1).all(|(other, _)| other != keys),
                "{keys:?}"
            );
        }
    }
}
