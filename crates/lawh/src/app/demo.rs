//! The README demo, made of the app's own frames: a key script typed into an [`App`] with no
//! window, each frame drawn by [`compose_frame`] as the window draws it (board, overlay, status
//! line) and written as a PNG, with a timeline for ffmpeg, `demo.json` for a video editor and
//! the on-screen key display's text. `demo/render.sh` runs [`render_demo`] and frames the
//! result; the script's format is in `demo/README.md`.
//!
//! Deterministic: the output depends on the script alone. Time is the script's, not the clock's;
//! math compiles in this process (the demo's own board, so trusted) and is resolved before each
//! frame, so no formula shows as a placeholder; the view is fixed; no lawhrc is read.

use std::fmt::Write as _;
use std::fs;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::time::Instant;

use lawh_core::{Board, Engine, Key, KeyCode, Mods, Point, parse_keys};
use lawh_render::{Renderer, Viewport};

use super::{App, compose_frame, frame_view};

/// Time between keys until a `pace` line sets another, in milliseconds.
const DEFAULT_PACE_MS: u64 = 80;

/// Most draws of one frame: a frame's work budget may leave text out (the first frame at a
/// size has a smaller budget, as in the window), and the next draw adds it.
const MAX_DRAWS: usize = 4;

/// Most characters the key display shows; older keys scroll out to the left behind `…`.
const KEY_LINE_CHARS: usize = 44;

/// What the key display puts between the keys of one `keys` line and the next.
const KEY_GROUP_GAP: &str = "   ";

/// Largest scale factor a script may ask for.
const MAX_SCALE: u16 = 4;

/// A demo script: the window, the view and the board's file name, then what happens.
#[derive(Debug, Clone, PartialEq)]
struct Script {
    /// The window's size in logical pixels.
    width: u16,
    height: u16,
    /// The scale factor: physical pixels a logical pixel.
    scale: u16,
    /// The fixed view: its centre in grid units, and its zoom.
    center: Point,
    zoom: f64,
    /// The board's file name, as the status line shows it. Exports land in the output
    /// directory.
    file: String,
    steps: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq)]
enum Step {
    /// A captioned section starts.
    Scene(String),
    /// Milliseconds between keys from here on.
    Pace(u64),
    /// Milliseconds the current frame holds.
    Wait(u64),
    /// Keys typed one by one at the pace.
    Keys(Vec<Key>),
}

/// Reads a script (see `demo/README.md`). Errors name the line.
fn parse_script(src: &str) -> Result<Script, String> {
    let mut window = None;
    let mut view = None;
    let mut file = None;
    let mut steps = Vec::new();
    for (i, line) in src.lines().enumerate() {
        let n = i + 1;
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let (directive, rest) = line.split_once(' ').unwrap_or((line, ""));
        let numbers = || -> Result<Vec<f64>, String> {
            rest.split_whitespace()
                .map(|word| {
                    word.parse::<f64>()
                        .ok()
                        .filter(|v| v.is_finite())
                        .ok_or_else(|| format!("line {n}: `{word}` is not a number"))
                })
                .collect()
        };
        let ms = || -> Result<u64, String> {
            rest.trim()
                .parse::<u64>()
                .ok()
                .filter(|&ms| ms > 0)
                .ok_or_else(|| format!("line {n}: `{directive}` takes milliseconds above 0"))
        };
        match directive {
            "window" => {
                let words: Vec<&str> = rest.split_whitespace().collect();
                let parsed: Option<Vec<u16>> = words.iter().map(|w| w.parse().ok()).collect();
                match parsed.as_deref() {
                    Some(&[w, h, s]) if w > 0 && h > 0 && (1..=MAX_SCALE).contains(&s) => {
                        window = Some((w, h, s));
                    }
                    _ => {
                        return Err(format!(
                            "line {n}: `window` takes a width and a height in logical pixels \
                             and a whole scale factor from 1 to {MAX_SCALE}"
                        ));
                    }
                }
            }
            "view" => match numbers()?.as_slice() {
                &[x, y, zoom] if zoom > 0.0 => view = Some((Point::new(x, y), zoom)),
                _ => {
                    return Err(format!(
                        "line {n}: `view` takes the centre's x and y in grid units and a zoom \
                         above 0"
                    ));
                }
            },
            "file" if !rest.trim().is_empty() => file = Some(rest.trim().to_owned()),
            "scene" => steps.push(Step::Scene(rest.trim().to_owned())),
            "pace" => steps.push(Step::Pace(ms()?)),
            "wait" => steps.push(Step::Wait(ms()?)),
            // Everything after `keys ` is typed, spaces included.
            "keys" => steps.push(Step::Keys(
                parse_keys(rest).map_err(|err| format!("line {n}: {err}"))?,
            )),
            _ => return Err(format!("line {n}: `{line}` is no directive")),
        }
    }
    let (width, height, scale) = window.ok_or("missing `window <width> <height> <scale>`")?;
    let (center, zoom) = view.ok_or("missing `view <x> <y> <zoom>`")?;
    let file = file.ok_or("missing `file <name>`")?;
    if width.checked_mul(scale).is_none() || height.checked_mul(scale).is_none() {
        return Err("the window is too large".to_owned());
    }
    Ok(Script {
        width,
        height,
        scale,
        center,
        zoom,
        file,
        steps,
    })
}

