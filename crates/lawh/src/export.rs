//! Exports from the app: the file `:export` asks for in the window ([`export_to`]), and
//! `lawh export` ([`run`]), which draws a board file into an SVG, PNG or PDF file without a
//! window.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::bail;
use lawh_core::{Board, Engine, ExportOptions, ExportReport, ObjectId};
use lawh_render::Renderer;

use crate::cli::ExportArgs;
use crate::files;

/// Why an export onto the board's own file is refused, after `cannot export "x.svg": `. An
/// exported file can't be opened as a board again, so writing it there would lose the board; a
/// board named `x.svg`, or a link of that name pointing at the board, would be overwritten
/// otherwise (the engine knows names, not files).
pub const OWN_FILE: &str = "that is the board's own file";

/// Draws `board`, or only the objects of `selection`, with `renderer` into a file of `options`'
/// format ([`Renderer::export`], which waits for every formula), then writes it to `target`
/// atomically ([`files::write_atomic`]): an existing file is replaced, a file that can't be
/// written is left as it was. Returns what the export reports, or why nothing was written (the
/// export's error or the write's), as the status line shows it after the file's name.
///
/// The renderer draws an export into an image of its own, so a window's next frame is drawn as
/// if no export had been.
pub fn export_to(
    renderer: &mut Renderer,
    board: &Board,
    selection: Option<&[ObjectId]>,
    options: ExportOptions,
    target: &Path,
) -> Result<ExportReport, String> {
    let started = Instant::now();
    let exported = renderer
        .export(board, selection, &options)
        .map_err(|err| err.to_string())?;
    let drawn = started.elapsed();
    files::write_atomic(target, &exported.bytes).map_err(|err| err.to_string())?;
    tracing::info!(
        path = %target.display(),
        format = options.format.extension(),
        bytes = exported.bytes.len(),
        failed_formulas = exported.report.failed_formulas,
        shrunk = ?exported.report.shrunk,
        export_ms = ms(drawn),
        ms = ms(started.elapsed()),
        "exported"
    );
    Ok(exported.report)
}

/// `lawh export`: reads the board in `args.board` (a missing file is an error, never a new
/// board; its size is checked first, as the window's open does), draws it with math compiled
/// in the worker process ([`crate::math_engine`]) and the bundled fonts, and writes
/// `args.out`. No window and no lawhrc: the file is the same on every machine.
///
/// Returns a line to print on stderr after a good export when there is something to know (see
/// [`note`]), or the error to exit with.
pub fn run(args: &ExportArgs) -> anyhow::Result<Option<String>> {
    let board = files::read_existing(&args.board)?;
    let mut renderer = Renderer::with_math(crate::math_engine()?);
    export_file(args, &board, &mut renderer)
}

/// The export of `board`, read from `args.board`, to `args.out` with `renderer`. Refused when
/// `args.out` is the board's own file ([`files::same_file`], so a link to the board too), before
/// anything is drawn.
fn export_file(
    args: &ExportArgs,
    board: &Board,
    renderer: &mut Renderer,
) -> anyhow::Result<Option<String>> {
    let name = args.out.display().to_string();
    if files::same_file(&args.board, &args.out) {
        bail!("cannot export \"{name}\": {OWN_FILE}");
    }
    match export_to(renderer, board, None, args.options, &args.out) {
        Ok(report) => Ok(note(&name, report)),
        Err(err) => bail!("cannot export \"{name}\": {err}"),
    }
}

