//! Running `:` commands: the command line's keys, shapes at the cursor, text and math at the
//! cursor or below the current object, writing, exporting and quitting. Mapping commands and
//! `:set` run in `keymap.rs` and `settings.rs`, `:si` in `size.rs`, `:sym` and its completions
//! in `sym.rs`.

use std::fmt::Write as _;
use std::path::{Component, Path};

use kurbo::Point;

use super::cmdline::Outcome;
use super::command::{Command, CommandError, Segment, parse_command};
use super::place::{Side, place, translate};
use super::register::id_prefix;
use super::size;
use super::sym::is_sym_command;
use super::undo::Edit;
use super::work::{EXPORT_OBJECT_WORK, EXPORT_WORK, MEASURE_WORK, PARSE_WORK, PLACE_WORK};
use super::{Cursor, Effect, Engine, MAX_COMMAND_LINE_CHARS, Measure, Mode, bounds};
use crate::board::{
    BoardError, MAX_BOARD_TEXT_CHARS, MAX_COORDINATE, MAX_OBJECTS, MAX_SIZE, MAX_TEXT_CHARS,
    MAX_TEXT_LINES, Object, ObjectId, Shape,
};
use crate::export::{ExportFormat, ExportReport};
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
/// as a write the engine did not ask for, which neither names the board, nor marks it saved,
/// nor quits, the safe side.
pub const MAX_PENDING_WRITES: usize = MAX_WRITES_PER_KEY;

/// Most exports one typed key (or a timeout) may ask for: 1. Each also counts as a write
/// against [`MAX_WRITES_PER_KEY`], since the app writes its file the same way.
///
/// For each export the app draws the whole board (or the selection) into the file's bytes on
/// the window's thread, then writes them: it waits for every formula to compile (up to 10 s
/// for them all, `EXPORT_MATH_WAIT` in lawh-render) and may draw for up to 2 s of estimated
/// work (`EXPORT_BUDGET_NS`), its PNG encoding included. Measured in release (lawh-render's
/// `measure_exports` and `measure_pdf_exports`, `docs/research/m4-measurements.md`):
/// `examples/area.lawh` as a PNG at scale 2 in 10 to 41 ms, a PDF in about 1 ms; the limits
/// board as a PNG shrunk to 64 million pixels in 0.39 to 0.41 s; 500 formulas of 22,500 glyphs
/// as a PDF 1.1 s before it was refused for its size. So one export may hold the window for
/// seconds, and a mapping that exports twice (`:exp a.png<CR>:exp b.pdf<CR>`) or runs itself
/// would hold it for as many times that.
///
/// An export past the cap is refused, and the key stops as at [`MAX_WRITES_PER_KEY`]: nothing
/// more runs for it, the typeahead is dropped and the message says so. The exports and writes
/// asked for before it still go. The count starts again with the next typed key, timeout or
/// click.
pub const MAX_EXPORTS_PER_KEY: usize = 1;

/// Why `:exp` asks for no export: the board, or the selection it was typed from, is empty.
const NOTHING_TO_EXPORT: &str = "nothing to export";

