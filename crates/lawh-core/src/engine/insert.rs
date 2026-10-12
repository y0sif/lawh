//! Insert mode: typing into a text block.
//!
//! A typed character goes in at the cursor, with Alt held too: characters a keyboard layout
//! types with `AltGr` arrive as plain characters, never as chords, so Alt with a character is
//! taken as the character. `<C-k>` opens a `:sym` line, to pick a symbol by name (see
//! `sym.rs`); other Ctrl and Cmd chords type nothing. `<CR>` breaks the line with `\n`, `<Tab>`
//! types a tab. `<BS>` and `<Del>` delete a grapheme before or after the cursor, joining lines
//! at a line break.
//! `<Left>` and `<Right>` move by a grapheme within the line, `<Up>` and `<Down>` by a line
//! keeping the column, `<Home>` and `<End>` to the line's ends. `<Esc>` ends the session (see
//! `text.rs`). Other keys do nothing.

use super::sym::SYM_KEY;
use super::text_motion::{
    ceil_boundary, column, content, indicators_before, left, line_at, next_grapheme, prev_grapheme,
    right, vertical, with_content,
};
use super::work::{COPY_WORK, SCAN_WORK, STEP_WORK};
use super::{Effect, Engine, Measure, exec};
use crate::key::{Key, KeyCode};

/// What an Insert-mode key does to the block.
enum Action {
    /// Replace bytes `from..to` with the text, then put the cursor after it.
    Splice {
        from: usize,
        to: usize,
        text: String,
    },
    /// Put the cursor at this byte.
    Move(usize),
    /// `<Up>` or `<Down>` (`up`): a line, keeping the wanted column.
    Line {
        up: bool,
    },
    Nothing,
}

impl Engine {
    pub(super) fn insert_key(&mut self, key: Key, measure: &mut dyn Measure) -> Vec<Effect> {
        if key == SYM_KEY {
            // Not recorded: the pick records the characters it puts in.
            self.open_sym_line();
            return Vec::new();
        }
        self.repeat.push(key);
        if key.code == KeyCode::Esc {
            self.end_session(true, measure);
        } else {
            self.keep_session_key(key);
            self.insert_edit(key);
        }
        Vec::new()
    }

    /// Runs Insert-mode `key` on the block. False when the board refused the edit, with a
    /// message saying why, or when the key runs out of work (see
    /// [`MAX_KEY_WORK`](super::MAX_KEY_WORK)): what the key went through of the block's text
    /// counts after it ran.
    pub(super) fn insert_edit(&mut self, key: Key) -> bool {
        if self.out_of_work() {
            return false;
        }
        let (action, work) = match (&self.block, self.block_text()) {
            (Some(block), Some(text)) => action(key, text, block.byte),
            _ => (Action::Nothing, 0),
        };
        let done = match action {
            Action::Splice { from, to, text } => self.splice(from, to, &text),
            Action::Move(byte) => {
                if let Some(block) = &mut self.block {
                    block.byte = byte;
                    block.want = None;
                }
                true
            }
            Action::Line { up } => {
                // `column` and `at_column` split both lines into graphemes.
                if let Some(before) = self.text_position() {
                    self.line_up_down(up, 1, true);
                    let after = self
                        .text_position()
                        .map_or(0, |p| p.line.end - p.line.start);
                    let split = (before.line.end - before.line.start).saturating_add(after);
                    let work = self.text_work(before, 0, split);
                    self.charge(work);
                }
                true
            }
            Action::Nothing => true,
        };
        self.charge(work) && done
    }

