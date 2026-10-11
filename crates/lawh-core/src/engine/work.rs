//! The work one typed key may do: [`MAX_KEY_WORK`].
//!
//! Mappings, `.` and counts let one key run a great many commands, and
//! [`MAX_EXPANDED_KEYS`](super::MAX_EXPANDED_KEYS) bounds only how many keys that is, not what
//! each costs: a mapping that runs `10000J10000K` twenty times, then itself, walked a column of
//! 500 objects for 7.6 s in release, and one that runs `$10000h` on a line of 5,000 flag halves
//! for 227 s. So commands count the work they do as they go, each kind of work at what it costs
//! (the `*_WORK` weights below): objects measured, put in reading order, looked at by
//! `H J K L` and the place rule, taken, put, and edited by undo and redo, fresh ids looked for,
//! the bytes of `:` lines run, and the bytes of a text block that a key copies, scans, splits
//! into graphemes or walks.
//!
//! Counts never multiply a command that makes no progress: motions stop where they cannot go
//! on, undo and redo at the ends of the history, a put that does not fit is refused whole (before
//! the register is copied), and a count on an insert stops once a time leaves the block as it
//! was. What is left to bound is work that does go somewhere, over and over.
//!
//! When a key's work passes the budget, the command stops where it is, the board as consistent
//! as after any command: a motion stops on the last object it reached, an operator or a put does
//! nothing, an undo with a count stops after the steps it made, an Insert session that `.` was
//! typing ends as `<Esc>` would end it (one undo step) and `.` keeps the whole change. A command
//! in a text block counts its work once it has run, so it may pass the budget by its own work,
//! which is at most a few milliseconds. Nothing more runs for that key: the typeahead is dropped
//! and the message says so. The budget starts again with the next typed key, timeout or click.

use super::Engine;
use super::text_motion::{Line, indicators_before, line_at};

/// Most work one typed key (or a timeout) may do: 100,000,000 units, a unit being about a
/// nanosecond of work in a debug build of lawh-core.
///
/// Each weight below is what one item of its kind of work took in a debug build, measured with
/// `work_unit_costs` in `tests/engine_cost.rs` (Linux `x86_64`, 2026-10-12, measuring with
/// [`FixedMeasure`](super::FixedMeasure)). Over its cases a unit took 0.1 to 1.1 ns in debug
/// and at most 0.1 ns in release (0.5 ns for typing, which is copying), so a key that runs out
/// takes about 0.1 s in debug and 10 ms in release (50 ms if it was typing). Keys that do no
/// work are bounded by [`MAX_EXPANDED_KEYS`](super::MAX_EXPANDED_KEYS) instead: 30,000 of them
/// take 36 ms in debug.
///
/// The costliest single commands fit: `10000J` down a column of 500 objects (62.5 million units,
/// 51 ms in debug and 1.8 ms in release), `.` of an insert of 5,000 characters (25 million) and
/// `10000ia<Esc>` up to [`MAX_TEXT_CHARS`](crate::board::MAX_TEXT_CHARS), `dG`, `yG` and `p` of a
/// full board (3 million at most). An undo or redo step of a whole board costs 1 to 1.5
/// million, so `10000u` over a history of such steps stops after about 80 of them (60 ms in
/// debug, 7 ms in release); the next key goes on.
pub const MAX_KEY_WORK: usize = 100_000_000;

// Weights: units for one item of each kind of work, from `work_unit_costs` in debug.

/// Measuring one object, or one character of text or math, for the board's bounds: `w` over 500
/// texts of 80 characters measured 40,500 in 0.8 ms.
pub(super) const MEASURE_WORK: usize = 20;
/// Putting one object in reading order (`w`, `b`, `gg`, `G`, the operators): `w` over 500
/// rectangles took 0.18 ms.
pub(super) const ORDER_WORK: usize = 500;
/// Looking at one object in a step of `H J K L`: `10000J` down 500 rectangles looked 250,000
/// times in 51 ms.
pub(super) const LOOK_WORK: usize = 250;
/// Looking at one object in a try of the place rule (`p`, `P`, `o`, `O`): `o` beside a column
/// of 500 texts looked 250,000 times in about 5 ms.
pub(super) const PLACE_WORK: usize = 20;
/// Copying one object into the register and taking it off the board (`d`, `y`): `dG` of 500
/// texts took 1.1 ms more than `G`.
pub(super) const TAKE_WORK: usize = 2_500;
/// One edit of an undo or redo step, or pointing the board's index at one object after it: a
/// step of 500 edits on a board of 500 objects took 1 ms, one putting 500 objects on an empty
/// board as much.
pub(super) const EDIT_WORK: usize = 1_000;
/// Putting one object (`p`, `P`): 250 rectangles took 1.7 ms.
pub(super) const PUT_WORK: usize = 7_000;
/// Looking past one object on the board for a fresh id: `:ci` after `c1` to `c499` took
/// 0.28 ms.
pub(super) const ID_WORK: usize = 600;
/// Reading and running one byte of a `:` line, typed or repeated with `@:`: `@:` of a `:te` of
/// 5,000 characters took 21 us.
pub(super) const PARSE_WORK: usize = 4;
/// Copying and counting one byte, and one character, of a block's text after an edit:
/// `10000ia<Esc>` copied 12.5 million of each in 28 ms.
pub(super) const COPY_WORK: usize = 1;
/// Scanning one byte for the ends of a line, or back through a run of flag halves: `$` on a
/// line of 5,000 flag halves (20,000 bytes) took 1.1 ms.
pub(super) const SCAN_WORK: usize = 60;
/// Splitting one byte of a line into graphemes (`j`, `k`, `b`, `e`, `h` with a count): `j`
/// between two lines of 2,499 characters took 0.85 ms.
pub(super) const SPLIT_WORK: usize = 250;
/// Walking one byte a grapheme at a time (`l`, `w`, `b`, `e`, `x`, `<Left>`, `<Right>`):
/// `10000b` back along 5,000 characters of two-letter words took 6.4 ms.
pub(super) const STEP_WORK: usize = 1_300;

