//! The editing engine: a state machine that takes keys, changes the board and asks the app for
//! what it cannot do itself (writing files, quitting) through [`Effect`]s.
//!
//! [`Engine::feed`] takes one key at a time. Each mode has its own dispatcher. In Normal mode,
//! keys gather in a pending buffer (a count, then the command's keys) until they make a whole
//! command from the `NORMAL` table, or none can. Every command that changes the board makes
//! one undo step.
//!
//! The cursor is a grid point or an object (see [`Cursor`]). Normal mode so far has `h j k l`
//! (one grid unit a step), `u` and `<C-r>`, all with counts, and `:` for the command line.

mod cmdline;
mod command;
mod cursor;
mod measure;
mod undo;

use kurbo::Point;

pub use cmdline::MAX_COMMAND_LINE_CHARS;
pub use command::{Command, CommandError, parse_command};
pub use cursor::{Cursor, anchor, round3};
pub use measure::{FixedMeasure, Measure, shape_bounds};
pub use undo::{MAX_UNDO_BYTES, MAX_UNDO_STEPS};

use self::cmdline::{CommandLine, Outcome};
use self::undo::{Edit, History, Step};
use crate::board::{Board, Object, Shape};
use crate::key::{Key, KeyCode, format_keys};

/// Largest count before a command; typing more digits keeps it here.
///
/// No motion, undo or put needs more: 10,000 grid units is far past any board on screen, and
/// [`MAX_UNDO_STEPS`] is lower still. Saturating keeps a count such as `999999999` cheap to act
/// on, or to refuse.
pub const MAX_COUNT: u32 = 10_000;

/// The engine's mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    /// Typing a `:` command (see [`Engine::command_line`]).
    CommandLine,
}

/// Something the app must do for the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Write the board ([`to_lawh`](crate::to_lawh) of [`Engine::board`]) to `path`
    /// atomically, then report back to the engine; quit after a good write if `quit_after`.
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

/// A Normal-mode command, run with a count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Normal {
    Left,
    Down,
    Up,
    Right,
    Undo,
    Redo,
    CommandLine,
}

/// Normal-mode commands by the keys that run them, after any count.
const NORMAL: &[(&[Key], Normal)] = &[
    (&[Key::char('h')], Normal::Left),
    (&[Key::char('j')], Normal::Down),
    (&[Key::char('k')], Normal::Up),
    (&[Key::char('l')], Normal::Right),
    (&[Key::char('u')], Normal::Undo),
    (&[Key::ctrl('r')], Normal::Redo),
    (&[Key::char(':')], Normal::CommandLine),
];

/// What the pending keys make in [`NORMAL`].
enum Lookup {
    Command(Normal),
    /// The start of a longer command: wait for more keys.
    Prefix,
    /// Nothing: drop the keys.
    Nothing,
}

fn lookup(keys: &[Key]) -> Lookup {
    let mut prefix = false;
    for (command_keys, command) in NORMAL {
        if *command_keys == keys {
            return Lookup::Command(*command);
        }
        prefix |= command_keys.len() > keys.len() && command_keys.starts_with(keys);
    }
    if prefix {
        Lookup::Prefix
    } else {
        Lookup::Nothing
    }
}

/// Normal-mode keys typed so far that do not make a whole command yet.
#[derive(Debug, Clone, Default)]
struct Pending {
    /// The count typed before the command, at most [`MAX_COUNT`].
    count: Option<u32>,
    /// The command's keys so far, after the count.
    keys: Vec<Key>,
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