    /// Replaces bytes `from..to` of the block's text with `insert` and puts the cursor after
    /// it. The board checks the caps on text: when it refuses, nothing changes, the message
    /// says why, and the result is false.
    pub(super) fn splice(&mut self, from: usize, to: usize, insert: &str) -> bool {
        let Some(block) = &self.block else {
            return true;
        };
        let id = block.id.clone();
        let Some(shape) = self.board.get(&id).map(|o| &o.shape) else {
            return true;
        };
        let text = content(shape).unwrap_or_default();
        let (Some(head), Some(tail)) = (text.get(..from), text.get(to..)) else {
            return true;
        };
        let Some(new) = with_content(shape, [head, insert, tail].concat()) else {
            return true;
        };
        // Copying the text, and the board counting it again (see `MAX_KEY_WORK`).
        let copied = new_len(&new);
        self.charge(COPY_WORK.saturating_mul(copied));
        if let Err(error) = self.board.replace(&id, new) {
            self.error(exec::refusal(&error));
            // A text past a cap may be counted again a character at a time, to tell which.
            self.charge(SCAN_WORK.saturating_mul(copied));
            return false;
        }
        self.changed();
        let byte = self
            .block_text()
            .map_or(from, |text| ceil_boundary(text, from + insert.len()));
        if let Some(block) = &mut self.block {
            block.byte = byte;
            block.want = None;
        }
        true
    }

    /// Moves the block cursor `count` lines up or down, keeping the wanted column; in Insert
    /// mode (`past_end`) it may stop at a line's end.
    pub(super) fn line_up_down(&mut self, up: bool, count: u32, past_end: bool) {
        let (Some(block), Some(text)) = (&self.block, self.block_text()) else {
            return;
        };
        let want = block.want.unwrap_or_else(|| column(text, block.byte));
        let moved = vertical(text, block.byte, count, up, want, past_end);
        if let Some(block) = &mut self.block {
            if let Some(byte) = moved {
                block.byte = byte;
            }
            block.want = Some(want);
        }
    }
}

/// Bytes and characters of `shape`'s text, for the work of copying it.
fn new_len(shape: &crate::board::Shape) -> usize {
    content(shape).map_or(0, |text| text.len().saturating_add(text.chars().count()))
}

/// What `key` does in Insert mode with the cursor at `byte` of `text`, and the work of finding
/// out (see [`MAX_KEY_WORK`](super::MAX_KEY_WORK)); an edit counts the copy of the text on top.
fn action(key: Key, text: &str, byte: usize) -> (Action, usize) {
    // Finding the grapheme before the cursor, or the boundary after an insert, may scan back
    // through the flag halves before it.
    let back = || SCAN_WORK.saturating_mul(indicators_before(text, byte));
    let line = || {
        let line = line_at(text, byte);
        SCAN_WORK.saturating_mul(line.end - line.start)
    };
    let mods = key.mods;
    let chord = mods.ctrl || mods.cmd;
    // Shift is part of a typed character already; on these keys it changes nothing.
    let plain = !(chord || mods.alt);
    let insert = |s: &str| Action::Splice {
        from: byte,
        to: byte,
        text: s.to_owned(),
    };
    let step = |to: usize| STEP_WORK.saturating_mul(to.abs_diff(byte));
    match key.code {
        KeyCode::Char(c) if !chord && !c.is_control() => {
            (insert(c.encode_utf8(&mut [0; 4])), back())
        }
        KeyCode::Enter if plain => (insert("\n"), back()),
        KeyCode::Tab if mods.is_empty() => (insert("\t"), back()),
        KeyCode::Backspace if plain => (delete(prev_grapheme(text, byte), byte), back()),
        KeyCode::Delete if plain => {
            let to = next_grapheme(text, byte);
            (delete(byte, to), back().saturating_add(step(to)))
        }
        KeyCode::Left if mods.is_empty() => {
            let to = left(text, byte, 1);
            (Action::Move(to), back().saturating_add(step(to)))
        }
        KeyCode::Right if mods.is_empty() => {
            let to = right(text, byte, 1, true);
            (Action::Move(to), step(to))
        }
        KeyCode::Up if mods.is_empty() => (Action::Line { up: true }, 0),
        KeyCode::Down if mods.is_empty() => (Action::Line { up: false }, 0),
        KeyCode::Home if mods.is_empty() => (Action::Move(line_at(text, byte).start), line()),
        KeyCode::End if mods.is_empty() => (Action::Move(line_at(text, byte).end), line()),
        _ => (Action::Nothing, 0),
    }
}

/// Deletes bytes `from..to`; nothing when they are empty (at the start or end of the text).
fn delete(from: usize, to: usize) -> Action {
    if from < to {
        Action::Splice {
            from,
            to,
            text: String::new(),
        }
    } else {
        Action::Nothing
    }
}