/// A frame written: its file, relative to the output directory, and when it starts showing.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Frame {
    file: String,
    start_ms: u64,
}

/// A key typed, and when.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Typed {
    at_ms: u64,
    key: Key,
}

/// A scene, from its start to the next one's or the end.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Scene {
    caption: String,
    start_ms: u64,
    end_ms: u64,
}

/// What the key display shows from `start_ms` to `end_ms`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct KeyLine {
    start_ms: u64,
    end_ms: u64,
    text: String,
}

/// What a script made: the frames written, and what the outputs other than frames are made of.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Recording {
    /// The frames' size in physical pixels.
    width: u32,
    height: u32,
    frames: Vec<Frame>,
    keys: Vec<Typed>,
    scenes: Vec<Scene>,
    key_lines: Vec<KeyLine>,
    duration_ms: u64,
}

/// Types `script` into a new app on an empty board and writes each distinct frame into
/// `out/frames` (emptied first). `:w` and `:exp` paths are taken from `out`.
fn record(script: &Script, out: &Path) -> Recording {
    let frames_dir = out.join("frames");
    if frames_dir.exists() {
        fs::remove_dir_all(&frames_dir).unwrap();
    }
    fs::create_dir_all(&frames_dir).unwrap();

    let mut engine = Engine::new(Board::new()).with_file_name(Some(script.file.clone()));
    engine.set_symbols(crate::drawable_symbols());
    let mut app = App::new(
        engine,
        Some(PathBuf::from(&script.file)),
        Some(out.to_owned()),
        Renderer::new_trusted(),
        Instant::now(),
        None,
    );
    let size = (
        script.width * script.scale,
        script.height * script.scale,
        f64::from(script.scale),
    );
    let view = Viewport::new(script.center, script.zoom, size.0, size.1, size.2);
    app.viewport = Some(view);
    app.keep_view = true;

    let trace = std::env::var_os("LAWH_DEMO_TRACE").is_some();
    let mut frames = Frames::new(frames_dir, size);
    let mut keys = Vec::new();
    let mut scenes: Vec<Scene> = Vec::new();
    let mut line = KeyLines::default();
    let mut now = 0;
    let mut pace = DEFAULT_PACE_MS;
    frames.add(&mut app, now);
    for step in &script.steps {
        match step {
            Step::Scene(caption) => {
                if let Some(last) = scenes.last_mut() {
                    last.end_ms = now;
                }
                scenes.push(Scene {
                    caption: caption.clone(),
                    start_ms: now,
                    end_ms: now,
                });
                line.clear(now);
            }
            Step::Pace(ms) => pace = *ms,
            Step::Wait(ms) => now += ms,
            Step::Keys(typed) => {
                line.group();
                for &key in typed {
                    keys.push(Typed { at_ms: now, key });
                    line.push(key, now);
                    app.feed(vec![key]);
                    assert!(!app.quitting, "the demo quit at {key}");
                    assert_eq!(
                        app.timer.deadline, None,
                        "{key} waits for a mapping's timeout, which the demo doesn't run"
                    );
                    assert_eq!(
                        app.viewport,
                        Some(view),
                        "the view followed the cursor after {key}: give the script a view \
                         that holds everything"
                    );
                    if trace {
                        trace_key(&app, key, now);
                    }
                    frames.add(&mut app, now);
                    // An export shows `exporting "file"` for a key's time, then runs.
                    if app.suspended.is_some() {
                        now += pace;
                        assert!(app.finish_exports());
                        frames.add(&mut app, now);
                    }
                    now += pace;
                }
            }
        }
    }
    if let Some(last) = scenes.last_mut() {
        last.end_ms = now;
    }
    line.clear(now);
    let last = frames.written.last().map_or(0, |frame| frame.start_ms);
    assert!(
        now > last,
        "the script must end with a `wait`, so its last frame shows"
    );
    Recording {
        width: u32::from(size.0),
        height: u32::from(size.1),
        frames: frames.written,
        keys,
        scenes,
        key_lines: line.lines,
        duration_ms: now,
    }
}

