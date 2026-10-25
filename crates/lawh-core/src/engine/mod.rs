//! The editing engine: a state machine that takes keys, changes the board and asks the app for
//! what it cannot do itself (writing files, quitting) through [`Effect`]s.
//!
//! [`Engine::feed`] takes one key at a time. Each mode has its own dispatcher. In Normal and
//! Visual mode, keys gather in a pending buffer (a count, an operator and its count, then the
//! command's keys) until they make a whole command from the key tables, or none can. Every
//! command that changes the board makes one undo step.
//!
//! The cursor is a grid point or an object (see [`Cursor`]). Normal mode has grid motions
//! (`h j k l`), object motions (`w b`, `H J K L`, `gg G`), the operators `d` and `y` with `dd`,
//! `yy` and `x`, puts (`p`, `P`), Visual mode (`v`), `.` and `@:`, undo and redo (`u`,
//! `<C-r>`), the size keys (`<C-a>`, `<C-x>`), all with counts, jump labels (`s`, see
//! [`Engine::jump_labels`]) and `:` for the command line (shapes, text, math, sizes, symbols by
//! name with completions, write, export and quit).
//!
//! Text and math objects are text blocks: `i a I A o O` type into one in Insert mode, `<CR>`
//! enters one in block-Normal mode, Vim's Normal mode inside the text (see
//! [`Engine::text_cursor`]). While a block is edited, the board already shows its live text;
//! the whole Insert session is one undo step.
//!
//! Keys go through key mappings before the grammar (see `keymap.rs`): `:map` and its family,
//! per mode as in Vim, with `<Leader>` and a timeout for keys that start a longer mapping
//! ([`Effect::ArmTimer`], [`Engine::timeout`]). `:set` changes the [`Settings`], and
//! [`Engine::source`] reads both from a lawhrc.
//!
//! One typed key may set off many commands, through mappings, `.` and counts. It does at most
//! [`MAX_KEY_WORK`] work (see `work.rs`), asks for at most [`MAX_WRITES_PER_KEY`] writes and
//! at most [`MAX_EXPORTS_PER_KEY`] export: past any of them, the command under way stops where
//! it is and the rest of the key's keys are dropped, with a message.

mod block;
mod cmdline;
mod command;
mod cursor;
mod exec;
mod insert;
mod jump;
mod keymap;
mod measure;
mod motion;
mod normal;
mod ops;
mod place;
mod rc;
mod register;
mod repeat;
mod settings;
mod size;
mod sym;
mod text;
mod text_motion;
mod undo;
mod visual;
mod work;

use std::collections::VecDeque;

use kurbo::{Point, Rect};
use nucleo_matcher::Matcher;

pub use cmdline::MAX_COMMAND_LINE_CHARS;
pub use command::{Command, CommandError, End, Segment, SizeChange, parse_command};
pub use cursor::{Cursor, anchor, round3};
pub use exec::{MAX_EXPORTS_PER_KEY, MAX_PENDING_WRITES, MAX_WRITES_PER_KEY};
pub use jump::{
    DEFAULT_JUMP_LABELS, JumpLabel, LabelChars, MAX_LABEL_CHARS, MAX_LABEL_KEYS, MIN_LABEL_CHARS,
};
pub use keymap::{
    MAX_EXPANDED_KEYS, MAX_MAP_DEPTH, MAX_MAP_KEYS, MAX_MAPS, MAX_PENDING_KEYS, MapKey, MapMode,
};
pub use measure::{FixedMeasure, Measure, shape_bounds};
pub use motion::ROW_TOLERANCE;
pub use place::PLACE_GAP;
pub use rc::{MAX_RC_BYTES, MAX_RC_WARNINGS, RcWarning};
pub use repeat::MAX_REPEAT_KEYS;
pub use settings::{
    Escape, MAX_TIMEOUTLEN, OptionName, OptionValue, SetArg, Settings, parse_png_scale,
};
pub use size::SIZE_STEPS;
pub use sym::{MAX_SYM_QUERY_CHARS, MAX_SYMBOLS, SYM_ROWS, Symbol};
pub use text::TextCursor;
pub use undo::{MAX_UNDO_BYTES, MAX_UNDO_STEPS};
pub use work::MAX_KEY_WORK;

