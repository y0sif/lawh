//! Running `:` commands: the command line's keys, shapes, text and math at the cursor, writing
//! and quitting.

use kurbo::Point;

use super::cmdline::Outcome;
use super::command::{Command, CommandError, Segment, parse_command};
use super::register::id_prefix;
use super::undo::Edit;
use super::{Cursor, Effect, Engine, MAX_COMMAND_LINE_CHARS, Mode};
use crate::board::{
    BoardError, MAX_BOARD_TEXT_CHARS, MAX_COORDINATE, MAX_OBJECTS, MAX_SIZE, MAX_TEXT_CHARS,
    MAX_TEXT_LINES, Object, Shape,
};
use crate::key::Key;

/// A write the engine asked the app for, until the app reports back.
#[derive(Debug, Clone)]
pub(super) struct PendingWrite {
    path: String,
    quit_after: bool,
    /// The engine's change count when the write was asked for.
    version: u64,
}

impl Engine {
    /// The app wrote the board to `path` (as an [`Effect::Write`] asked), or failed to with
    /// `result`'s error. Returns [`Effect::Quit`] when the write was asked for by `:wq` or `:x`,
    /// went well, and the board has not changed since.
    ///
    /// A good write to the board's own file makes the board unmodified, unless it changed after
    /// the write was asked for. A write to another file (a copy, `:w other` on a named board)
    /// leaves it as it was, as in Vim.
    pub fn write_done(&mut self, path: &str, result: Result<(), String>) -> Vec<Effect> {
        let asked = self.pending_write.take_if(|w| w.path == path);
        let current = asked.as_ref().is_some_and(|w| w.version == self.version);
        if let Err(error) = result {
            self.error(format!("cannot write \"{path}\": {error}"));
            return Vec::new();
        }
        if current && self.file_name.as_deref() == Some(path) {
            self.history.mark_saved();
        }
        self.info(format!("\"{path}\" written"));
        if current && asked.is_some_and(|w| w.quit_after) {
            vec![Effect::Quit]
        } else {
            Vec::new()
        }
    }

    pub(super) fn command_line_key(&mut self, key: Key) -> Vec<Effect> {
        match self.command_line.key(key) {
            Outcome::Editing => Vec::new(),
            Outcome::Full => {
                self.error(format!(
                    "the command line holds at most {MAX_COMMAND_LINE_CHARS} characters"
                ));
                Vec::new()
            }
            Outcome::Cancel => {
                self.leave_command_line();
                Vec::new()
            }
            Outcome::Run(line) => {
                self.mode = Mode::Normal;
                let effects = self.execute_line(&line);
                self.leave_command_line();
                effects
            }
        }
    }

    /// Back from the command line: to block-Normal mode when it was opened there and the
    /// cursor is still on the block, else to Normal mode.
    fn leave_command_line(&mut self) {
        self.mode = if self.block.is_some() {
            Mode::BlockNormal
        } else {
            Mode::Normal
        };
        self.sync_block(None);
    }

    /// Reads and runs a `:` line, keeping it for `@:` (unless it is empty).
    pub(super) fn execute_line(&mut self, line: &str) -> Vec<Effect> {
        let command = parse_command(line);
        if command != Err(CommandError::Empty) {
            self.last_command_line = Some(line.to_owned());
        }
        match command {
            Ok(command) => self.run_command(&command),
            // `:` then `<CR>` does nothing, as in Vim.
            Err(CommandError::Empty) => Vec::new(),
            Err(error) => {
                self.error(error.to_string());
                Vec::new()
            }
        }
    }