/// Prints what a key left that a script's author needs: the status line, jump labels and
/// `:sym` rows.
fn trace_key(app: &App, key: Key, now: u64) {
    let status = super::status_line(&app.engine, app.message.as_deref(), app.suspended.as_ref());
    eprintln!(
        "{now:>6} ms {key:<8} {:?} | {} | {}",
        app.engine.cursor(),
        status.left,
        status.right
    );
    if let Some((labels, typed)) = app.engine.jump_labels() {
        let labels: Vec<String> = labels
            .iter()
            .map(|l| format!("{}={}", l.text, l.id.as_str()))
            .collect();
        eprintln!("          labels (typed {typed:?}): {}", labels.join(" "));
    }
    if let Some((rows, selected)) = app.engine.completions() {
        let rows: Vec<String> = rows
            .iter()
            .map(|s| format!("{} {}", s.name, s.value))
            .collect();
        eprintln!(
            "          rows (selected {selected:?}): {}",
            rows.join(", ")
        );
    }
}

/// The frames written so far.
struct Frames {
    dir: PathBuf,
    size: (u16, u16, f64),
    /// The last frame's pixels, so a key that changes nothing on screen writes no frame.
    last: Vec<u8>,
    written: Vec<Frame>,
}

impl Frames {
    fn new(dir: PathBuf, size: (u16, u16, f64)) -> Self {
        Self {
            dir,
            size,
            last: Vec::new(),
            written: Vec::new(),
        }
    }

    /// Draws the app's frame and writes it, shown from `now`, unless it is the frame showing.
    /// A frame written at `now` already is replaced: it never showed.
    fn add(&mut self, app: &mut App, now: u64) {
        let pixels = draw(app, self.size);
        if pixels == self.last {
            return;
        }
        if self.written.last().is_some_and(|f| f.start_ms == now) {
            self.written.pop();
        }
        let file = format!("frames/frame-{:05}.png", self.written.len() + 1);
        let (width, height, _) = self.size;
        write_png(
            &self.dir.join(Path::new(&file).file_name().unwrap()),
            &pixels,
            u32::from(width),
            u32::from(height),
        );
        self.written.push(Frame {
            file,
            start_ms: now,
        });
        self.last = pixels;
    }
}