    /// Takes one key. Keys are made canonical first (see [`Key::new`]).
    ///
    /// `_measure` gives the bounds of text and math, for the object motions and placing that
    /// come next; nothing needs it yet.
    pub fn feed(&mut self, key: Key, _measure: &mut dyn Measure) -> Vec<Effect> {
        let key = key.canonical();
        self.message = None;
        match self.mode {
            Mode::Normal => self.normal_key(key),
            Mode::CommandLine => self.command_line_key(key),
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

    /// Keys typed but not yet run, in Vim notation (`3`, later `3d`), for the status line. A
    /// count past [`MAX_COUNT`] shows as `MAX_COUNT`.
    pub fn pending(&self) -> String {
        let count = self
            .pending
            .count
            .map(|n| n.to_string())
            .unwrap_or_default();
        count + &format_keys(&self.pending.keys)
    }

    pub fn message(&self) -> Option<&Message> {
        self.message.as_ref()
    }

    pub fn file_name(&self) -> Option<&str> {
        self.file_name.as_deref()
    }

    /// The board has changed since it was opened or last saved (undoing back to that point
    /// makes it unmodified again).
    pub fn is_modified(&self) -> bool {
        self.history.is_modified()
    }

    fn error(&mut self, text: impl Into<String>) {
        self.message = Some(Message {
            text: text.into(),
            error: true,
        });
    }

    fn normal_key(&mut self, key: Key) -> Vec<Effect> {
        if let Some(digit) = self.count_digit(key) {
            let count = self.pending.count.unwrap_or(0);
            let count = count.saturating_mul(10).saturating_add(digit);
            self.pending.count = Some(count.min(MAX_COUNT));
            return Vec::new();
        }
        self.pending.keys.push(key);
        match lookup(&self.pending.keys) {
            Lookup::Prefix => Vec::new(),
            Lookup::Nothing => {
                self.pending = Pending::default();
                Vec::new()
            }
            Lookup::Command(command) => {
                let count = self.pending.count.unwrap_or(1);
                self.pending = Pending::default();
                self.run_normal(command, count)
            }
        }
    }

    /// The digit `key` adds to the count, if it is part of one: `1` to `9` start a count, `0`
    /// only goes on one.
    fn count_digit(&self, key: Key) -> Option<u32> {
        if !self.pending.keys.is_empty() || !key.mods.is_empty() {
            return None;
        }
        let KeyCode::Char(c) = key.code else {
            return None;
        };
        let digit = c.to_digit(10)?;
        (digit != 0 || self.pending.count.is_some()).then_some(digit)
    }

    fn run_normal(&mut self, command: Normal, count: u32) -> Vec<Effect> {
        let steps = f64::from(count);
        match command {
            Normal::Left => self.move_by(-steps, 0.0),
            Normal::Down => self.move_by(0.0, steps),
            Normal::Up => self.move_by(0.0, -steps),
            Normal::Right => self.move_by(steps, 0.0),
            Normal::Undo => self.undo(count),
            Normal::Redo => self.redo(count),
            Normal::CommandLine => {
                self.command_line.clear();
                self.mode = Mode::CommandLine;
            }
        }
        Vec::new()
    }

    /// Moves the cursor point by whole grid units; an object cursor becomes a grid point.
    fn move_by(&mut self, dx: f64, dy: f64) {
        self.cursor = Cursor::Grid(cursor::offset(self.cursor_point(), dx, dy));
    }

    fn command_line_key(&mut self, key: Key) -> Vec<Effect> {
        match self.command_line.key(key) {
            Outcome::Editing => Vec::new(),
            Outcome::Full => {
                self.error(format!(
                    "the command line holds at most {MAX_COMMAND_LINE_CHARS} characters"
                ));
                Vec::new()
            }
            Outcome::Cancel => {
                self.mode = Mode::Normal;
                Vec::new()
            }
            Outcome::Run(line) => {
                self.mode = Mode::Normal;
                match parse_command(&line) {
                    Ok(command) => self.run_command(&command),
                    // `:` then `<CR>` does nothing, as in Vim.
                    Err(CommandError::Empty) => Vec::new(),
                    Err(error) => {
                        self.error(error.to_string());
                        Vec::new()
                    }
                }
            }
        }
    }

    fn run_command(&mut self, command: &Command) -> Vec<Effect> {
        match *command {
            Command::Circle { x, y, r } => {
                let at = self.cursor_point();
                let center = Point::new(x.unwrap_or(at.x), y.unwrap_or(at.y));
                self.add_object(
                    "c",
                    Shape::Ellipse {
                        center,
                        rx: r,
                        ry: r,
                    },
                );
            }
            Command::Undo => self.undo(1),
            Command::Redo => self.redo(1),
        }
        Vec::new()
    }

    /// Puts a new object on top of the board, with a fresh id from `prefix` (`c1`), as one undo
    /// step, and makes it the current object. A refusal becomes the message.
    fn add_object(&mut self, prefix: &str, shape: Shape) {
        let id = self.board.fresh_id(prefix);
        let edit = Edit::Insert {
            index: self.board.len(),
            object: Object::new(id.clone(), shape),
        };
        if let Err(error) = edit.apply(&mut self.board) {
            self.error(error.to_string());
            return;
        }
        let cursor_before = std::mem::replace(&mut self.cursor, Cursor::Object(id));
        self.history.push(Step {
            edits: vec![edit],
            cursor_before,
            cursor_after: self.cursor.clone(),
        });
    }

    /// Takes back up to `count` steps, putting the cursor where it was before the last one.
    fn undo(&mut self, count: u32) {
        for done in 0..count {
            match self.history.undo(&mut self.board) {
                Some(Ok(cursor)) => self.cursor = cursor,
                Some(Err(error)) => return self.error(format!("cannot undo: {error}")),
                None if done == 0 => return self.error("already at the oldest change"),
                None => return,
            }
        }
    }

    /// Makes again up to `count` undone steps, putting the cursor where it was after the last.
    fn redo(&mut self, count: u32) {
        for done in 0..count {
            match self.history.redo(&mut self.board) {
                Some(Ok(cursor)) => self.cursor = cursor,
                Some(Err(error)) => return self.error(format!("cannot redo: {error}")),
                None if done == 0 => return self.error("already at the newest change"),
                None => return,
            }
        }
    }
}