use self::cmdline::CommandLine;
use self::exec::PendingWrite;
use self::jump::Labels;
use self::keymap::Keymap;
use self::normal::Pending;
use self::register::Register;
use self::repeat::Repeat;
use self::sym::{Completions, Symbols};
use self::text::{Block, Session};
use self::undo::{Edit, History, Step};
use self::work::{EDIT_WORK, Work};
use crate::board::{Board, Object, ObjectId};
use crate::export::ExportOptions;
use crate::key::{Key, KeyCode};

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
    /// the write went well, `write_done` returns [`Effect::Quit`], unless the write was a copy
    /// and the board's own file still lacks its changes; the app does not quit by itself.
    ///
    /// One key or timeout asks for at most [`MAX_WRITES_PER_KEY`] writes, and the board does not
    /// change while the app does them: the app may write each file once for them all, as long as
    /// it reports every write, in order.
    Write { path: String, quit_after: bool },
    /// Export the board to `path` (`:exp`): draw it, or only the objects of `selection` when it
    /// is `Some` (the Visual selection `:exp` was typed from, in z-order, never empty), into a
    /// file of `options.format` with the rest of `options` (`:set exportbg`, `:set pngscale`),
    /// write that atomically, then report back with [`Engine::export_done`]. The board itself
    /// is not written, and the export neither names it nor marks it saved.
    ///
    /// One key or timeout asks for at most [`MAX_EXPORTS_PER_KEY`] exports, each counted as a
    /// write against [`MAX_WRITES_PER_KEY`] too, and the board does not change while the app
    /// does them.
    Export {
        path: String,
        options: ExportOptions,
        selection: Option<Vec<ObjectId>>,
    },
    /// Quit the app. Only sent when quitting is allowed.
    Quit,
    /// Keys wait for a longer mapping: call [`Engine::timeout`] after `ms` milliseconds,
    /// unless another key comes first. The app drops its armed timer on every key, before
    /// [`Engine::feed`], and arms this one in its place.
    ArmTimer { ms: u64 },
}

