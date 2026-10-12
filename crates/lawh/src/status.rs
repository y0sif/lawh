//! The status line's text and the window title, from the engine's state.

use std::path::Path;

use lawh_core::engine::round3;
use lawh_core::{Cursor, Engine, Mode};

/// What the status line shows for unnamed boards, as Vim does.
const NO_NAME: &str = "[No Name]";

/// Marks a board with unwritten changes, after its name.
const MODIFIED: &str = " [+]";

/// Space between the groups of the status line's right side.
const GAP: &str = "   ";

/// The status line's text (see [`status`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub left: String,
    pub right: String,
    /// `left` is an error, drawn in the error colour.
    pub error: bool,
}

/// The status line for `engine`, with `app_message` from the app (a lawhrc warning, a refused
/// close) when there is one.
///
/// Left: the app's message if there is one (drawn as an error), else the engine's message, else
/// the mode as Vim shows it: `-- INSERT --`, `-- VISUAL --`, `-- TEXT --` in a text
/// block's Normal mode, and nothing in Normal mode. The command line, while open, is drawn in
/// place of the left side by the renderer.
///
/// Right: the keys typed so far of a command (when there are any), the board's file name (or
/// `[No Name]`) with ` [+]` when it has unwritten changes, and the cursor point as `x,y`
/// followed by the current object's id when the cursor is on one; three spaces between them:
/// `2d   area.lawh [+]   3,-2 c1`.
///
/// The app drops its message on every key, click and timeout, the only times the engine sets
/// one, so an app message is always the newer of the two: a close refused right after a key
/// that left an engine message still says so.
pub fn status(engine: &Engine, app_message: Option<&str>) -> Status {
    let (left, error) = if let Some(message) = app_message {
        (message.to_owned(), true)
    } else if let Some(message) = engine.message() {
        (message.text.clone(), message.error)
    } else {
        (mode_label(engine.mode()).to_owned(), false)
    };

    let mut right = engine.pending();
    if !right.is_empty() {
        right.push_str(GAP);
    }
    right.push_str(engine.file_name().unwrap_or(NO_NAME));
    if engine.is_modified() {
        right.push_str(MODIFIED);
    }
    right.push_str(GAP);
    let at = engine.cursor_point();
    let mut place = format!("{},{}", coordinate(at.x), coordinate(at.y));
    if let Cursor::Object(id) = engine.cursor() {
        place.push(' ');
        place.push_str(id.as_str());
    }
    right.push_str(&place);
    Status { left, right, error }
}

/// The window title: the file's name (without its directory) or `[No Name]`, ` [+]` when the
/// board has unwritten changes, then ` - lawh`.
pub fn title(engine: &Engine) -> String {
    let name = engine.file_name().map_or(NO_NAME.into(), |name| {
        Path::new(name)
            .file_name()
            .map_or_else(|| name.into(), |base| base.to_string_lossy())
    });
    let modified = if engine.is_modified() { MODIFIED } else { "" };
    format!("{name}{modified} - lawh")
}

fn mode_label(mode: Mode) -> &'static str {
    match mode {
        Mode::Normal | Mode::CommandLine => "",
        Mode::Insert => "-- INSERT --",
        Mode::Visual => "-- VISUAL --",
        Mode::BlockNormal => "-- TEXT --",
    }
}

/// A coordinate as the status line shows it: to 0.001 unit, the engine's precision, with no
/// trailing zeros and no `-0`.
fn coordinate(v: f64) -> String {
    // Adding 0.0 turns -0 into 0.
    format!("{}", round3(v) + 0.0)
}

#[cfg(test)]
mod tests {
    use lawh_core::{Board, FixedMeasure, Point, from_lawh, parse_keys};

    use super::*;

    fn engine(src: &str, name: Option<&str>) -> Engine {
        Engine::new(from_lawh(src).unwrap()).with_file_name(name.map(str::to_owned))
    }

    fn feed(engine: &mut Engine, keys: &str) {
        for key in parse_keys(keys).unwrap() {
            engine.feed(key, &mut FixedMeasure);
        }
    }

    fn line(left: &str, right: &str, error: bool) -> Status {
        Status {
            left: left.into(),
            right: right.into(),
            error,
        }
    }

    const BOARD: &str = "lawh version=1\ncircle id=c1 x=3 y=-2 r=1\ntext id=t1 x=0 y=4 \"hi\"\n";