/// What to say after a good export, in the window's words ([`Engine::export_done`]:
/// `"out.png" exported, 2 formulas failed, shrunk to 0.5x to fit`), when the export has more to
/// report than the file being written: formulas that failed (drawn as their source in red), or
/// a PNG or PDF shrunk to fit its caps. `None` for a clean export, which prints nothing.
fn note(name: &str, report: ExportReport) -> Option<String> {
    if report.failed_formulas == 0 && report.shrunk.is_none() {
        return None;
    }
    let mut engine = Engine::new(Board::new());
    engine.export_done(name, Ok(report));
    engine.message().map(|message| message.text.clone())
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use lawh_core::engine::Settings;
    use lawh_core::{ExportFormat, from_lawh};

    use super::*;
    use crate::test_dir::TestDir;

    const AREA: &str = include_str!("../../../examples/area.lawh");

    fn args(board: PathBuf, out: PathBuf) -> ExportArgs {
        let name = out.to_string_lossy().into_owned();
        let format = ExportFormat::from_path(&name).unwrap();
        ExportArgs {
            board,
            out,
            options: Settings::default().export_options(format),
        }
    }

    #[test]
    fn a_clean_export_says_nothing_and_a_failed_formula_is_named() {
        assert_eq!(note("out.png", ExportReport::default()), None);
        let failed = ExportReport {
            failed_formulas: 2,
            shrunk: None,
        };
        assert_eq!(
            note("out.png", failed).as_deref(),
            Some("\"out.png\" exported, 2 formulas failed")
        );
        let shrunk = ExportReport {
            failed_formulas: 1,
            shrunk: Some(0.5),
        };
        assert_eq!(
            note("a/out.pdf", shrunk).as_deref(),
            Some("\"a/out.pdf\" exported, 1 formula failed, shrunk to 0.5x to fit")
        );
    }

    #[test]
    fn a_board_exports_to_its_file_and_a_failing_formula_is_a_note() {
        let dir = TestDir::new("cli-export-file");
        let board_path = dir.write("area.lawh", AREA);
        let board = from_lawh(AREA).unwrap();
        let mut renderer = Renderer::new_trusted();
        let out = dir.path("area.svg");
        let note = export_file(
            &args(board_path.clone(), out.clone()),
            &board,
            &mut renderer,
        );
        assert_eq!(note.unwrap(), None);
        assert!(fs::read(&out).unwrap().starts_with(b"<?xml"));

        let failing = "lawh version=1\nmath id=m1 x=0 y=0 \"frac(1,\"\n";
        let failing_path = dir.write("failing.lawh", failing);
        let out = dir.path("failing.png");
        let note = export_file(
            &args(failing_path, out.clone()),
            &from_lawh(failing).unwrap(),
            &mut renderer,
        )
        .unwrap();
        assert_eq!(
            note,
            Some(format!("\"{}\" exported, 1 formula failed", out.display()))
        );
        assert!(fs::read(&out).unwrap().starts_with(b"\x89PNG\r\n\x1a\n"));
    }

    #[test]
    fn exporting_onto_the_board_itself_writes_nothing() {
        let dir = TestDir::new("cli-export-own-file");
        let board_path = dir.write("board.svg", AREA);
        let board = from_lawh(AREA).unwrap();
        let mut renderer = Renderer::new_trusted();
        let err = export_file(
            &args(board_path.clone(), board_path.clone()),
            &board,
            &mut renderer,
        )
        .unwrap_err();
        assert_eq!(
            format!("{err:#}"),
            format!(
                "cannot export \"{}\": that is the board's own file",
                board_path.display()
            )
        );
        assert_eq!(fs::read_to_string(&board_path).unwrap(), AREA);

        // Another path to it: `.` and a link.
        let dotted = dir.root().join(".").join("board.svg");
        export_file(&args(board_path.clone(), dotted), &board, &mut renderer).unwrap_err();
        #[cfg(unix)]
        {
            let link = dir.path("link.pdf");
            std::os::unix::fs::symlink("board.svg", &link).unwrap();
            export_file(&args(board_path.clone(), link), &board, &mut renderer).unwrap_err();
        }
        assert_eq!(fs::read_to_string(&board_path).unwrap(), AREA);
    }

    #[test]
    fn export_and_write_errors_name_the_file() {
        let dir = TestDir::new("cli-export-errors");
        let board_path = dir.write("empty.lawh", "lawh version=1\n");
        let mut renderer = Renderer::new_trusted();
        let out = dir.path("empty.png");
        let err =
            export_file(&args(board_path, out.clone()), &Board::new(), &mut renderer).unwrap_err();
        assert_eq!(
            format!("{err:#}"),
            format!("cannot export \"{}\": nothing to export", out.display())
        );
        assert!(!out.exists());

        let board_path = dir.write("area.lawh", AREA);
        let out = dir.path("no-such-dir").join("area.pdf");
        let err = export_file(
            &args(board_path, out.clone()),
            &from_lawh(AREA).unwrap(),
            &mut renderer,
        )
        .unwrap_err();
        let err = format!("{err:#}");
        assert!(
            err.starts_with(&format!("cannot export \"{}\": ", out.display())),
            "{err}"
        );
        assert!(!out.exists());
    }
}
