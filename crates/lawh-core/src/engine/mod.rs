//! The editing engine: a state machine that takes keys, changes the board and asks the app for
//! what it cannot do itself (writing files, quitting) through [`Effect`]s.
//!
//! [`Engine::feed`] takes one key at a time. Each mode has its own dispatcher. In Normal and
//! Visual mode, keys gather in a pending buffer (a count, an operator and its count, then the
//! command's keys) until they make a whole command from the key tables, or none can. Every
//! command that changes the board makes one undo step.
//!
//! The cursor is a grid point or an object (see [`Cursor`]). Normal mode has grid motions
//! (`h j k l`), object motions (`w b`, `H J K L`), the operators `d` and `y` with `dd`, `yy` and
//! `x`, puts (`p`, `P`), Visual mode (`v`), `.` and `@:`, undo and redo (`u`, `<C-r>`), all with
//! counts, and `:` for the command line (shapes, text, math, write and quit).
//!
//! Text and math objects are text blocks: `i a I A o O` type into one in Insert mode, `<CR>`
//! enters one in block-Normal mode, Vim's Normal mode inside the text (see
//! [`Engine::text_cursor`]). While a block is edited, the board already shows its live text;
//! the whole Insert session is one undo step.

mod block;
mod cmdline;
mod command;
mod cursor;
mod exec;
mod insert;
mod measure;
mod motion;
mod normal;
mod ops;
mod place;
mod register;
mod repeat;
mod settings;
mod text;
mod text_motion;
mod undo;
mod visual;

use kurbo::{Point, Rect};

pub use cmdline::MAX_COMMAND_LINE_CHARS;
pub use command::{Command, CommandError, End, Segment, parse_command};
pub use cursor::{Cursor, anchor, round3};
pub use measure::{FixedMeasure, Measure, shape_bounds};
pub use motion::ROW_TOLERANCE;
pub use place::PLACE_GAP;
pub use repeat::MAX_REPEAT_KEYS;
pub use settings::{Escape, Settings};
pub use text::TextCursor;
pub use undo::{MAX_UNDO_BYTES, MAX_UNDO_STEPS};

use self::cmdline::CommandLine;
use self::exec::PendingWrite;
use self::normal::Pending;
use self::register::Register;
use self::repeat::Repeat;
use self::text::{Block, Session};
use self::undo::{Edit, History, Step};
use crate::board::{Board, Object, ObjectId};
use crate::key::Key;

/// Largest count before a command; typing more digits keeps it here. Both counts of `2d3w`
/// together (6) stop here too.
///
/// No motion, undo or put needs more: 10,000 grid units is far past any board on screen,
/// [`MAX_UNDO_STEPS`] is lower still, and a board holds at most
/// [`MAX_OBJECTS`](crate::board::MAX_OBJECTS) objects to walk, delete or put. Saturating keeps a
/// count such as `999999999` cheap to act on, or to refuse.
pub const MAX_COUNT: u32 = 10_000;

/// The engine's mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    /// Selecting objects with a rectangle (see [`Engine::selection`]).
    Visual,
    /// Typing a `:` command (see [`Engine::command_line`]).
    CommandLine,
    /// Typing into a text block (see [`Engine::text_cursor`]).
    Insert,
    /// Vim's Normal mode inside a text block (see [`Engine::text_cursor`]).
    BlockNormal,
}

/// Something the app must do for the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Write the board ([`to_lawh`](crate::to_lawh) of [`Engine::board`]) to `path`
    /// atomically, then report back with [`Engine::write_done`]. When `quit_after` is set and
    /// the write went well, `write_done` returns [`Effect::Quit`]; the app does not quit by
    /// itself.
    Write { path: String, quit_after: bool },
    /// Quit the app. Only sent when quitting is allowed.
    Quit,
    /// Call the engine's timeout after `ms` milliseconds, unless another key comes first.
    ArmTimer { ms: u64 },
}

/// A message for the status line. It lasts until the next key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub text: String,
    /// The message reports a refusal or failure, not just information.
    pub error: bool,
}