    #[test]
    fn normal_mode_shows_the_file_and_the_cursor() {
        let mut e = engine(BOARD, Some("boards/area.lawh"));
        assert_eq!(status(&e, None), line("", "boards/area.lawh   0,0", false));
        feed(&mut e, "3l2k");
        assert_eq!(status(&e, None), line("", "boards/area.lawh   3,-2", false));
        let c1 = lawh_core::ObjectId::new("c1").unwrap();
        e.click(Point::new(3.2, -2.0), Some(c1), &mut FixedMeasure);
        assert_eq!(
            status(&e, None),
            line("", "boards/area.lawh   3,-2 c1", false)
        );
    }

    #[test]
    fn pending_keys_come_first() {
        let mut e = engine(BOARD, Some("a.lawh"));
        feed(&mut e, "2d3");
        assert_eq!(status(&e, None), line("", "2d3   a.lawh   0,0", false));
    }

    #[test]
    fn the_keys_that_showed_jump_labels_wait_with_a_first_label_character() {
        let src = "lawh version=1\n\
                   circle id=c1 x=1 y=0 r=1\n\
                   circle id=c2 x=4 y=0 r=1\n\
                   circle id=c3 x=7 y=0 r=1\n";
        let mut e = engine(src, Some("a.lawh"));
        // Three objects and two characters: `a` on the nearest, then `ba` and `bb`.
        assert_eq!(e.source("set jumplabels=ab\n"), []);
        feed(&mut e, "s");
        assert_eq!(status(&e, None), line("", "s   a.lawh   0,0", false));
        feed(&mut e, "<Esc>2ds");
        assert_eq!(status(&e, None), line("", "ds   a.lawh   0,0", false));
        feed(&mut e, "b");
        assert_eq!(status(&e, None), line("", "dsb   a.lawh   0,0", false));
        feed(&mut e, "b");
        assert_eq!(status(&e, None), line("", "a.lawh [+]   0,0", false));
        assert_eq!(e.board().len(), 2);
    }

    #[test]
    fn modes_are_labelled_as_in_vim() {
        let mut e = engine(BOARD, None);
        feed(&mut e, "v");
        assert_eq!(status(&e, None).left, "-- VISUAL --");
        feed(&mut e, "<Esc>i");
        assert_eq!(status(&e, None).left, "-- INSERT --");
        feed(&mut e, "x<Esc>");
        assert_eq!(status(&e, None).left, "");
        feed(&mut e, "<CR>");
        assert_eq!(status(&e, None).left, "-- TEXT --");
        feed(&mut e, ":");
        assert_eq!(e.command_line(), Some(("", 0)));
        assert_eq!(status(&e, None).left, "");
    }

    #[test]
    fn an_unnamed_modified_board() {
        let mut e = Engine::new(Board::new());
        feed(&mut e, ":ci 1<CR>");
        assert_eq!(status(&e, None), line("", "[No Name] [+]   0,0 c1", false));
        assert_eq!(title(&e), "[No Name] [+] - lawh");
    }

    #[test]
    fn messages_come_before_the_mode() {
        let mut e = engine(BOARD, Some("a.lawh"));
        let app = Some("lawhrc line 2: nope");
        assert_eq!(
            status(&e, app),
            line("lawhrc line 2: nope", "a.lawh   0,0", true)
        );
        feed(&mut e, "v:nope<CR>");
        let shown = status(&e, None);
        assert!(shown.left.contains("nope"), "{shown:?}");
        assert!(shown.error, "{shown:?}");
        assert_eq!(
            status(&e, app).left,
            "lawhrc line 2: nope",
            "the app's is newer"
        );
        feed(&mut e, ":w<CR>");
        e.write_done("a.lawh", Ok(()), false);
        assert_eq!(
            status(&e, None),
            line("\"a.lawh\" written", "a.lawh   0,0", false)
        );
    }

    #[test]
    fn coordinates_are_short() {
        assert_eq!(coordinate(0.0), "0");
        assert_eq!(coordinate(-0.0), "0");
        assert_eq!(coordinate(-0.0001), "0");
        assert_eq!(coordinate(0.1 + 0.2), "0.3");
        assert_eq!(coordinate(-2.5), "-2.5");
        assert_eq!(coordinate(1.234_567), "1.235");
        assert_eq!(coordinate(1_000_000.0), "1000000");
    }

    #[test]
    fn the_title_is_the_file_name() {
        let mut e = engine(BOARD, Some("boards/area.lawh"));
        assert_eq!(title(&e), "area.lawh - lawh");
        feed(&mut e, "dd");
        assert_eq!(title(&e), "area.lawh - lawh", "no object under the cursor");
        feed(&mut e, "wdd");
        assert_eq!(title(&e), "area.lawh [+] - lawh");
        assert_eq!(title(&engine(BOARD, Some(".."))), ".. - lawh");
        assert_eq!(title(&engine(BOARD, None)), "[No Name] - lawh");
    }
}