/// The app's frame of `size` (physical width, height and scale factor), drawn by the window's
/// own code ([`frame_view`], [`compose_frame`]) with every formula compiled and all text laid
/// out: premultiplied RGBA8.
fn draw(app: &mut App, size: (u16, u16, f64)) -> Vec<u8> {
    app.renderer.resolve_math(app.engine.board());
    for _ in 0..MAX_DRAWS {
        let vp = frame_view(&mut app.viewport, &app.engine, &mut app.renderer, size);
        let pixels = compose_frame(
            &app.engine,
            &mut app.renderer,
            app.message.as_deref(),
            app.suspended.as_ref(),
            &vp,
        )
        .to_vec();
        let drawn = app.renderer.last_frame();
        if !drawn.incomplete() {
            assert_eq!(
                (drawn.math_pending, drawn.math_evicted, drawn.downgraded),
                (0, 0, 0),
                "{drawn:?}"
            );
            return pixels;
        }
    }
    panic!("a frame still left text out after {MAX_DRAWS} draws");
}

/// Writes a frame's premultiplied RGBA8 pixels as an RGB PNG, alpha dropped as the window
/// drops it ([`super::rgba_to_0rgb`]): the paper is opaque.
fn write_png(path: &Path, rgba: &[u8], width: u32, height: u32) {
    let (pixels, _) = rgba.as_chunks::<4>();
    let rgb: Vec<u8> = pixels.iter().flat_map(|px| [px[0], px[1], px[2]]).collect();
    let file = BufWriter::new(fs::File::create(path).unwrap());
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&rgb).unwrap();
    writer.finish().unwrap();
}

/// How a key is shown on screen, and whether it is a key with no character of its own (or a
/// chord), shown by name: `Esc`, `Enter`, `Tab`, `Ctrl+K`. A character is itself.
fn display(key: Key) -> (String, bool) {
    let name = match key.code {
        KeyCode::Char(c) if key.mods.is_empty() => return (c.to_string(), false),
        KeyCode::Char(c) => c.to_uppercase().to_string(),
        KeyCode::Esc => "Esc".to_owned(),
        KeyCode::Enter => "Enter".to_owned(),
        KeyCode::Tab => "Tab".to_owned(),
        KeyCode::Backspace => "Backspace".to_owned(),
        KeyCode::Delete => "Del".to_owned(),
        KeyCode::Up => "Up".to_owned(),
        KeyCode::Down => "Down".to_owned(),
        KeyCode::Left => "Left".to_owned(),
        KeyCode::Right => "Right".to_owned(),
        KeyCode::Home => "Home".to_owned(),
        KeyCode::End => "End".to_owned(),
        KeyCode::PageUp => "PgUp".to_owned(),
        KeyCode::PageDown => "PgDn".to_owned(),
        KeyCode::Insert => "Ins".to_owned(),
        KeyCode::F(n) => format!("F{n}"),
    };
    let Mods {
        ctrl,
        alt,
        shift,
        cmd,
    } = key.mods;
    let mut shown = String::new();
    for (held, prefix) in [
        (ctrl, "Ctrl+"),
        (alt, "Alt+"),
        (shift, "Shift+"),
        (cmd, "Cmd+"),
    ] {
        if held {
            shown.push_str(prefix);
        }
    }
    shown.push_str(&name);
    (shown, true)
}

/// The key display: the keys typed since the scene started, a named key set apart by a space
/// and each `keys` line's keys by [`KEY_GROUP_GAP`], which takes the place of a space typed
/// first on the line.
#[derive(Debug, Default)]
struct KeyLines {
    typed: String,
    since: u64,
    /// The next key starts a group.
    group: bool,
    lines: Vec<KeyLine>,
}

impl KeyLines {
    /// The next key starts a new group: a `keys` line.
    fn group(&mut self) {
        self.group = true;
    }

    fn push(&mut self, key: Key, now: u64) {
        self.end(now);
        self.since = now;
        let (shown, named) = display(key);
        if std::mem::take(&mut self.group) && !self.typed.is_empty() {
            self.typed.truncate(self.typed.trim_end().len());
            self.typed.push_str(KEY_GROUP_GAP);
            if shown == " " {
                return;
            }
        }
        if named {
            if !self.typed.is_empty() && !self.typed.ends_with(' ') {
                self.typed.push(' ');
            }
            self.typed.push_str(&shown);
            self.typed.push(' ');
        } else {
            self.typed.push_str(&shown);
        }
    }

    fn clear(&mut self, now: u64) {
        self.end(now);
        self.typed.clear();
        self.since = now;
    }