/// The Vim engine for one board.
#[derive(Debug, Clone)]
pub struct Engine {
    board: Board,
    mode: Mode,
    cursor: Cursor,
    pending: Pending,
    command_line: CommandLine,
    history: History,
    message: Option<Message>,
    file_name: Option<String>,
    /// The unnamed register.
    register: Option<Register>,
    /// The last change, for `.`.
    repeat: Repeat,
    /// The last `:` line run, for `@:`.
    last_command_line: Option<String>,
    /// In Visual mode, where `v` was pressed: a point or an object's bounds.
    visual: Option<Rect>,
    /// In Visual mode, the objects inside the rectangle, in z-order.
    selection: Vec<ObjectId>,
    /// Counts every change to the board, undo and redo included, so a write can tell whether
    /// the board is still the one it wrote.
    version: u64,
    /// The write asked for with [`Effect::Write`], until [`Engine::write_done`].
    pending_write: Option<PendingWrite>,
    /// The text block being edited: in Insert and block-Normal mode, and on a command line
    /// opened from block-Normal mode.
    block: Option<Block>,
    /// The Insert session under way.
    session: Option<Session>,
    settings: Settings,
}

impl Engine {
    /// An engine for `board`, in Normal mode with the cursor at the grid's origin. The board
    /// counts as saved.
    pub fn new(board: Board) -> Self {
        Self {
            board,
            mode: Mode::Normal,
            cursor: Cursor::Grid(Point::ZERO),
            pending: Pending::default(),
            command_line: CommandLine::default(),
            history: History::default(),
            message: None,
            file_name: None,
            register: None,
            repeat: Repeat::default(),
            last_command_line: None,
            visual: None,
            selection: Vec::new(),
            version: 0,
            pending_write: None,
            block: None,
            session: None,
            settings: Settings::default(),
        }
    }

    /// The file the board was read from and is written to, if it has one.
    #[must_use]
    pub fn with_file_name(mut self, name: Option<String>) -> Self {
        self.file_name = name;
        self
    }

