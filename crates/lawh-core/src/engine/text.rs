//! Text blocks: entering a text or math object to edit its text, and Insert sessions.
//!
//! From the board, `i a I A` on a grid point start a new text block there in Insert mode; `o`
//! and `O` start one below or above the current object (or the cursor's grid point) by the
//! place rule. On a text or math object, `i` and `I` insert at the start of its text, `a` and
//! `A` at the end, and `<CR>` enters block-Normal mode at the start. Other kinds have no text.
//!
//! A new block goes on the board, empty, as soon as Insert mode starts, so the board always
//! shows the live text and the renderer can draw the cursor in it. Every edit replaces the
//! block on the board, which checks the caps on text. An Insert session, from entering Insert
//! mode to leaving it, is one undo step and one change for `.`; a new block still empty when it
//! ends is taken off the board again, leaving no undo step.
//!
//! The Insert-mode keys are in `insert.rs`, block-Normal mode in `block.rs`, and positions in
//! the text in `text_motion.rs`.

use kurbo::{Point, Rect};

use super::place::{Side, place};
use super::repeat::{Context, MAX_REPEAT_KEYS};
use super::settings::Escape;
use super::text_motion::{after_insert, content, on_grapheme};
use super::undo::Edit;
use super::{Cursor, Engine, Measure, Mode, bounds, exec};
use crate::board::{Object, ObjectId, Shape};
use crate::key::{Key, KeyCode};

/// The cursor inside a text block, for drawing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextCursor {
    /// The block: a text or math object.
    pub id: ObjectId,
    /// A byte offset in the block's text (or math source), on a grapheme boundary. In Insert
    /// mode, where the next character goes; in block-Normal mode, the start of the grapheme the
    /// cursor is on, or the start of an empty line.
    pub byte: usize,
    /// Block-Normal mode (the cursor is on a grapheme), not Insert mode (between graphemes).
    pub block: bool,
}

/// How a board key enters a text block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Entry {
    /// `i`, `I`: Insert mode at the start of the text.
    Start,
    /// `a`, `A`: Insert mode at its end.
    End,
    /// `o` (below), `O` (above): Insert mode in a new block next to the current object.
    Open(Side),
    /// `<CR>`: block-Normal mode at the start of the text.
    Block,
}

/// The text block being edited, in Insert mode, block-Normal mode, and on a command line
/// opened from block-Normal mode.
#[derive(Debug, Clone)]
pub(super) struct Block {
    pub(super) id: ObjectId,
    /// The cursor in the block's text (see [`TextCursor::byte`]).
    pub(super) byte: usize,
    /// The column, in graphemes, that `j` and `k` (`<Up>` and `<Down>` in Insert mode) keep to
    /// once one of them has moved; `usize::MAX` after `$`, for the end of every line.
    pub(super) want: Option<usize>,
}

/// An Insert session, from entering Insert mode to leaving it.
#[derive(Debug, Clone)]
pub(super) struct Session {
    /// Entered from block-Normal mode, not from the board.
    from_block: bool,
    /// The board cursor and the byte in the block before the session, for undo.
    cursor_before: Cursor,
    byte_before: Option<usize>,
    /// The block's shape before the session; `None` when the session made the block.
    before: Option<Shape>,
    /// For `o` and `O`: the side a count puts further blocks on.
    open: Option<Side>,
    /// Blocks a count on `o` or `O` made and finished, as their inserts.
    done: Vec<Edit>,
    /// The last of those blocks and its cursor, should the current one end empty.
    last_done: Option<(ObjectId, usize)>,
    /// The count typed before the key that entered Insert mode.
    count: Option<u32>,
    /// The keys typed in Insert mode, for the count; `None` past [`MAX_REPEAT_KEYS`].
    keys: Option<Vec<Key>>,
}

/// Where Insert mode starts in a block, and what it was entered from.
struct Start {
    id: ObjectId,
    /// Where typing starts.
    byte: usize,
    /// The block's shape before the session; `None` for a new block.
    before: Option<Shape>,
    from_block: bool,
    /// The byte in the block before the session, for undo.
    byte_before: Option<usize>,
    open: Option<Side>,
    count: Option<u32>,
}

