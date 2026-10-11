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

use crate::key::{Key, KeyCode};

/// A change `.` can repeat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Change {
    /// The count the change ran with, `None` when none was typed.
    pub(crate) count: Option<u32>,
    /// Its keys, without counts.
    pub(crate) keys: Vec<Key>,
}

impl Change {
    /// The keys to replay: `count` (or the change's own) as digits, then the change's keys.
    pub(crate) fn replay(&self, count: Option<u32>) -> Vec<Key> {
        let digits = count
            .or(self.count)
            .map(|n| n.to_string())
            .unwrap_or_default();
        digits
            .chars()
            .map(|c| Key::named(KeyCode::Char(c)))
            .chain(self.keys.iter().copied())
            .collect()
    }
}

/// The last change and the one being recorded.
#[derive(Debug, Clone, Default)]
pub(crate) struct Repeat {
    last: Option<Change>,
    /// Keys of the command being typed, without counts.
    recording: Vec<Key>,
    /// `.` is replaying the last change: another `.` inside it does nothing.
    pub(crate) replaying: bool,
}

impl Repeat {
    /// A new command starts: forget the keys recorded so far.
    pub(crate) fn start(&mut self) {
        self.recording.clear();
    }

    /// Records a key of the command being typed (not a count digit).
    pub(crate) fn push(&mut self, key: Key) {
        self.recording.push(key);
    }

    /// The command changed the board: its keys, run with `count`, become the last change.
    pub(crate) fn finish(&mut self, count: Option<u32>) {
        self.last = Some(Change {
            count,
            keys: std::mem::take(&mut self.recording),
        });
    }

    /// Forgets the last change: a change was made that `.` cannot repeat.
    pub(crate) fn forget(&mut self) {
        self.last = None;
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
        assert_eq!(change.replay(None), parse_keys("6dw").unwrap());
        assert_eq!(change.replay(Some(12)), parse_keys("12dw").unwrap());
        repeat.forget();
        assert_eq!(repeat.last(), None);
    }
}