    fn run_command(&mut self, command: &Command) -> Vec<Effect> {
        let at = self.cursor_point();
        let or_at =
            |x: Option<f64>, y: Option<f64>| Point::new(x.unwrap_or(at.x), y.unwrap_or(at.y));
        match command {
            Command::Circle { x, y, r } => self.add_shape(Shape::Ellipse {
                center: or_at(*x, *y),
                rx: *r,
                ry: *r,
            }),
            Command::Ellipse { x, y, rx, ry } => self.add_shape(Shape::Ellipse {
                center: or_at(*x, *y),
                rx: *rx,
                ry: *ry,
            }),
            Command::Rect { x, y, w, h } => self.add_shape(Shape::Rect {
                origin: or_at(*x, *y),
                w: *w,
                h: *h,
            }),
            Command::Line(segment) => {
                let (a, b) = ends(segment, at);
                self.add_shape(Shape::Line { a, b });
            }
            Command::Arrow(segment) => {
                let (a, b) = ends(segment, at);
                self.add_shape(Shape::Arrow { a, b });
            }
            Command::Text(text) => self.add_shape(Shape::Text {
                at,
                text: text.clone(),
            }),
            Command::Math(src) => self.add_shape(Shape::Math {
                at,
                src: src.clone(),
            }),
            Command::Write { file } => return self.write(file.as_deref(), false),
            Command::WriteQuit { file } => return self.write(file.as_deref(), true),
            Command::Exit { file } => {
                return if self.is_modified() {
                    self.write(file.as_deref(), true)
                } else {
                    vec![Effect::Quit]
                };
            }
            Command::Quit { force } => {
                if *force || !self.is_modified() {
                    return vec![Effect::Quit];
                }
                self.error("the board has changes: :w writes them, :q! quits without them");
            }
            Command::Undo => self.undo(1),
            Command::Redo => self.redo(1),
        }
        Vec::new()
    }

    /// Asks the app to write the board to `file`, or to the board's own file. An unnamed board
    /// takes `file` as its name; a named board writes a copy to it.
    fn write(&mut self, file: Option<&str>, quit_after: bool) -> Vec<Effect> {
        let path = match (file, &self.file_name) {
            (Some(file), None) => {
                self.file_name = Some(file.to_owned());
                file.to_owned()
            }
            (Some(file), Some(_)) => file.to_owned(),
            (None, Some(name)) => name.clone(),
            (None, None) => {
                self.error("no file name: :w <file> names the board");
                return Vec::new();
            }
        };
        self.pending_write = Some(PendingWrite {
            path: path.clone(),
            quit_after,
            version: self.version,
        });
        vec![Effect::Write { path, quit_after }]
    }

    /// Puts a new object on top of the board, with a fresh id for its kind (`c1`, `t2`), as one
    /// undo step, and makes it the current object. A refusal becomes the message.
    fn add_shape(&mut self, shape: Shape) {
        if !shape.coordinates_in_range() {
            return self.error(coordinates_message());
        }
        let id = self.board.fresh_id(id_prefix(&shape));
        let edit = Edit::Insert {
            index: self.board.len(),
            object: Object::new(id.clone(), shape),
        };
        if let Err(error) = edit.apply(&mut self.board) {
            return self.error(refusal(&error));
        }
        let before = std::mem::replace(&mut self.cursor, Cursor::Object(id));
        self.commit(vec![edit], before);
    }
}

/// The two ends of a line or an arrow, the start defaulting to the cursor point `at`.
fn ends(segment: &Segment, at: Point) -> (Point, Point) {
    let a = Point::new(segment.x1.unwrap_or(at.x), segment.y1.unwrap_or(at.y));
    let b = Point::new(segment.x2.resolve(a.x), segment.y2.resolve(a.y));
    (a, b)
}

fn coordinates_message() -> String {
    format!("coordinates must be within -{MAX_COORDINATE}..={MAX_COORDINATE}")
}

pub(super) fn too_much_text() -> String {
    format!("a board holds at most {MAX_BOARD_TEXT_CHARS} characters of text and math in all")
}

/// Why the board refused a new object, without naming its id: the object never reached the
/// board.
pub(super) fn refusal(error: &BoardError) -> String {
    match error {
        BoardError::TooManyObjects(_) => {
            format!("the board is full: it holds at most {MAX_OBJECTS} objects")
        }
        BoardError::TextTooLong(_) => {
            format!("a text or formula holds at most {MAX_TEXT_CHARS} characters")
        }
        BoardError::TooManyLines(_) => {
            format!("a text or formula holds at most {MAX_TEXT_LINES} lines")
        }
        BoardError::TooMuchText(_) => too_much_text(),
        BoardError::CoordinateOutOfRange(_) => coordinates_message(),
        BoardError::NonFinite(_) => "every number must be finite".to_owned(),
        BoardError::InvalidSize(_) => format!("sizes must be above 0 and at most {MAX_SIZE}"),
        other => other.to_string(),
    }
}