    /// What showed since the last change ends at `now`; it goes on what showed before it when
    /// the two read the same.
    fn end(&mut self, now: u64) {
        let text = self.typed.trim_end();
        if now > self.since && !text.is_empty() {
            let count = text.chars().count();
            let text = if count > KEY_LINE_CHARS {
                let tail: String = text.chars().skip(count + 1 - KEY_LINE_CHARS).collect();
                format!("\u{2026}{}", tail.trim_start())
            } else {
                text.to_owned()
            };
            if let Some(last) = self.lines.last_mut()
                && last.end_ms == self.since
                && last.text == text
            {
                last.end_ms = now;
                return;
            }
            self.lines.push(KeyLine {
                start_ms: self.since,
                end_ms: now,
                text,
            });
        }
    }
}

/// `s` as a JSON string.
fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if u32::from(c) < 0x20 => write!(out, "\\u{:04x}", u32::from(c)).unwrap(),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Seconds for ffmpeg, from milliseconds.
fn seconds(ms: u64) -> String {
    format!("{}.{:03}", ms / 1000, ms % 1000)
}

/// The timeline for ffmpeg's concat demuxer: each frame and how long it shows. Each is read at
/// 1,000 frames a second (`option framerate`), so its time keeps its milliseconds; the image
/// reader's default, 25, would round it to 40 ms. The last frame is named again so that its
/// duration counts; `demo/render.sh` cuts the video at `duration_ms` (`demo.json`).
fn timeline(rec: &Recording) -> String {
    fn entry(out: &mut String, file: &str) {
        writeln!(out, "file '{file}'\noption framerate 1000").unwrap();
    }
    let mut out = String::from("ffconcat version 1.0\n");
    for (i, frame) in rec.frames.iter().enumerate() {
        let end = rec
            .frames
            .get(i + 1)
            .map_or(rec.duration_ms, |next| next.start_ms);
        entry(&mut out, &frame.file);
        writeln!(out, "duration {}", seconds(end - frame.start_ms)).unwrap();
    }
    if let Some(last) = rec.frames.last() {
        entry(&mut out, &last.file);
    }
    out
}

/// `demo.json`: everything a video editor needs to time captions and keys to the frames.
fn demo_json(script: &Script, rec: &Recording) -> String {
    let mut out = String::from("{\n  \"version\": 1,\n");
    writeln!(out, "  \"file\": {},", json_string(&script.file)).unwrap();
    writeln!(
        out,
        "  \"width\": {},\n  \"height\": {},\n  \"logical_width\": {},\n  \
         \"logical_height\": {},\n  \"scale\": {},\n  \"duration_ms\": {},",
        rec.width, rec.height, script.width, script.height, script.scale, rec.duration_ms
    )
    .unwrap();
    out.push_str("  \"frames\": [\n");
    let frames: Vec<String> = rec
        .frames
        .iter()
        .enumerate()
        .map(|(i, frame)| {
            let end = rec
                .frames
                .get(i + 1)
                .map_or(rec.duration_ms, |next| next.start_ms);
            format!(
                "    {{\"file\": {}, \"start_ms\": {}, \"duration_ms\": {}}}",
                json_string(&frame.file),
                frame.start_ms,
                end - frame.start_ms
            )
        })
        .collect();
    out.push_str(&frames.join(",\n"));
    out.push_str("\n  ],\n  \"keys\": [\n");
    let keys: Vec<String> = rec
        .keys
        .iter()
        .map(|typed| {
            let (shown, named) = display(typed.key);
            format!(
                "    {{\"at_ms\": {}, \"key\": {}, \"display\": {}, \"named\": {named}}}",
                typed.at_ms,
                json_string(&typed.key.to_string()),
                json_string(&shown)
            )
        })
        .collect();
    out.push_str(&keys.join(",\n"));
    out.push_str("\n  ],\n  \"scenes\": [\n");
    let scenes: Vec<String> = rec
        .scenes
        .iter()
        .map(|scene| {
            format!(
                "    {{\"caption\": {}, \"start_ms\": {}, \"end_ms\": {}}}",
                json_string(&scene.caption),
                scene.start_ms,
                scene.end_ms
            )
        })
        .collect();
    out.push_str(&scenes.join(",\n"));
    out.push_str("\n  ]\n}\n");
    out
}