/// The message for `i a I A <CR>` on an object without text.
const NO_TEXT: &str = "only text and math have text to edit";

impl Engine {
    /// The cursor inside the text block being edited, in Insert and block-Normal mode.
    pub fn text_cursor(&self) -> Option<TextCursor> {
        let block = match self.mode {
            Mode::Insert => false,
            Mode::BlockNormal => true,
            Mode::Normal | Mode::Visual | Mode::CommandLine => return None,
        };
        let current = self.block.as_ref()?;
        Some(TextCursor {
            id: current.id.clone(),
            byte: current.byte,
            block,
        })
    }

    /// The text of the block being edited, while it is a text or math object on the board.
    pub(super) fn block_text(&self) -> Option<&str> {
        let block = self.block.as_ref()?;
        self.board.get(&block.id).and_then(|o| content(&o.shape))
    }

    /// `i a I A o O <CR>` on the board, with the count typed before them.
    pub(super) fn enter_text(
        &mut self,
        entry: Entry,
        count: Option<u32>,
        measure: &mut dyn Measure,
    ) {
        if let Entry::Open(side) = entry {
            return self.open_block(side, count, measure);
        }
        let id = match &self.cursor {
            Cursor::Grid(at) => {
                if entry == Entry::Block {
                    return self.error("no text under the cursor: <CR> enters text and math");
                }
                let at = *at;
                return self.new_block(at, None, count);
            }
            Cursor::Object(id) => id.clone(),
        };
        let Some(object) = self.board.get(&id) else {
            return;
        };
        let Some(text) = content(&object.shape) else {
            return self.error(NO_TEXT);
        };
        let byte = match entry {
            Entry::Start | Entry::Block | Entry::Open(_) => 0,
            Entry::End => text.len(),
        };
        if entry == Entry::Block {
            self.block = Some(Block {
                id,
                byte: on_grapheme(text, byte),
                want: None,
            });
            self.mode = Mode::BlockNormal;
            return;
        }
        let before = Some(object.shape.clone());
        self.start_session(Start {
            id,
            byte,
            before,
            from_block: false,
            byte_before: Some(byte),
            open: None,
            count,
        });
    }

    /// `o` and `O`: a new text block in Insert mode on `side` of the current object.
    fn open_block(&mut self, side: Side, count: Option<u32>, measure: &mut dyn Measure) {
        match self.open_position(side, measure) {
            Some(at) => self.new_block(at, Some(side), count),
            None => self.error("no room for a new text block"),
        }
    }

    /// Where a new text block goes on `side` of the current object, by the place rule. With a
    /// grid cursor, the grid point is the object, of no size. `None` when there is no room.
    fn open_position(&self, side: Side, measure: &mut dyn Measure) -> Option<Point> {
        let obstacles = self.layout(measure);
        let point = self.cursor_point();
        let reference = match &self.cursor {
            Cursor::Grid(_) => None,
            Cursor::Object(id) => self.index_of(id).and_then(|i| obstacles.get(i).copied()),
        }
        .unwrap_or_else(|| Rect::from_points(point, point));
        let empty = Object::new(self.board.fresh_id("t"), empty_text(Point::ZERO));
        place(reference, bounds(&empty, measure).size(), &obstacles, side)
    }

    /// Puts an empty text block at `at` on top of the board and starts Insert mode in it.
    fn new_block(&mut self, at: Point, open: Option<Side>, count: Option<u32>) {
        if let Some(id) = self.add_empty_block(at) {
            self.start_session(Start {
                id,
                byte: 0,
                before: None,
                from_block: false,
                byte_before: None,
                open,
                count,
            });
        }
    }

    /// Puts an empty text block at `at` on top of the board: its id, or `None` with a message
    /// when the board refuses it.
    fn add_empty_block(&mut self, at: Point) -> Option<ObjectId> {
        let id = self.board.fresh_id("t");
        match self.board.insert(Object::new(id.clone(), empty_text(at))) {
            Ok(()) => Some(id),
            Err(error) => {
                self.error(exec::refusal(&error));
                None
            }
        }
    }

