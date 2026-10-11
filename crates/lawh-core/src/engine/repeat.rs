//! Recording changes for `.`.
//!
//! A change is recorded as the keys that made it, without counts, plus the count it ran with
//! (both counts of `2d3w` multiplied: 6). Recording starts at the first key of a Normal command
//! and ends when the command completes: a command that changed the board keeps the recording
//! as the last change ([`Repeat::finish`]); any other command leaves the last change alone. A
//! command that goes on in another mode (an insert session) keeps recording the keys typed
//! there until it ends, then finishes.
//!
//! `.` replays the keys through the engine's key dispatch (after mappings, so mappings do not
//! apply again), with its own count in place of the recorded one if it has one. The replay
//! records itself, so the new count sticks for the next `.`, as in Vim.
//!
//! A change made inside a text block (block-Normal `x`) is only repeated inside a block, and a
//! board change (`x`, `dd`, `p`, `o`) only on the board: `x` in a block deletes a character, on
//! the board a whole object. An insert session entered with `i`, `a`, `I` or `A` is repeated
//! in either place, since those keys start typing in both.
//!
//! A change that types in Insert mode stops its replay when its first key does not start
//! Insert mode (`i` on a rectangle, `o` with no room, a full board): the keys typed after it
//! would run as commands.

use crate::key::{Key, KeyCode};

/// Most keys a change keeps for `.`, and most keys a count on an insert (`3ia<Esc>`) replays in
/// all.
///
/// An insert session records every key typed in it, so a long one could hold any number of
/// keys. The largest text worth typing in one go is [`MAX_TEXT_CHARS`](crate::board::MAX_TEXT_CHARS)
/// characters, and this leaves as much again for moving and correcting. A longer session cannot
/// be repeated (`.` then does nothing), and a count stops replaying once it would pass this,
/// which keeps `10000i` before a long session cheap.
pub const MAX_REPEAT_KEYS: usize = 10_000;

/// A change `.` can repeat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Change {
    /// The count the change ran with, `None` when none was typed.
    pub(crate) count: Option<u32>,
    /// Its keys, without counts.
    pub(crate) keys: Vec<Key>,
    /// Where `.` repeats it.
    pub(crate) context: Context,
    /// Its first key enters Insert mode and the rest are typed there.
    pub(crate) insert: bool,
}

/// Where a change can be repeated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Context {
    /// In Normal mode, on the board.
    Board,
    /// In block-Normal mode, inside a text block.
    Block,
    /// In either.
    Anywhere,
}

impl Change {
    /// The keys to replay: `count` (or the change's own) as digits, then the change's keys.
    /// For a change that types in Insert mode, also how many of them run before Insert mode must
    /// have started: the digits and the first key.
    pub(crate) fn replay(&self, count: Option<u32>) -> (Vec<Key>, Option<usize>) {
        let digits = count
            .or(self.count)
            .map(|n| n.to_string())
            .unwrap_or_default();
        let entry = self.insert.then_some(digits.len() + 1);
        let keys = digits
            .chars()
            .map(|c| Key::named(KeyCode::Char(c)))
            .chain(self.keys.iter().copied())
            .collect();
        (keys, entry)
    }
}

/// The last change and the one being recorded.
#[derive(Debug, Clone, Default)]
pub(crate) struct Repeat {
    last: Option<Change>,
    /// Keys of the command being typed, without counts.
    recording: Vec<Key>,
    /// The command being typed started in block-Normal mode.
    recording_in_block: bool,
    /// The command being typed has more than [`MAX_REPEAT_KEYS`] keys.
    too_long: bool,
    /// `.` is replaying the last change: another `.` inside it does nothing.
    pub(crate) replaying: bool,
}

impl Repeat {
    /// A new command starts on the board: forget the keys recorded so far.
    pub(crate) fn start(&mut self) {
        self.recording.clear();
        self.recording_in_block = false;
        self.too_long = false;
    }

    /// A new command starts inside a text block.
    pub(crate) fn start_in_block(&mut self) {
        self.start();
        self.recording_in_block = true;
    }

    /// Records a key of the command being typed (not a count digit), up to
    /// [`MAX_REPEAT_KEYS`].
    pub(crate) fn push(&mut self, key: Key) {
        if self.recording.len() < MAX_REPEAT_KEYS {
            self.recording.push(key);
        } else {
            self.too_long = true;
        }
    }

    /// The command changed the board: its keys, run with `count`, become the last change, to
    /// repeat where the command started. A command too long to keep leaves nothing to repeat.
    pub(crate) fn finish(&mut self, count: Option<u32>) {
        let context = if self.recording_in_block {
            Context::Block
        } else {
            Context::Board
        };
        self.finish_change(count, context, false);
    }

    /// An insert session changed the board: [`Repeat::finish`] for a change whose first key
    /// enters Insert mode, repeated in `context`.
    pub(crate) fn finish_insert(&mut self, count: Option<u32>, context: Context) {
        self.finish_change(count, context, true);
    }

    fn finish_change(&mut self, count: Option<u32>, context: Context, insert: bool) {
        let keys = std::mem::take(&mut self.recording);
        self.last = (!self.too_long).then_some(Change {
            count,
            keys,
            context,
            insert,
        });
    }

    /// Forgets the last change: a change was made that `.` cannot repeat.
    pub(crate) fn forget(&mut self) {
        self.last = None;
        self.recording.clear();
    }

    /// Makes `change` the last change again, dropping what was recorded since: for a `.` that
    /// was cut short (see [`MAX_KEY_WORK`](crate::engine::MAX_KEY_WORK)).
    pub(crate) fn restore(&mut self, change: Change) {
        self.last = Some(change);
        self.recording.clear();
    }

    pub(crate) fn last(&self) -> Option<&Change> {
        self.last.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::parse_keys;

    #[test]
    fn a_count_replaces_the_recorded_one() {
        let mut repeat = Repeat::default();
        repeat.start();
        for key in parse_keys("dw").unwrap() {
            repeat.push(key);
        }
        repeat.finish(Some(6));
        let change = repeat.last().unwrap();
        assert_eq!(change.replay(None), (parse_keys("6dw").unwrap(), None));
        assert_eq!(change.replay(Some(12)), (parse_keys("12dw").unwrap(), None));
        // An insert: Insert mode must have started after the digits and the first key.
        repeat.start();
        for key in parse_keys("oab<Esc>").unwrap() {
            repeat.push(key);
        }
        repeat.finish_insert(Some(3), Context::Board);
        let change = repeat.last().unwrap();
        assert_eq!(
            change.replay(None),
            (parse_keys("3oab<Esc>").unwrap(), Some(2))
        );
        assert_eq!(change.replay(Some(12)).1, Some(3));
        repeat.forget();
        assert_eq!(repeat.last(), None);
    }

    #[test]
    fn a_change_past_max_repeat_keys_is_not_kept() {
        let mut repeat = Repeat::default();
        repeat.start_in_block();
        repeat.push(Key::char('x'));
        repeat.finish(None);
        assert!(repeat.last().is_some_and(|c| c.context == Context::Block));
        repeat.start();
        for _ in 0..=MAX_REPEAT_KEYS {
            repeat.push(Key::char('a'));
        }
        repeat.finish(None);
        assert_eq!(repeat.last(), None);
    }
}