/// A message for the status line. It lasts until the next typed key or click; a timeout
/// ([`Engine::timeout`]) runs keys typed before it and keeps the message they left.
///
/// One key may leave several messages: each command it sets off may (through mappings, `.` and
/// counts), and so may the reports of its writes and exports ([`Engine::write_done`],
/// [`Engine::export_done`]), which come after all its commands. Information never replaces an
/// error, so a mapping such as `:w copy.lawh<CR>:w<CR>` whose copy fails ends on the failure,
/// not on `"a.lawh" written`, and one refused for asking too many writes ends on the refusal,
/// not on its writes' reports. An error replaces any message, an earlier error too: the last
/// error is the one that stopped the key (a recursive mapping, too much work, a refused write or
/// export), or a write or export that failed after the key's commands, the news the status line
/// must not lose.
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
    /// In Visual mode, the objects inside the rectangle, in z-order. Kept on a command line
    /// opened from Visual mode (see `visual_command`).
    selection: Vec<ObjectId>,
    /// The command line was opened from Visual mode: `selection` is still the selection, for
    /// `:si` (as Vim's `:'<,'>`), until the command line closes.
    visual_command: bool,
    /// Counts every change to the board, undo and redo included, so a write can tell whether
    /// the board is still the one it wrote. Every change to the board bumps it (see
    /// [`Engine::changed`]).
    version: u64,
    /// The writes asked for with [`Effect::Write`] and not yet reported with
    /// [`Engine::write_done`], oldest first, at most [`MAX_PENDING_WRITES`].
    pending_writes: VecDeque<PendingWrite>,
    /// The text block being edited: in Insert and block-Normal mode, and on a command line
    /// opened from block-Normal mode.
    block: Option<Block>,
    /// The Insert session under way.
    session: Option<Session>,
    settings: Settings,
    /// Key mappings and the typeahead.
    keymap: Keymap,
    /// The work done for the key being run (see [`MAX_KEY_WORK`]).
    work: Work,
    /// Jump labels showing after `s`, in Normal or Visual mode; every key goes to them first.
    labels: Option<Labels>,
    /// The part of the board in view, for jump labels (see [`Engine::set_view`]).
    view: Option<Rect>,
    /// The symbols `:sym` picks from (see [`Engine::set_symbols`]).
    symbols: Symbols,
    /// The matcher `:sym` matches with, made the first time it matches: it holds about 135 KB
    /// of scratch memory.
    matcher: Option<Matcher>,
    /// The completions of a `:sym` line on the command line (see [`Engine::completions`]).
    completions: Option<Completions>,
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
            visual_command: false,
            version: 0,
            pending_writes: VecDeque::new(),
            block: None,
            session: None,
            settings: Settings::default(),
            keymap: Keymap::default(),
            work: Work::default(),
            labels: None,
            view: None,
            symbols: Symbols::default(),
            matcher: None,
            completions: None,
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

    /// Takes one typed key. Keys are made canonical first (see [`Key::new`]), then go through
    /// the key mappings. When the keys typed so far start a longer mapping, they wait: the
    /// effects end with [`Effect::ArmTimer`], and the keys run on the next key or on
    /// [`Engine::timeout`]. After the mappings, Shift with `<Esc>`, `<CR>`, `<BS>` or `<Del>`
    /// is dropped, in every mode: `<S-CR>` runs the command line. A mapping of the shifted key
    /// still wins.
    ///
    /// `measure` gives the bounds of text and math, for object motions, Visual mode and puts,
    /// and what laying text out to measure it cost, which counts against the key's work
    /// ([`Measure::take_work`]).
    pub fn feed(&mut self, key: Key, measure: &mut dyn Measure) -> Vec<Effect> {
        let key = key.canonical();
        self.message = None;
        self.type_key(key, measure)
    }

    /// Runs one key in the current mode, after mappings, or picks a jump label with it while
    /// labels show. `.` replays its keys through here, so mappings do not apply to them again.
    fn dispatch(&mut self, key: Key, measure: &mut dyn Measure) -> Vec<Effect> {
        let key = unshifted(key);
        if self.labels.is_some() {
            return self.label_key(key, measure);
        }
        match self.mode {
            Mode::Normal => self.normal_key(key, measure),
            Mode::Visual => self.visual_key(key, measure),
            Mode::CommandLine => self.command_line_key(key, measure),
            Mode::Insert => self.insert_key(key, measure),
            Mode::BlockNormal => self.block_key(key, measure),
        }
    }

    /// A mouse click at board point `at`, on object `hit` if any. It ends jump labels, an
    /// Insert session (as one undo step, a `:sym` line opened from it too), block-Normal mode,
    /// Visual mode and the command line, drops pending keys (those waiting for a mapping too),
    /// and puts the cursor on `hit`, or else on the grid point nearest `at` (whole units). A
    /// point that is not finite leaves the cursor where it is.
    pub fn click(&mut self, at: Point, hit: Option<ObjectId>, measure: &mut dyn Measure) {
        self.reset_work(measure);
        if self.session.is_some() {
            self.end_session(false, measure);
        }
        self.labels = None;
        self.block = None;
        self.pending = Pending::default();
        self.keymap.flush();
        self.message = None;
        self.command_line.clear();
        self.completions = None;
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

    /// Keys typed but not yet run, in Vim notation (`3`, `2d3`), for the status line: a
    /// command's keys so far, then keys waiting for a longer mapping (`2d<Space>`). A count past
    /// [`MAX_COUNT`] shows as `MAX_COUNT`. While jump labels show, the keys that showed them and
    /// the first character of a label typed since, counts left out (`s`, `ds`, `dsk`).
    pub fn pending(&self) -> String {
        if let Some(labels) = &self.labels {
            return labels.pending();
        }
        self.pending.display() + &self.keymap.waiting()
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

    /// Makes `text` the message, as an error, in place of any message (see [`Message`]).
    fn error(&mut self, text: impl Into<String>) {
        self.message = Some(Message {
            text: text.into(),
            error: true,
        });
    }

    /// Makes `text` the message, as information, unless the message is an error: an error
    /// stays until the next typed key or click, or a later error (see [`Message`]).
    fn info(&mut self, text: impl Into<String>) {
        if self.message.as_ref().is_some_and(|m| m.error) {
            return;
        }
        self.message = Some(Message {
            text: text.into(),
            error: false,
        });
    }

    /// The position of object `id` in z-order.
    fn index_of(&self, id: &ObjectId) -> Option<usize> {
        self.board.position(id)
    }

    /// The bounds of every object, in z-order, counting the work `measure` reports for them
    /// (and for anything it measured since it last reported: [`Engine::charge_measured`]). The
    /// caller counts the engine's own share first ([`Engine::layout_work`]).
    fn layout(&mut self, measure: &mut dyn Measure) -> Vec<Rect> {
        let layout = self
            .board
            .objects()
            .iter()
            .map(|o| bounds(o, measure))
            .collect();
        self.charge_measured(measure);
        layout
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
        self.changed();
    }

    /// The board has changed: a write asked for before now no longer has it as it is.
    fn changed(&mut self) {
        self.version = self.version.wrapping_add(1);
    }

    /// Takes back up to `count` steps, putting the cursor where it was before the last one. In
    /// block-Normal mode it stays in the block while the cursor is on it. It stops at the
    /// oldest step, and when the key runs out of work (see [`MAX_KEY_WORK`]): each step counts
    /// its edits twice (making them, then indexing the objects they put) and the objects on the
    /// board.
    fn undo(&mut self, count: u32) {
        let mut byte = None;
        for done in 0..count {
            let size = self.history.undo_size();
            let edits = size.saturating_mul(2).saturating_add(self.board.len());
            let work = EDIT_WORK.saturating_mul(edits);
            if !self.charge(work) {
                break;
            }
            match self.history.undo(&mut self.board) {
                Some(Ok((cursor, at))) => {
                    self.cursor = cursor;
                    byte = at;
                    self.changed();
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
    /// In block-Normal mode it stays in the block while the cursor is on it. It stops at the
    /// newest step, and when the key runs out of work, as [`Engine::undo`] does.
    fn redo(&mut self, count: u32) {
        let mut byte = None;
        for done in 0..count {
            let size = self.history.redo_size();
            let edits = size.saturating_mul(2).saturating_add(self.board.len());
            let work = EDIT_WORK.saturating_mul(edits);
            if !self.charge(work) {
                break;
            }
            match self.history.redo(&mut self.board) {
                Some(Ok((cursor, at))) => {
                    self.cursor = cursor;
                    byte = at;
                    self.changed();
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

/// `key` without Shift when it is Shift alone with `<Esc>`, `<CR>`, `<BS>` or `<Del>`. Shift is
/// often still held on these keys after a capital (`:W<S-CR>`), and most terminals send them as
/// the plain keys, so they act the same in Vim.
fn unshifted(key: Key) -> Key {
    let shift_alone = key.mods.shift && !(key.mods.ctrl || key.mods.alt || key.mods.cmd);
    match key.code {
        KeyCode::Esc | KeyCode::Enter | KeyCode::Backspace | KeyCode::Delete if shift_alone => {
            Key::named(key.code)
        }
        _ => key,
    }
}

/// The bounds of `object`, or a box of zero size at its anchor when it cannot be measured. The
/// caller counts the work `measure` reports for it ([`Engine::charge_measured`]).
fn bounds(object: &Object, measure: &mut dyn Measure) -> Rect {
    measure.bounds(object).unwrap_or_else(|| {
        let p = anchor(&object.shape);
        Rect::from_points(p, p)
    })
}