    /// Enters Insert mode in a block.
    fn start_session(&mut self, start: Start) {
        self.session = Some(Session {
            from_block: start.from_block,
            cursor_before: self.cursor.clone(),
            byte_before: start.byte_before,
            before: start.before,
            open: start.open,
            done: Vec::new(),
            last_done: None,
            count: start.count,
            keys: Some(Vec::new()),
        });
        self.cursor = Cursor::Object(start.id.clone());
        self.block = Some(Block {
            id: start.id,
            byte: start.byte,
            want: None,
        });
        self.mode = Mode::Insert;
    }

    /// `i a I A` in block-Normal mode: Insert mode at `byte` in the current block.
    pub(super) fn insert_in_block(&mut self, byte: usize, count: Option<u32>) {
        let Some(block) = &self.block else {
            return;
        };
        let (id, byte_before) = (block.id.clone(), block.byte);
        let Some(before) = self.board.get(&id).map(|o| o.shape.clone()) else {
            return;
        };
        self.start_session(Start {
            id,
            byte,
            before: Some(before),
            from_block: true,
            byte_before: Some(byte_before),
            open: None,
            count,
        });
    }

    /// Keeps a key typed in Insert mode, for the session's count.
    pub(super) fn keep_session_key(&mut self, key: Key) {
        let Some(session) = &mut self.session else {
            return;
        };
        let full = session
            .keys
            .as_ref()
            .is_some_and(|keys| keys.len() >= MAX_REPEAT_KEYS);
        if full {
            session.keys = None;
        } else if let Some(keys) = &mut session.keys {
            keys.push(key);
        }
    }

    /// The session has changed the board so far.
    pub(super) fn session_changed(&self) -> bool {
        let Some(session) = &self.session else {
            return false;
        };
        let shape = self
            .block
            .as_ref()
            .and_then(|b| self.board.get(&b.id))
            .map(|o| &o.shape);
        !session.done.is_empty()
            || match &session.before {
                None => shape.and_then(content).is_some_and(|t| !t.is_empty()),
                Some(before) => shape != Some(before),
            }
    }

    /// Ends the Insert session, for `<Esc>` (`by_esc`) or a click. `<Esc>` first types the
    /// session's keys again for a count on the key that started it.
    ///
    /// A change to the board becomes one undo step and the last change for `.`. A new block
    /// left empty is taken off the board. The cursor goes to the block: on the board, or in
    /// block-Normal mode after `<Esc>` when the session came from there or the `escape` setting
    /// is [`Escape::Block`]. Without a block left, it goes back to where it was before.
    pub(super) fn end_session(&mut self, by_esc: bool, measure: &mut dyn Measure) {
        if by_esc {
            self.repeat_session(measure);
        } else {
            // A click ends the session: `.` replays it with the `<Esc>` it did not type.
            self.repeat.push(Key::named(KeyCode::Esc));
        }
        let (Some(mut session), Some(block)) = (self.session.take(), self.block.take()) else {
            self.mode = Mode::Normal;
            return;
        };
        let mut edits = std::mem::take(&mut session.done);
        let mut kept = Some((block.id.clone(), block.byte));
        let object = self.board.get(&block.id).cloned();
        let index = self.index_of(&block.id);
        match (&session.before, object, index) {
            (None, Some(object), Some(index)) => {
                if content(&object.shape).is_none_or(str::is_empty) {
                    self.board.remove(&block.id);
                    kept = session.last_done.take();
                } else {
                    edits.push(Edit::Insert { index, object });
                }
            }
            (Some(before), Some(object), _) if object.shape != *before => {
                edits.push(Edit::Replace {
                    id: block.id,
                    before: before.clone(),
                    after: object.shape,
                });
            }
            _ => {}
        }

        let to_block = by_esc && (session.from_block || self.settings.escape == Escape::Block);
        self.mode = Mode::Normal;
        let byte_after = match kept {
            None => {
                self.cursor = session.cursor_before.clone();
                None
            }
            Some((id, byte)) => {
                let text = self.board.get(&id).and_then(|o| content(&o.shape));
                let byte = after_insert(text.unwrap_or_default(), byte);
                self.cursor = Cursor::Object(id.clone());
                if to_block {
                    self.block = Some(Block {
                        id,
                        byte,
                        want: None,
                    });
                    self.mode = Mode::BlockNormal;
                }
                Some(byte)
            }
        };
        if !edits.is_empty() {
            self.commit_at(
                edits,
                session.cursor_before,
                session.byte_before,
                byte_after,
            );
            // `o` and `O` make a block next to an object: only on the board. `i a I A` type
            // wherever they are pressed, on the board or in a block.
            if session.open.is_some() {
                self.repeat.finish(session.count);
            } else {
                self.repeat.finish_in(session.count, Context::Anywhere);
            }
        }
    }

