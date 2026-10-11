//! Editing the `:` command line: typing, `<BS>`, `<Left>` `<Right>` `<Home>` `<End>`, `<Esc>`
//! and `<CR>`.

use crate::board::MAX_TEXT_CHARS;
use crate::key::{Key, KeyCode};

/// Most characters on the `:` command line, the `:` not counted.
///
/// The longest line worth typing is `:text` with a text of
/// [`MAX_TEXT_CHARS`] characters; 64 more leave room for the
/// command's name and spaces. A longer line could only make an object the board refuses.
pub const MAX_COMMAND_LINE_CHARS: usize = MAX_TEXT_CHARS + 64;

/// What a key did to the command line.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    /// Still editing (the key may have changed the line, or done nothing).
    Editing,
    /// The line is full; the typed character was refused.
    Full,
    /// Leave the command line without running it.
    Cancel,
    /// Run this line.
    Run(String),
}

/// The text after `:` and the cursor in it, as a byte offset on a character boundary.
#[derive(Debug, Clone, Default)]
pub(super) struct CommandLine {
    text: String,
    cursor: usize,
    /// Characters in `text`, for [`MAX_COMMAND_LINE_CHARS`].
    chars: usize,
}

impl CommandLine {
    pub(super) fn text(&self) -> &str {
        &self.text
    }

    pub(super) fn cursor(&self) -> usize {
        self.cursor
    }

    /// Empties the line, to start typing a new command.
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(super) fn key(&mut self, key: Key) -> Outcome {
        let plain = key.mods.is_empty();
        match key.code {
            KeyCode::Char(c) if is_typed(key) => self.insert(c),
            KeyCode::Esc if plain => Outcome::Cancel,
            KeyCode::Enter if plain => {
                let line = std::mem::take(&mut self.text);
                self.clear();
                Outcome::Run(line)
            }
            KeyCode::Backspace if plain => self.backspace(),
            KeyCode::Left if plain => {
                self.cursor = self.before_cursor().map_or(0, |(i, _)| i);
                Outcome::Editing
            }
            KeyCode::Right if plain => {
                self.cursor = self.after_cursor().unwrap_or(self.text.len());
                Outcome::Editing
            }
            KeyCode::Home if plain => {
                self.cursor = 0;
                Outcome::Editing
            }
            KeyCode::End if plain => {
                self.cursor = self.text.len();
                Outcome::Editing
            }
            _ => Outcome::Editing,
        }
    }

    fn insert(&mut self, c: char) -> Outcome {
        if self.chars >= MAX_COMMAND_LINE_CHARS {
            return Outcome::Full;
        }
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
        self.chars += 1;
        Outcome::Editing
    }

    /// Deletes the character before the cursor. On an empty line it leaves, as in Vim.
    fn backspace(&mut self) -> Outcome {
        if self.text.is_empty() {
            return Outcome::Cancel;
        }
        if let Some((i, _)) = self.before_cursor() {
            self.text.remove(i);
            self.cursor = i;
            self.chars -= 1;
        }
        Outcome::Editing
    }

    /// The character before the cursor and its offset.
    fn before_cursor(&self) -> Option<(usize, char)> {
        self.text.get(..self.cursor)?.char_indices().next_back()
    }

    /// The offset after the character at the cursor.
    fn after_cursor(&self) -> Option<usize> {
        let c = self.text.get(self.cursor..)?.chars().next()?;
        Some(self.cursor + c.len_utf8())
    }
}

/// A key that types its character: no Ctrl, Alt or Cmd, and not a control character.
fn is_typed(key: Key) -> bool {
    let typed = matches!(key.code, KeyCode::Char(c) if !c.is_control());
    typed && !(key.mods.ctrl || key.mods.alt || key.mods.cmd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::parse_keys;

    /// Types `script` on an empty line: the outcome of the last key, the text and the cursor.
    fn edit(script: &str) -> (Outcome, String, usize) {
        let mut line = CommandLine::default();
        let mut last = Outcome::Editing;
        for key in parse_keys(script).unwrap() {
            last = line.key(key);
        }
        assert_eq!(line.chars, line.text.chars().count());
        (last, line.text, line.cursor)
    }

    #[test]
    fn typing_and_moving_work_on_characters() {
        let editing = |text: &str, cursor| (Outcome::Editing, text.to_owned(), cursor);
        assert_eq!(edit("ci 3"), editing("ci 3", 4));
        assert_eq!(edit("é2<Left>π"), editing("éπ2", 4));
        assert_eq!(edit("ab<Home>c<End>d"), editing("cabd", 4));
        assert_eq!(edit("ab<Left><Left><Left>c"), editing("cab", 1));
        assert_eq!(edit("ab<Home><Right>c<Right><Right>d"), editing("acbd", 4));
        assert_eq!(edit("aé<BS>b"), editing("ab", 2));
        assert_eq!(edit("ab<Home><BS>"), editing("ab", 0));
        assert_eq!(edit("ab<Left><BS>"), editing("b", 0));
        // Keys with Ctrl, Alt or Cmd type nothing; neither do other keys.
        assert_eq!(
            edit("a<C-b><M-c><D-d><Tab><Up><F1><S-Left>"),
            editing("a", 1)
        );
    }

    #[test]
    fn esc_and_backspace_on_empty_cancel_enter_runs() {
        assert_eq!(edit("ci 3<Esc>").0, Outcome::Cancel);
        assert_eq!(edit("<BS>").0, Outcome::Cancel);
        assert_eq!(edit("a<BS><BS>").0, Outcome::Cancel);
        assert_eq!(
            edit("ci 3<CR>"),
            (Outcome::Run("ci 3".to_owned()), String::new(), 0)
        );
    }

    #[test]
    fn the_line_holds_at_most_max_command_line_chars() {
        let mut line = CommandLine::default();
        for _ in 0..MAX_COMMAND_LINE_CHARS {
            assert_eq!(line.key(Key::char('é')), Outcome::Editing);
        }
        assert_eq!(line.key(Key::char('x')), Outcome::Full);
        assert_eq!(line.text().chars().count(), MAX_COMMAND_LINE_CHARS);
        assert_eq!(line.key(Key::named(KeyCode::Backspace)), Outcome::Editing);
        assert_eq!(line.key(Key::char('x')), Outcome::Editing);
        assert_eq!(line.key(Key::char('x')), Outcome::Full);
    }
}