    pub fn board(&self) -> &Board {
        &self.board
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The settings, to change them. A change applies from the next key.
    pub fn settings_mut(&mut self) -> &mut Settings {
        &mut self.settings
    }

    /// Takes one key. Keys are made canonical first (see [`Key::new`]).
    ///
    /// `measure` gives the bounds of text and math, for object motions, Visual mode and puts.
    pub fn feed(&mut self, key: Key, measure: &mut dyn Measure) -> Vec<Effect> {
        let key = key.canonical();
        self.message = None;
        self.dispatch(key, measure)
    }

    /// Runs one key in the current mode. `.` replays its keys through here, so mappings do not
    /// apply to them again.
    fn dispatch(&mut self, key: Key, measure: &mut dyn Measure) -> Vec<Effect> {
        match self.mode {
            Mode::Normal => self.normal_key(key, measure),
            Mode::Visual => self.visual_key(key, measure),
            Mode::CommandLine => self.command_line_key(key),
            Mode::Insert => self.insert_key(key, measure),
            Mode::BlockNormal => self.block_key(key, measure),
        }
    }

    /// A mouse click at board point `at`, on object `hit` if any. It ends an Insert session
    /// (as one undo step), block-Normal mode, Visual mode and the command line, drops pending
    /// keys, and puts the cursor on `hit`, or else on the grid point nearest `at` (whole units).
    /// A point that is not finite leaves the cursor where it is.
    pub fn click(&mut self, at: Point, hit: Option<ObjectId>, measure: &mut dyn Measure) {
        if self.session.is_some() {
            self.end_session(false, measure);
        }
        self.block = None;
        self.pending = Pending::default();
        self.message = None;
        self.command_line.clear();
        self.leave_visual();
        self.mode = Mode::Normal;
        match hit.filter(|id| self.board.contains(id)) {
            Some(id) => self.cursor = Cursor::Object(id),
            None if at.is_finite() => self.cursor = Cursor::Grid(cursor::grid_point(at)),
            None => {}
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn cursor(&self) -> &Cursor {
        &self.cursor
    }

    /// The cursor's grid point, or the anchor (see [`anchor`]) of the current object.
    pub fn cursor_point(&self) -> Point {
        match &self.cursor {
            Cursor::Grid(p) => *p,
            // The engine keeps an object cursor on an object; the origin is only a fallback.
            Cursor::Object(id) => self.board.get(id).map_or(Point::ZERO, |o| anchor(&o.shape)),
        }
    }

    /// The command line's text after `:` and its cursor as a byte offset, in command-line mode.
    pub fn command_line(&self) -> Option<(&str, usize)> {
        (self.mode == Mode::CommandLine)
            .then(|| (self.command_line.text(), self.command_line.cursor()))
    }

    /// Keys typed but not yet run, in Vim notation (`3`, `2d3`), for the status line. A count
    /// past [`MAX_COUNT`] shows as `MAX_COUNT`.
    pub fn pending(&self) -> String {
        self.pending.display()
    }

    pub fn message(&self) -> Option<&Message> {
        self.message.as_ref()
    }

    pub fn file_name(&self) -> Option<&str> {
        self.file_name.as_deref()
    }

    /// The board has changed since it was opened or last saved (undoing back to that point
    /// makes it unmodified again). Text typed in an Insert session counts at once.
    pub fn is_modified(&self) -> bool {
        self.history.is_modified() || self.session_changed()
    }

    fn error(&mut self, text: impl Into<String>) {
        self.message = Some(Message {
            text: text.into(),
            error: true,
        });
    }

    fn info(&mut self, text: impl Into<String>) {
        self.message = Some(Message {
            text: text.into(),
            error: false,
        });
    }

    /// The position of object `id` in z-order.
    fn index_of(&self, id: &ObjectId) -> Option<usize> {
        self.board.objects().iter().position(|o| &o.id == id)
    }

    /// The bounds of every object, in z-order.
    fn layout(&self, measure: &mut dyn Measure) -> Vec<Rect> {
        self.board
            .objects()
            .iter()
            .map(|o| bounds(o, measure))
            .collect()
    }

    /// Moves the cursor point by whole grid units; an object cursor becomes a grid point.
    fn move_by(&mut self, dx: f64, dy: f64) {
        self.cursor = Cursor::Grid(cursor::offset(self.cursor_point(), dx, dy));
    }

    /// Records edits already made on the board as one undo step, from `cursor_before` to the
    /// cursor now.
    fn commit(&mut self, edits: Vec<Edit>, cursor_before: Cursor) {
        self.commit_at(edits, cursor_before, None, None);
    }

    /// [`Engine::commit`] for a change made inside a text block: undo and redo in
    /// block-Normal mode put the cursor at `byte_before` and `byte_after` in it.
    fn commit_at(
        &mut self,
        edits: Vec<Edit>,
        cursor_before: Cursor,
        byte_before: Option<usize>,
        byte_after: Option<usize>,
    ) {
        self.history.push(Step {
            edits,
            cursor_before,
            cursor_after: self.cursor.clone(),
            byte_before,
            byte_after,
        });
        self.version = self.version.wrapping_add(1);
    }

    /// Takes back up to `count` steps, putting the cursor where it was before the last one. In
    /// block-Normal mode it stays in the block while the cursor is on it.
    fn undo(&mut self, count: u32) {
        let mut byte = None;
        for done in 0..count {
            match self.history.undo(&mut self.board) {
                Some(Ok((cursor, at))) => {
                    self.cursor = cursor;
                    byte = at;
                    self.version = self.version.wrapping_add(1);
                }
                Some(Err(error)) => {
                    self.error(format!("cannot undo: {error}"));
                    break;
                }
                None if done == 0 => {
                    self.error("already at the oldest change");
                    break;
                }
                None => break,
            }
        }
        self.sync_block(byte);
    }

    /// Makes again up to `count` undone steps, putting the cursor where it was after the last.
    /// In block-Normal mode it stays in the block while the cursor is on it.
    fn redo(&mut self, count: u32) {
        let mut byte = None;
        for done in 0..count {
            match self.history.redo(&mut self.board) {
                Some(Ok((cursor, at))) => {
                    self.cursor = cursor;
                    byte = at;
                    self.version = self.version.wrapping_add(1);
                }
                Some(Err(error)) => {
                    self.error(format!("cannot redo: {error}"));
                    break;
                }
                None if done == 0 => {
                    self.error("already at the newest change");
                    break;
                }
                None => break,
            }
        }
        self.sync_block(byte);
    }
}

/// The bounds of `object`, or a box of zero size at its anchor when it cannot be measured.
fn bounds(object: &Object, measure: &mut dyn Measure) -> Rect {
    measure.bounds(object).unwrap_or_else(|| {
        let p = anchor(&object.shape);
        Rect::from_points(p, p)
    })
}
