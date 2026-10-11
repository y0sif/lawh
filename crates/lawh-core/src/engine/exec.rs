//! Running `:` commands: the command line's keys, shapes, text and math at the cursor, writing
//! and quitting. Mapping commands and `:set` run in `keymap.rs` and `settings.rs`.

use std::path::{Component, Path};

use kurbo::Point;

use super::cmdline::Outcome;
use super::command::{Command, CommandError, Segment, parse_command};
use super::register::id_prefix;
use super::undo::Edit;
use super::work::PARSE_WORK;
use super::{Cursor, Effect, Engine, MAX_COMMAND_LINE_CHARS, Mode};
use crate::board::{
    BoardError, MAX_BOARD_TEXT_CHARS, MAX_COORDINATE, MAX_OBJECTS, MAX_SIZE, MAX_TEXT_CHARS,
    MAX_TEXT_LINES, Object, Shape,
};
use crate::key::Key;

/// Most writes one typed key (or a timeout) may ask for: 4.
///
/// For each write the engine asks for, the app turns the board into text, writes it to a new
/// file, flushes it to disk and renames it, unless it wrote the same file for the same key
/// already. One write took 2.76 ms on an SSD, most of it the flush. Before this cap, a mapping
/// such as `nmap Q :w<CR>Q` asked for 1,000 writes (stopped only by
/// [`MAX_MAP_DEPTH`](super::MAX_MAP_DEPTH)), 2.8 s of frozen window, and one of 85 `:w<CR>`
/// then itself for 9,945 (stopped by [`MAX_EXPANDED_KEYS`](super::MAX_EXPANDED_KEYS)), 27 s.
/// [`MAX_KEY_WORK`](super::MAX_KEY_WORK) does not bound them: the engine's part of a write is
/// only reading its `:` line (39,780 units for those 9,945).
///
/// Four writes take about 11 ms, about what a key that runs out of work takes in release, and
/// leave room for a mapping that writes the board and a copy or two (`:w<CR>:w copy.lawh<CR>`).
/// A write past the cap is refused, and the key stops as when it runs out of work: nothing more
/// runs for it (a `:wq` or `:x` refused does not quit), the typeahead is dropped and the message
/// says so. The count starts again with the next typed key, timeout or click.
pub const MAX_WRITES_PER_KEY: usize = 4;

/// Most writes that may wait for the app to report them with [`Engine::write_done`]: as many as
/// one key may ask for ([`MAX_WRITES_PER_KEY`]).
///
/// The app writes and reports right after the key that asked, so only the writes of one key
/// wait together, and they all fit: a mapping such as `:w<CR>:w copy.lawh<CR>` asks for two.
/// Each report is matched to the oldest write waiting with the same path, so every write can
/// mark the board saved.
///
/// Only an app that leaves writes unreported fills the queue: then the oldest write is
/// forgotten to make room. Reports are still matched oldest first, so when another write
/// waiting has the forgotten one's path, the forgotten write's report takes that one, and it is
/// the newest report for the path that finds no write waiting. A report that finds none reads
/// as a write the engine did not ask for, which neither marks the board saved nor quits, the
/// safe side.
pub const MAX_PENDING_WRITES: usize = MAX_WRITES_PER_KEY;

/// A write the engine asked the app for, until the app reports back.
#[derive(Debug, Clone)]
pub(super) struct PendingWrite {
    path: String,
    quit_after: bool,
    /// `:wq!` or `:x!`: quit after the write even when the board's own file lacks its changes.
    force: bool,
    /// The engine's change count when the write was asked for.
    version: u64,
}

/// Why `:q` (and `:wq` to another file) does not quit.
const UNSAVED: &str = "the board has changes: :w writes them, :q! quits without them";