    /// A count on the key that started the session: types the session's keys `count - 1` more
    /// times, as Vim does (`3ia<Esc>` inserts `aaa`). After `o` or `O`, each time in a new
    /// block on the same side of the last. Stops at the first edit the board refuses, and once
    /// [`MAX_REPEAT_KEYS`] keys would be replayed.
    fn repeat_session(&mut self, measure: &mut dyn Measure) {
        let Some(session) = &self.session else {
            return;
        };
        let (Some(count), Some(keys), open) = (session.count, session.keys.clone(), session.open)
        else {
            return;
        };
        if keys.is_empty() {
            return;
        }
        let times = usize::try_from(count.saturating_sub(1))
            .unwrap_or(usize::MAX)
            .min(MAX_REPEAT_KEYS / keys.len());
        for _ in 0..times {
            if let Some(side) = open
                && !self.open_another(side, measure)
            {
                return;
            }
            for &key in &keys {
                if !self.insert_edit(key) {
                    return;
                }
            }
        }
    }

    /// For a count on `o` or `O`: finishes the current block and starts the next on `side` of
    /// it. False, leaving the current block as it is, when it is empty or there is no room.
    fn open_another(&mut self, side: Side, measure: &mut dyn Measure) -> bool {
        let Some(block) = self.block.clone() else {
            return false;
        };
        let Some(object) = self.board.get(&block.id).cloned() else {
            return false;
        };
        let Some(index) = self.index_of(&block.id) else {
            return false;
        };
        if content(&object.shape).is_none_or(str::is_empty) {
            return false;
        }
        let Some(at) = self.open_position(side, measure) else {
            self.error("no room for a new text block");
            return false;
        };
        let Some(id) = self.add_empty_block(at) else {
            return false;
        };
        if let Some(session) = &mut self.session {
            session.done.push(Edit::Insert { index, object });
            session.last_done = Some((block.id, block.byte));
        }
        self.cursor = Cursor::Object(id.clone());
        self.block = Some(Block {
            id,
            byte: 0,
            want: None,
        });
        true
    }

    /// After undo, redo or a `:` command: keeps the block being edited while the cursor is on
    /// it and it still has text, with the cursor at `byte` (from the undo step) if given, else
    /// where it was, on a grapheme. Otherwise leaves it: block-Normal mode becomes Normal mode.
    pub(super) fn sync_block(&mut self, byte: Option<usize>) {
        let Some(block) = &self.block else {
            return;
        };
        let on_block = matches!(&self.cursor, Cursor::Object(id) if *id == block.id);
        let kept = self
            .block_text()
            .filter(|_| on_block)
            .map(|text| on_grapheme(text, byte.unwrap_or(block.byte)));
        if let (Some(byte), Some(block)) = (kept, &mut self.block) {
            block.byte = byte;
            block.want = None;
        } else {
            self.block = None;
            if self.mode == Mode::BlockNormal {
                self.mode = Mode::Normal;
            }
        }
    }
}

/// An empty text at `at`: a new block before its first character.
fn empty_text(at: Point) -> Shape {
    Shape::Text {
        at,
        text: String::new(),
    }
}