/// The key display for ffmpeg: one line a state, `start_ms`, `end_ms` and the text, tab
/// separated. The text holds no tab or line break (named keys are shown by name).
fn key_lines_tsv(rec: &Recording) -> String {
    let mut out = String::new();
    for line in &rec.key_lines {
        assert!(!line.text.contains(['\t', '\n', '\r']), "{line:?}");
        writeln!(out, "{}\t{}\t{}", line.start_ms, line.end_ms, line.text).unwrap();
    }
    out
}

/// Renders `script` into `out`: `frames/`, `timeline.ffconcat`, `demo.json` and `keys.tsv`.
fn render(script: &Script, out: &Path) -> Recording {
    fs::create_dir_all(out).unwrap();
    let rec = record(script, out);
    fs::write(out.join("timeline.ffconcat"), timeline(&rec)).unwrap();
    fs::write(out.join("demo.json"), demo_json(script, &rec)).unwrap();
    fs::write(out.join("keys.tsv"), key_lines_tsv(&rec)).unwrap();
    rec
}

/// The workspace's root: relative paths in the environment are taken from it, not from this
/// crate's directory, where `cargo test` runs.
fn workspace_path(path: &str) -> PathBuf {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    crate_dir.ancestors().nth(2).unwrap_or(crate_dir).join(path)
}

/// Renders the demo script `LAWH_DEMO_SCRIPT` into the directory `LAWH_DEMO_OUT`.
/// `LAWH_DEMO_TRACE=1` prints each key's status line, labels and `:sym` rows. Run by
/// `demo/render.sh`.
#[test]
#[ignore = "renders the README demo: run demo/render.sh"]
fn render_demo() {
    let var = |name: &str| {
        std::env::var(name).unwrap_or_else(|_| panic!("set {name}; demo/render.sh does"))
    };
    let script_path = workspace_path(&var("LAWH_DEMO_SCRIPT"));
    let out = workspace_path(&var("LAWH_DEMO_OUT"));
    let src = fs::read_to_string(&script_path)
        .unwrap_or_else(|err| panic!("{}: {err}", script_path.display()));
    let script =
        parse_script(&src).unwrap_or_else(|err| panic!("{}: {err}", script_path.display()));
    let rec = render(&script, &out);
    eprintln!(
        "{} frames of {}x{}, {} keys, {} ms, in {}",
        rec.frames.len(),
        rec.width,
        rec.height,
        rec.keys.len(),
        rec.duration_ms,
        out.display()
    );
}

mod tests {
    use super::*;
    use crate::test_dir::TestDir;

    #[test]
    fn the_readme_demo_script_reads() {
        let src = fs::read_to_string(workspace_path("demo/area.demo")).unwrap();
        let script = parse_script(&src).unwrap();
        assert_eq!(script.file, "area.lawh");
        assert!(script.steps.iter().any(|s| matches!(s, Step::Keys(_))));
        assert!(matches!(script.steps.last(), Some(Step::Wait(_))));
    }

    #[test]
    fn scripts_say_what_is_wrong_and_where() {
        let head = "window 100 80 1\nview 0 0 1\nfile a.lawh\n";
        let err = |body: &str| parse_script(&format!("{head}{body}")).unwrap_err();
        assert_eq!(err("wait 0\n"), "line 4: `wait` takes milliseconds above 0");
        assert_eq!(err("pace x\n"), "line 4: `pace` takes milliseconds above 0");
        assert_eq!(err("jump\n"), "line 4: `jump` is no directive");
        assert!(err("keys <C-\n").starts_with("line 4: "));
        assert!(parse_script("view 0 0 1\nfile a\n").is_err());
        assert!(parse_script("window 100 80 5\nview 0 0 1\nfile a\n").is_err());
        let script = parse_script(&format!("{head}# a comment\n\nkeys :ci 2<CR>\n")).unwrap();
        assert_eq!(script.steps, [Step::Keys(parse_keys(":ci 2<CR>").unwrap())]);
    }