/// The smallest shrink factor [`Engine::export_done`] names: a factor below it shows as
/// `shrunk to fit`, with no number. A board within its caps never gets there: the smallest
/// factor is a PNG at scale 8 of a square board 4,000,000 units across, from `MAX_COORDINATE`
/// to `-MAX_COORDINATE` and a size beyond, shrunk to 64 million pixels, about 0.000006.
const MIN_SHOWN_SHRINK: f64 = 1e-6;

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
    /// An unnamed board takes `path` as its name when the write went well (`:w a.lawh`), so a
    /// write that failed (a typo in a directory) leaves it unnamed and the next `:w name` can
    /// name it. When several writes of one key would name it (`:w a.lawh<CR>:w b.lawh<CR>` in a
    /// mapping), the first reported to have gone well does, and the others are copies.
    ///
    /// A good write to the board's own file makes the board unmodified, unless it changed after
    /// the write was asked for. The file is the board's own when `same_file` says so (the app
    /// found, after the write, that `path` is the board's file under another name, such as
    /// through a link), or when the two names are the same path once `.` and `..` are worked
    /// out, without asking the file system (`./a.lawh` and `dir/../a.lawh` are `a.lawh`). A
    /// write to another file (a copy, `:w other` on a named board) leaves the board as it was,
    /// as in Vim, so `:wq other` on a modified board writes the copy and does not quit, unless
    /// forced with `:wq!`.
    ///
    /// Several writes may wait at once (`:w<CR>:w copy.lawh<CR>` in a mapping): each report is
    /// matched to the oldest write asked for with the same path. A report that matches none
    /// neither names the board, nor marks it saved, nor quits.
    ///
    /// The report becomes the message as the key's own messages do (see [`Message`]): a failure
    /// is an error and replaces any message, while `"a.lawh" written` does not replace an error
    /// the key left, so it hides neither a failed copy reported before it nor a key refused for
    /// asking too many writes.
    ///
    /// [`Message`]: super::Message
    pub fn write_done(
        &mut self,
        path: &str,
        result: Result<(), String>,
        same_file: bool,
    ) -> Vec<Effect> {
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
        if self.file_name.is_none() && asked.is_some() {
            self.file_name = Some(path.to_owned());
        }
        let own = same_file
            || self
                .file_name
                .as_deref()
                .is_some_and(|name| same_path(name, path));
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

    /// The app exported the board to `path` (as an [`Effect::Export`] asked), or failed to with
    /// `result`'s error. This only sets the message: an export never names the board nor marks
    /// it saved, since the file it writes is a picture, not a board to open again.
    ///
    /// A good export says `"out.pdf" exported`, then `, 1 formula failed` or
    /// `, 2 formulas failed` when formulas were drawn as their source, in the error colour (as on
    /// screen), and `, shrunk to 0.5x to fit` when the renderer made it smaller than asked to fit
    /// a size cap. The factor has two significant digits, rounded down, so it never reads as 1:
    /// 0.995 is `0.99x`, 0.000164 is `0.00016x`. A failure is the error
    /// `cannot export "out.pdf": {error}`.
    ///
    /// The report becomes the message as [`Engine::write_done`]'s does (see [`Message`]): a
    /// failure is an error and replaces any message, while a good export does not replace an
    /// error the key left, such as a key refused for asking a second export.
    ///
    /// [`Message`]: super::Message
    pub fn export_done(&mut self, path: &str, result: Result<ExportReport, String>) {
        let report = match result {
            Ok(report) => report,
            Err(error) => return self.error(format!("cannot export \"{path}\": {error}")),
        };
        let mut text = format!("\"{path}\" exported");
        match report.failed_formulas {
            0 => {}
            1 => text.push_str(", 1 formula failed"),
            n => {
                let _ = write!(text, ", {n} formulas failed");
            }
        }
        if let Some(factor) = report.shrunk {
            match shown_factor(factor) {
                Some(factor) => {
                    let _ = write!(text, ", shrunk to {factor}x to fit");
                }
                None => text.push_str(", shrunk to fit"),
            }
        }
        self.info(text);
    }

    /// A key on the command line: the completion keys of a `:sym` line, else editing, `<Esc>`
    /// and `<CR>`. A line opened by `<C-k>` from Insert mode runs only `:sym`, and goes back to
    /// Insert mode (see `sym.rs`).
    pub(super) fn command_line_key(&mut self, key: Key, measure: &mut dyn Measure) -> Vec<Effect> {
        if self.completion_key(key) {
            return Vec::new();
        }
        match self.command_line.key(key) {
            Outcome::Editing => {
                self.update_completions();
                Vec::new()
            }
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
            Outcome::Run(line) if self.session.is_some() => {
                if is_sym_command(&line) {
                    self.execute_line(&line, measure);
                } else if parse_command(&line) != Err(CommandError::Empty) {
                    self.error("from Insert mode the command line runs only `:sym`");
                }
                self.leave_command_line();
                Vec::new()
            }
            Outcome::Run(line) => {
                self.mode = Mode::Normal;
                let effects = self.execute_line(&line, measure);
                self.leave_command_line();
                effects
            }
        }
    }

    /// Back from the command line: to the Insert session `<C-k>` opened it from; else to
    /// block-Normal mode when it was opened there and the cursor is still on the block, else to
    /// Normal mode. A selection kept from Visual mode is dropped.
    fn leave_command_line(&mut self) {
        self.completions = None;
        if self.session.is_some() {
            self.mode = Mode::Insert;
            return;
        }
        if self.visual_command {
            self.leave_visual();
        }
        self.mode = if self.block.is_some() {
            Mode::BlockNormal
        } else {
            Mode::Normal
        };
        self.sync_block(None);
    }

    /// Reads and runs a `:` line, keeping it for `@:` (unless it is empty). Reading it counts as
    /// work (see [`MAX_KEY_WORK`](super::MAX_KEY_WORK)): `@:` runs a line of thousands of
    /// characters for two keys. Once the key has run out, the line does not run. `measure` is
    /// for `:te`, `:ma` and `:sym` on an object, which place their block below it.
    pub(super) fn execute_line(&mut self, line: &str, measure: &mut dyn Measure) -> Vec<Effect> {
        if !self.charge(PARSE_WORK.saturating_mul(line.len())) {
            return Vec::new();
        }
        let command = parse_command(line);
        if command != Err(CommandError::Empty) {
            self.last_command_line = Some(line.to_owned());
        }
        match command {
            Ok(command) => self.run_command(&command, measure),
            // `:` then `<CR>` does nothing, as in Vim.
            Err(CommandError::Empty) => Vec::new(),
            Err(error) => {
                self.error(error.to_string());
                Vec::new()
            }
        }
    }

    fn run_command(&mut self, command: &Command, measure: &mut dyn Measure) -> Vec<Effect> {
        let at = self.cursor_point();
        let new_size = self.settings.size();
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
            Command::Text { text, size } => {
                let shape = Shape::Text {
                    at,
                    text: text.clone(),
                    size: size.unwrap_or(new_size),
                };
                self.add_block(shape, measure);
            }
            Command::Math { src, size } => {
                let shape = Shape::Math {
                    at,
                    src: src.clone(),
                    size: size.unwrap_or(new_size),
                };
                self.add_block(shape, measure);
            }
            Command::Size(change) => self.size_command(*change),
            Command::Sym { query } => self.pick_symbol(query, measure),
            Command::Write { file } => return self.write(file.as_deref(), None),
            Command::WriteQuit { file, force } => return self.write(file.as_deref(), Some(*force)),
            Command::Exit { file, force } => {
                return if self.is_modified() {
                    self.write(file.as_deref(), Some(*force))
                } else {
                    vec![Effect::Quit]
                };
            }
            Command::Export { file, format } => return self.export(file, *format),
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

    /// Asks the app to write the board to `file`, or to the board's own file
    /// ([`Engine::write_name`]). An unnamed board takes `file` as its name once the write goes
    /// well (see [`Engine::write_done`]); a named board writes a copy to it. `quit`: quit after
    /// the write, forced (`Some(true)`) or not.
    ///
    /// The key's writes are counted: past [`MAX_WRITES_PER_KEY`] the write is refused (the
    /// board keeps its name, or lack of one) and the key stops.
    fn write(&mut self, file: Option<&str>, quit: Option<bool>) -> Vec<Effect> {
        let Some(path) = file.or_else(|| self.write_name()).map(str::to_owned) else {
            self.error("no file name: :w <file> names the board");
            return Vec::new();
        };
        if !self.charge_write() {
            return Vec::new();
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

    /// `:exp[ort]`: asks the app to export the board to `path` in `format`, with the `exportbg`
    /// and `pngscale` options, or only the objects selected when the command line was opened
    /// from Visual mode (as `:si` takes them). An empty board, or an empty selection, has
    /// nothing to export: the message says so and no export is asked for.
    ///
    /// The key's exports are counted, each as a write too: past [`MAX_EXPORTS_PER_KEY`] (or
    /// [`MAX_WRITES_PER_KEY`]) the export is refused and the key stops. The work counted is
    /// `EXPORT_WORK`, and `EXPORT_OBJECT_WORK` for each object selected.
    fn export(&mut self, path: &str, format: ExportFormat) -> Vec<Effect> {
        let selection = if self.visual_command {
            if !self.charge(EXPORT_OBJECT_WORK.saturating_mul(self.selection.len())) {
                return Vec::new();
            }
            let ids: Vec<ObjectId> = self
                .selection
                .iter()
                .filter(|id| self.board.contains(id))
                .cloned()
                .collect();
            Some(ids)
        } else {
            None
        };
        if selection
            .as_ref()
            .map_or(self.board.is_empty(), Vec::is_empty)
        {
            self.error(NOTHING_TO_EXPORT);
            return Vec::new();
        }
        if !self.charge(EXPORT_WORK) || !self.charge_export() {
            return Vec::new();
        }
        vec![Effect::Export {
            path: path.to_owned(),
            options: self.settings.export_options(format),
            selection,
        }]
    }

    /// The file `:w` with no name writes: the board's own. An unnamed board has none, unless
    /// writes that would name it wait for their reports: then the oldest one's file, so that a
    /// mapping such as `:w a.lawh<CR>:wq<CR>` writes `a.lawh` and quits, as if the first write
    /// had named the board at once. Every write waiting on an unnamed board would name it, as a
    /// board never loses its name.
    fn write_name(&self) -> Option<&str> {
        self.file_name
            .as_deref()
            .or_else(|| self.pending_writes.front().map(|w| w.path.as_str()))
    }

    /// Puts a new object on top of the board, with a fresh id for its kind (`c1`, `t2`), as one
    /// undo step, and makes it the current object. A refusal becomes the message. Finding the
    /// id counts as work (see [`MAX_KEY_WORK`](super::MAX_KEY_WORK)).
    pub(super) fn add_shape(&mut self, shape: Shape) {
        if !shape.coordinates_in_range() {
            return self.error(coordinates_message());
        }
        if !self.charge(self.id_work()) {
            return;
        }
        let id = self.board.fresh_id(id_prefix(&shape));
        self.insert_object(Object::new(id, shape));
    }

    /// `:te`, `:ma` and `:sym` on the board: a new text or math block, `shape`, made as
    /// [`Engine::add_shape`] makes one. On a grid point it goes there (`shape` is at the cursor
    /// point already). On an object it goes below it by the place rule, as `o` opens a block:
    /// half a unit under the object, left edges lined up, past any object in the way (see
    /// `place.rs`), so it never lands on the object as it did at the object's anchor. The
    /// block's own bounds, as measured with its text, are the box placed.
    ///
    /// Measuring the board and the block, finding the id and the place rule's looks count as
    /// work: when the key runs out, no block is made. With no room below within
    /// [`MAX_COORDINATE`], the message says so.
    pub(super) fn add_block(&mut self, shape: Shape, measure: &mut dyn Measure) {
        let Cursor::Object(current) = &self.cursor else {
            return self.add_shape(shape);
        };
        let current = current.clone();
        let own_work = MEASURE_WORK.saturating_mul(shape.content_chars().saturating_add(1));
        let work = self
            .layout_work()
            .saturating_add(own_work)
            .saturating_add(self.id_work());
        if !self.charge(work) {
            return;
        }
        let obstacles = self.layout(measure);
        if self.out_of_work() {
            return;
        }
        let Some(reference) = self
            .index_of(&current)
            .and_then(|i| obstacles.get(i).copied())
        else {
            // The engine keeps an object cursor on an object; at its point, as a fallback.
            return self.add_shape(shape);
        };
        let object = Object::new(self.board.fresh_id(id_prefix(&shape)), shape);
        let own = bounds(&object, measure);
        if !self.charge_measured(measure) {
            return;
        }
        let mut looked = 0;
        let at = place(reference, own.size(), &obstacles, Side::Below, &mut looked);
        if !self.charge(PLACE_WORK.saturating_mul(looked)) {
            return;
        }
        let placed = at
            .map(|at| {
                let by = at - own.origin();
                translate(&object.shape, by.x, by.y)
            })
            .filter(Shape::coordinates_in_range);
        match placed {
            Some(shape) => self.insert_object(Object::new(object.id, shape)),
            None => self.error(match object.shape {
                Shape::Math { .. } => "no room for a new math block",
                _ => "no room for a new text block",
            }),
        }
    }

    /// Puts `object` on top of the board as one undo step and makes it the current object. A
    /// refusal becomes the message.
    fn insert_object(&mut self, object: Object) {
        let id = object.id.clone();
        let edit = Edit::Insert {
            index: self.board.len(),
            object,
        };
        if let Err(error) = edit.apply(&mut self.board) {
            return self.error(refusal(&error));
        }
        let before = std::mem::replace(&mut self.cursor, Cursor::Object(id));
        self.commit(vec![edit], before);
    }
}

/// A shrink factor as [`Engine::export_done`] shows it: two significant digits, rounded down so
/// that it never reads as 1 (0.995 is `0.99`), with no exponent (`0.00016`) and no trailing
/// zeros (`0.5`). `None` outside [`MIN_SHOWN_SHRINK`] to 1, 1 left out, and for a factor that
/// is not finite.
fn shown_factor(factor: f64) -> Option<String> {
    if !(MIN_SHOWN_SHRINK..1.0).contains(&factor) {
        return None;
    }
    // Twelve decimals are six significant digits or more down to `MIN_SHOWN_SHRINK`, and round
    // away binary noise (0.29 is 0.28999999999999998 in binary). A factor within that rounding
    // of 1 is still at least 0.99.
    let decimals = format!("{factor:.12}");
    let Some(digits) = decimals.strip_prefix("0.") else {
        return Some("0.99".to_owned());
    };
    let zeros = digits.len() - digits.trim_start_matches('0').len();
    let kept = digits
        .get(..zeros + 2)
        .unwrap_or(digits)
        .trim_end_matches('0');
    Some(format!("0.{kept}"))
}

/// `a` and `b` name the same file as far as their text tells: the same parts once `.` parts
/// are dropped and each `..` takes back the part before it. The file system is not asked, so
/// links are not followed (`link/../a` is `a`). Parts are split as the platform splits paths:
/// on `/`, and on Windows on `\` too.
fn same_path(a: &str, b: &str) -> bool {
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
        BoardError::TextSizeOutOfRange(_) => size::range_message(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shrink_factors_show_two_digits_rounded_down() {
        for (factor, shown) in [
            (0.5, "0.5"),
            (0.25, "0.25"),
            (0.29, "0.29"),
            (0.1, "0.1"),
            (0.96, "0.96"),
            (0.995, "0.99"),
            (0.999_999_999_999_9, "0.99"),
            (0.333_333, "0.33"),
            (0.000_164, "0.00016"),
            (0.000_16, "0.00016"),
            (0.000_006_25, "0.0000062"),
            (MIN_SHOWN_SHRINK, "0.000001"),
            (0.01, "0.01"),
        ] {
            assert_eq!(shown_factor(factor).as_deref(), Some(shown), "{factor}");
        }
        for factor in [
            1.0,
            1.5,
            0.0,
            -0.5,
            MIN_SHOWN_SHRINK / 2.0,
            f64::NAN,
            f64::INFINITY,
        ] {
            assert_eq!(shown_factor(factor), None, "{factor}");
        }
    }

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
            assert!(same_path(a, b), "{a} {b}");
            assert!(same_path(b, a), "{b} {a}");
        }
        for (a, b) in [
            ("a.lawh", "../a.lawh"),
            ("a.lawh", "dir/a.lawh"),
            ("a.lawh", "/a.lawh"),
            ("a.lawh", "a.lawh/.."),
            ("a.lawh", "A.lawh"),
        ] {
            assert!(!same_path(a, b), "{a} {b}");
        }
    }
}