impl Engine {
    /// The app wrote the board to `path` (as an [`Effect::Write`] asked), or failed to with
    /// `result`'s error. Returns [`Effect::Quit`] when the write was asked for by `:wq` or `:x`,
    /// went well, the board has not changed since, and nothing is left unsaved.
    ///
    /// A good write to the board's own file makes the board unmodified, unless it changed after
    /// the write was asked for. The file is the board's own when the two names are the same
    /// path once `.` and `..` are worked out, without asking the file system (`./a.lawh` and
    /// `dir/../a.lawh` are `a.lawh`). A write to another file (a copy, `:w other` on a named
    /// board) leaves the board as it was, as in Vim, so `:wq other` on a modified board writes
    /// the copy and does not quit, unless forced with `:wq!`.
    ///
    /// Several writes may wait at once (`:w<CR>:w copy.lawh<CR>` in a mapping): each report is
    /// matched to the oldest write asked for with the same path.
    pub fn write_done(&mut self, path: &str, result: Result<(), String>) -> Vec<Effect> {
        let asked = self
            .pending_writes
            .iter()
            .position(|w| w.path == path)
            .and_then(|i| self.pending_writes.remove(i));
        let current = asked.as_ref().is_some_and(|w| w.version == self.version);
        if let Err(error) = result {
            self.error(format!("cannot write \"{path}\": {error}"));
            return Vec::new();
        }
        let own = self
            .file_name
            .as_deref()
            .is_some_and(|name| same_file(name, path));
        if current && own {
            self.history.mark_saved();
        }
        let Some(asked) = asked.filter(|w| current && w.quit_after) else {
            self.info(format!("\"{path}\" written"));
            return Vec::new();
        };
        if self.is_modified() && !asked.force {
            self.error(format!("\"{path}\" written, but {UNSAVED}"));
            return Vec::new();
        }
        self.info(format!("\"{path}\" written"));
        vec![Effect::Quit]
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

    /// Reads and runs a `:` line, keeping it for `@:` (unless it is empty). Reading it counts as
    /// work (see [`MAX_KEY_WORK`](super::MAX_KEY_WORK)): `@:` runs a line of thousands of
    /// characters for two keys. Once the key has run out, the line does not run.
    pub(super) fn execute_line(&mut self, line: &str) -> Vec<Effect> {
        if !self.charge(PARSE_WORK.saturating_mul(line.len())) {
            return Vec::new();
        }
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
            Command::Write { file } => return self.write(file.as_deref(), None),
            Command::WriteQuit { file, force } => return self.write(file.as_deref(), Some(*force)),
            Command::Exit { file, force } => {
                return if self.is_modified() {
                    self.write(file.as_deref(), Some(*force))
                } else {
                    vec![Effect::Quit]
                };
            }
            Command::Quit { force } => {
                if *force || !self.is_modified() {
                    return vec![Effect::Quit];
                }
                self.error(UNSAVED);
            }
            Command::Undo => self.undo(1),
            Command::Redo => self.redo(1),
            Command::Map { .. }
            | Command::ShowMaps { .. }
            | Command::Unmap { .. }
            | Command::MapClear { .. }
            | Command::Set(_) => match self.run_setup(command) {
                Ok(Some(text)) => self.info(text),
                Ok(None) => {}
                Err(text) => self.error(text),
            },
        }
        Vec::new()
    }

    /// Asks the app to write the board to `file`, or to the board's own file. An unnamed board
    /// takes `file` as its name; a named board writes a copy to it. `quit`: quit after the
    /// write, forced (`Some(true)`) or not.
    ///
    /// The key's writes are counted: past [`MAX_WRITES_PER_KEY`] the write is refused (the
    /// board keeps its name, or lack of one) and the key stops.
    fn write(&mut self, file: Option<&str>, quit: Option<bool>) -> Vec<Effect> {
        let Some(path) = file.or(self.file_name.as_deref()).map(str::to_owned) else {
            self.error("no file name: :w <file> names the board");
            return Vec::new();
        };
        if !self.charge_write() {
            return Vec::new();
        }
        if self.file_name.is_none() {
            self.file_name = Some(path.clone());
        }
        let quit_after = quit.is_some();
        if self.pending_writes.len() >= MAX_PENDING_WRITES {
            self.pending_writes.pop_front();
        }
        self.pending_writes.push_back(PendingWrite {
            path: path.clone(),
            quit_after,
            force: quit == Some(true),
            version: self.version,
        });
        vec![Effect::Write { path, quit_after }]
    }

    /// Puts a new object on top of the board, with a fresh id for its kind (`c1`, `t2`), as one
    /// undo step, and makes it the current object. A refusal becomes the message. Finding the
    /// id counts as work (see [`MAX_KEY_WORK`](super::MAX_KEY_WORK)).
    fn add_shape(&mut self, shape: Shape) {
        if !shape.coordinates_in_range() {
            return self.error(coordinates_message());
        }
        if !self.charge(self.id_work()) {
            return;
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

/// `a` and `b` name the same file as far as their text tells: the same parts once `.` parts
/// are dropped and each `..` takes back the part before it. The file system is not asked, so
/// links are not followed (`link/../a` is `a`). Parts are split as the platform splits paths:
/// on `/`, and on Windows on `\` too.
fn same_file(a: &str, b: &str) -> bool {
    lexical_parts(a) == lexical_parts(b)
}

/// The parts of `path` with `.` dropped and each `..` taking back the part before it. A `..`
/// with no part before it stays (`../a`), except right after the root, where it does nothing
/// (`/..` is `/`).
fn lexical_parts(path: &str) -> Vec<Component<'_>> {
    let mut parts: Vec<Component<'_>> = Vec::new();
    for part in Path::new(path).components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => match parts.last() {
                Some(Component::Normal(_)) => {
                    parts.pop();
                }
                Some(Component::RootDir) => {}
                _ => parts.push(part),
            },
            Component::Normal(_) | Component::RootDir | Component::Prefix(_) => parts.push(part),
        }
    }
    parts
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_file_works_out_dots_without_the_file_system() {
        for (a, b) in [
            ("a.lawh", "./a.lawh"),
            ("a.lawh", "dir/../a.lawh"),
            ("a.lawh", "a/b/../../a.lawh"),
            ("dir/a.lawh", "dir/./a.lawh"),
            ("dir/a.lawh", "dir//a.lawh"),
            ("/a.lawh", "/../a.lawh"),
            ("../a.lawh", "./../a.lawh"),
        ] {
            assert!(same_file(a, b), "{a} {b}");
            assert!(same_file(b, a), "{b} {a}");
        }
        for (a, b) in [
            ("a.lawh", "../a.lawh"),
            ("a.lawh", "dir/a.lawh"),
            ("a.lawh", "/a.lawh"),
            ("a.lawh", "a.lawh/.."),
            ("a.lawh", "A.lawh"),
        ] {
            assert!(!same_file(a, b), "{a} {b}");
        }
    }
}