    #[test]
    fn keys_show_as_typed_or_by_name() {
        let shown = |notation: &str| display(parse_keys(notation).unwrap()[0]);
        assert_eq!(shown("a"), ("a".to_owned(), false));
        assert_eq!(shown("<Space>"), (" ".to_owned(), false));
        assert_eq!(shown("<CR>"), ("Enter".to_owned(), true));
        assert_eq!(shown("<Esc>"), ("Esc".to_owned(), true));
        assert_eq!(shown("<S-Tab>"), ("Shift+Tab".to_owned(), true));
        assert_eq!(shown("<C-k>"), ("Ctrl+K".to_owned(), true));
        assert_eq!(json_string("a\"b\\c\u{1}"), r#""a\"b\\c\u0001""#);
    }

    #[test]
    fn the_key_display_sets_named_keys_and_key_lines_apart_and_clears_with_a_scene() {
        let mut lines = KeyLines::default();
        lines.group();
        for (at, key) in (10..).step_by(10).zip(parse_keys("ab<Esc>").unwrap()) {
            lines.push(key, at);
        }
        lines.group();
        lines.push(parse_keys(" ").unwrap()[0], 40);
        lines.push(parse_keys("c").unwrap()[0], 50);
        lines.clear(100);
        lines.push(parse_keys("x").unwrap()[0], 150);
        lines.clear(200);
        let shown: Vec<_> = lines
            .lines
            .iter()
            .map(|l| (l.start_ms, l.end_ms, l.text.as_str()))
            .collect();
        assert_eq!(
            shown,
            [
                (10, 20, "a"),
                (20, 30, "ab"),
                (30, 50, "ab Esc"),
                (50, 100, "ab Esc   c"),
                (150, 200, "x"),
            ]
        );
    }

    /// A small script, drawn twice: the same frames, timeline and `demo.json` each time.
    #[test]
    fn a_script_renders_the_same_frames_each_time() {
        let src = "window 320 240 1\nview 0 0 1\nfile t.lawh\n\
                   wait 100\nscene Text\npace 50\nkeys iab<Esc>\nwait 200\nkeys :ci 1<CR>\nwait 300\n";
        let script = parse_script(src).unwrap();
        let dir = TestDir::new("demo");
        let outputs = |name: &str| {
            let out = dir.path(name);
            let rec = render(&script, &out);
            let mut files = Vec::new();
            for frame in &rec.frames {
                files.push(fs::read(out.join(&frame.file)).unwrap());
            }
            for name in ["timeline.ffconcat", "demo.json", "keys.tsv"] {
                files.push(fs::read(out.join(name)).unwrap());
            }
            (rec, files)
        };
        let (rec, first) = outputs("one");
        let (again, second) = outputs("two");
        assert_eq!(rec, again);
        assert_eq!(first, second);
        // The empty board, `i`, each of `a` and `b`, `<Esc>`, then `:`, `c`, `i`, ` `, `1` and the
        // circle on `<CR>`.
        assert_eq!(rec.frames.len(), 11, "{:?}", rec.frames);
        assert_eq!(rec.frames[0].start_ms, 0);
        assert_eq!(rec.frames[1].start_ms, 100);
        assert_eq!(rec.duration_ms, 100 + 4 * 50 + 200 + 6 * 50 + 300);
        assert_eq!((rec.width, rec.height), (320, 240));
        let timeline = String::from_utf8(first[first.len() - 3].clone()).unwrap();
        assert!(timeline.starts_with(
            "ffconcat version 1.0\nfile 'frames/frame-00001.png'\noption framerate 1000\n\
             duration 0.100\n"
        ));
        assert!(
            timeline.ends_with(
                "duration 0.350\nfile 'frames/frame-00011.png'\noption framerate 1000\n"
            ),
            "{timeline}"
        );
    }
}