/// The message when a key runs out of work.
const TOO_MUCH_WORK: &str = "that key did too much work; the rest of it was dropped";

/// The work done for the key being run.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Work {
    spent: usize,
    /// The key ran out of work: nothing more runs for it.
    over: bool,
}

impl Engine {
    /// Units of work (see [`MAX_KEY_WORK`]) the last typed key, timeout or click did, the keys
    /// it set off included.
    pub fn key_work(&self) -> usize {
        self.work.spent
    }

    /// A new typed key, timeout or click: its work starts from nothing.
    pub(super) fn reset_work(&mut self) {
        self.work = Work::default();
    }

    /// Counts `units` of work for the key being run. False when they do not fit in what is
    /// left of [`MAX_KEY_WORK`]: then the key has run out, counting none of them. The caller
    /// stops what it was doing; the typeahead is dropped, the message says why, and every later
    /// charge for the key fails too.
    pub(super) fn charge(&mut self, units: usize) -> bool {
        if self.work.over {
            return false;
        }
        match self.work.spent.checked_add(units) {
            Some(spent) if spent <= MAX_KEY_WORK => {
                self.work.spent = spent;
                true
            }
            _ => {
                self.work.over = true;
                self.keymap.flush();
                self.error(TOO_MUCH_WORK);
                false
            }
        }
    }

    /// The key being run has run out of work (see [`Engine::charge`]).
    pub(super) fn out_of_work(&self) -> bool {
        self.work.over
    }

    /// Units of work the key being run may still do.
    pub(super) fn work_left(&self) -> usize {
        if self.work.over {
            0
        } else {
            MAX_KEY_WORK.saturating_sub(self.work.spent)
        }
    }

    /// The work of measuring every object on the board.
    pub(super) fn layout_work(&self) -> usize {
        MEASURE_WORK.saturating_mul(self.board.len().saturating_add(self.board.text_chars()))
    }

    /// The work of finding a fresh id.
    pub(super) fn id_work(&self) -> usize {
        ID_WORK.saturating_mul(self.board.len())
    }

    /// Where the cursor is in the block being edited, for [`Engine::text_work`]: its byte, its
    /// line and the length of the text.
    pub(super) fn text_position(&self) -> Option<TextPosition> {
        let (text, block) = self.block_text().zip(self.block.as_ref())?;
        Some(TextPosition {
            byte: block.byte,
            line: line_at(text, block.byte),
            len: text.len(),
        })
    }

    /// The work of a key in the block that went from `before` to where the cursor is now,
    /// walking `steps` bytes a grapheme at a time and splitting `split` bytes into graphemes,
    /// besides scanning the lines it went from and to and those between.
    pub(super) fn text_work(&self, before: TextPosition, steps: usize, split: usize) -> usize {
        let Some(after) = self.text_position() else {
            return 0;
        };
        let span = before
            .line
            .end
            .max(after.line.end)
            .saturating_sub(before.line.start.min(after.line.start));
        SCAN_WORK
            .saturating_mul(span)
            .saturating_add(SPLIT_WORK.saturating_mul(split))
            .saturating_add(STEP_WORK.saturating_mul(steps))
    }

    /// The work of finding where the grapheme around the cursor starts in the block: a scan
    /// back through the flag halves before it.
    pub(super) fn grapheme_work(&self) -> usize {
        let indicators = self
            .block_text()
            .zip(self.block.as_ref())
            .map_or(0, |(text, block)| indicators_before(text, block.byte));
        SCAN_WORK.saturating_mul(indicators)
    }
}

/// Where the cursor was in the block being edited (see [`Engine::text_position`]).
#[derive(Debug, Clone, Copy)]
pub(super) struct TextPosition {
    pub(super) byte: usize,
    pub(super) line: Line,
    /// Bytes of the block's text.
    pub(super) len: usize,
}
