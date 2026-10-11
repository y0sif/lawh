//! Insert mode: typing into a text block.
//!
//! A typed character goes in at the cursor, with Alt held too: characters a keyboard layout
//! types with `AltGr` arrive as plain characters, never as chords, so Alt with a character is
//! taken as the character. Ctrl and Cmd chords type nothing in M2. `<CR>` breaks the line with `\n`, `<Tab>` types a tab. `<BS>` and
//! `<Del>` delete a grapheme before or after the cursor, joining lines at a line break.
//! `<Left>` and `<Right>` move by a grapheme within the line, `<Up>` and `<Down>` by a line
//! keeping the column, `<Home>` and `<End>` to the line's ends. `<Esc>` ends the session (see
//! `text.rs`). Other keys do nothing.

use super::text_motion::{
    ceil_boundary, column, content, left, line_end, line_start, next_grapheme, prev_grapheme,
    right, vertical, with_content,
};
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
    /// message saying why.
    pub(super) fn insert_edit(&mut self, key: Key) -> bool {
        let action = match (&self.block, self.block_text()) {
            (Some(block), Some(text)) => action(key, text, block.byte),
            _ => Action::Nothing,
        };
        match action {
            Action::Splice { from, to, text } => self.splice(from, to, &text),
            Action::Move(byte) => {
                if let Some(block) = &mut self.block {
                    block.byte = byte;
                    block.want = None;
                }
                true
            }
            Action::Line { up } => {
                self.line_up_down(up, 1, true);
                true
            }
            Action::Nothing => true,
        }
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
        if let Err(error) = self.board.replace(&id, new) {
            self.error(exec::refusal(&error));
            return false;
        }
        self.version = self.version.wrapping_add(1);
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

/// What `key` does in Insert mode with the cursor at `byte` of `text`.
fn action(key: Key, text: &str, byte: usize) -> Action {
    let mods = key.mods;
    let chord = mods.ctrl || mods.cmd;
    // Shift is part of a typed character already; on these keys it changes nothing.
    let plain = !(chord || mods.alt);
    let insert = |s: &str| Action::Splice {
        from: byte,
        to: byte,
        text: s.to_owned(),
    };
    match key.code {
        KeyCode::Char(c) if !chord && !c.is_control() => insert(c.encode_utf8(&mut [0; 4])),
        KeyCode::Enter if plain => insert("\n"),
        KeyCode::Tab if mods.is_empty() => insert("\t"),
        KeyCode::Backspace if plain => delete(prev_grapheme(text, byte), byte),
        KeyCode::Delete if plain => delete(byte, next_grapheme(text, byte)),
        KeyCode::Left if mods.is_empty() => Action::Move(left(text, byte, 1)),
        KeyCode::Right if mods.is_empty() => Action::Move(right(text, byte, 1, true)),
        KeyCode::Up if mods.is_empty() => Action::Line { up: true },
        KeyCode::Down if mods.is_empty() => Action::Line { up: false },
        KeyCode::Home if mods.is_empty() => Action::Move(line_start(text, byte)),
        KeyCode::End if mods.is_empty() => Action::Move(line_end(text, byte)),
        _ => Action::Nothing,
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
