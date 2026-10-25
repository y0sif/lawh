//! `:export`, keys in and effects out: the command and its short form, the format from the
//! file's extension, the whole board or the Visual selection, the `exportbg` and `pngscale`
//! options, `:w` refusing an image's or a PDF's name, the cap on exports per key, and the
//! message [`Engine::export_done`] leaves. The renderer draws exports; here only what the
//! engine asks the app for is checked.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

mod common;

use common::{Run, board, run, run_engine, run_with_rc};
use insta::assert_snapshot;
use lawh_core::engine::{Effect, Engine, MAX_EXPORTS_PER_KEY, MAX_WRITES_PER_KEY, RcWarning};
use lawh_core::export::{DEFAULT_PNG_SCALE_HUNDREDTHS, MAX_PNG_SCALE_HUNDREDTHS};
use lawh_core::{
    Board, ExportBackground, ExportFormat, ExportOptions, ExportReport, Mode, ObjectId,
};

/// A circle and a rectangle in a row, a text below them:
///
/// ```text
/// y -1..1     c1 (-1..1)   r1 (4..6)
/// y 4..4.6    t1 (-1..0.5)
/// ```
fn shapes() -> Board {
    board(
        "circle id=c1 x=0 y=0 r=1
rect id=r1 x=4 y=-1 w=2 h=2
text id=t1 x=-1 y=4 \"notes\"
",
    )
}

fn id(s: &str) -> ObjectId {
    ObjectId::new(s).unwrap()
}

fn message(r: &Run) -> Option<&str> {
    r.engine.message().map(|m| m.text.as_str())
}

/// The message, which must be an error.
fn error(r: &Run) -> &str {
    let m = r.engine.message().expect("a message");
    assert!(m.error, "not an error: {m:?}");
    &m.text
}

/// The message, which must be information.
fn info(r: &Run) -> &str {
    let m = r.engine.message().expect("a message");
    assert!(!m.error, "an error: {m:?}");
    &m.text
}

/// The export of the whole board to `path` with the default options for `format`.
fn whole(path: &str, format: ExportFormat) -> Effect {
    Effect::Export {
        path: path.to_owned(),
        options: ExportOptions {
            format,
            ..ExportOptions::default()
        },
        selection: None,
    }
}

/// The exports `r`'s keys asked for, and its writes.
fn exports_and_writes(r: &Run) -> (usize, usize) {
    let exports = r
        .effects
        .iter()
        .filter(|e| matches!(e, Effect::Export { .. }));
    let writes = r
        .effects
        .iter()
        .filter(|e| matches!(e, Effect::Write { .. }));
    (exports.count(), writes.count())
}

/// The message when one key asks for more than [`MAX_EXPORTS_PER_KEY`] exports.
fn too_many_exports() -> String {
    format!("that key asked for more than {MAX_EXPORTS_PER_KEY} export; the rest of it was dropped")
}

/// The message when one key asks for more than [`MAX_WRITES_PER_KEY`] writes.
fn too_many_writes() -> String {
    format!("that key asked for more than {MAX_WRITES_PER_KEY} writes; the rest of it was dropped")
}

/// An engine for [`shapes`] named `a.lawh`, with `rc` read, after `script`.
fn named(rc: &str, script: &str) -> Run {
    let mut engine = Engine::new(shapes()).with_file_name(Some("a.lawh".to_owned()));
    assert_eq!(engine.source(rc), []);
    run_engine(engine, script)
}

// The command.

#[test]
fn export_asks_the_app_for_the_whole_board() {
    let r = run(shapes(), ":export out.png<CR>");
    assert_eq!(r.effects, [whole("out.png", ExportFormat::Png)]);
    assert_eq!(message(&r), None);
    assert_eq!(r.engine.mode(), Mode::Normal);
    // Nothing changed on the board, and an export does not name it.
    assert_eq!(r.engine.board(), &shapes());
    assert!(!r.engine.is_modified());
    assert_eq!(r.engine.file_name(), None);
}

#[test]
fn exp_is_the_shortest_form_and_a_bang_changes_nothing() {
    let expected = [whole("a.svg", ExportFormat::Svg)];
    for line in [
        ":exp a.svg<CR>",
        ":expo a.svg<CR>",
        ":expor a.svg<CR>",
        ":export a.svg<CR>",
        ":exp! a.svg<CR>",
        ":export!   a.svg  <CR>",
    ] {
        let r = run(shapes(), line);
        assert_eq!(r.effects, expected, "{line}");
        assert_eq!(message(&r), None, "{line}");
    }
    // `:ex` stays free (Vim's `:edit`), and `:e` with it.
    for (line, unknown) in [(":ex a.svg<CR>", "ex"), (":e a.svg<CR>", "e")] {
        let r = run(shapes(), line);
        assert_eq!(r.effects, [], "{line}");
        assert_eq!(error(&r), format!("unknown command `:{unknown}`"));
    }
}

#[test]
fn the_format_comes_from_the_extension_in_any_case() {
    for (file, format) in [
        ("out.svg", ExportFormat::Svg),
        ("out.PNG", ExportFormat::Png),
        ("board.Pdf", ExportFormat::Pdf),
        ("~/notes/area of a circle.png", ExportFormat::Png),
        ("dir.pdf/out.svg", ExportFormat::Svg),
    ] {
        let r = run(shapes(), &format!(":exp {file}<CR>"));
        assert_eq!(r.effects, [whole(file, format)], "{file}");
    }
}

#[test]
fn export_says_what_is_wrong_and_asks_for_nothing() {
    for (line, expected) in [
        (
            ":exp<CR>",
            "missing the file: `:exp[ort] <file>`, with .svg, .png or .pdf",
        ),
        (
            ":export!  <CR>",
            "missing the file: `:exp[ort] <file>`, with .svg, .png or .pdf",
        ),
        (":exp out<CR>", "export needs .svg, .png or .pdf"),
        (":exp out.jpg<CR>", "export needs .svg, .png or .pdf"),
        (":exp out.lawh<CR>", "export needs .svg, .png or .pdf"),
        (":exp .png<CR>", "export needs .svg, .png or .pdf"),
        (":exp out.png.lawh<CR>", "export needs .svg, .png or .pdf"),
        (
            ":exp !convert a.png<CR>",
            "writing to a command or appending to a file is not supported: `:exp !convert a.png`",
        ),
        (
            ":export >>a.png<CR>",
            "writing to a command or appending to a file is not supported: `:export >>a.png`",
        ),
    ] {
        let r = run(shapes(), line);
        assert_eq!(r.effects, [], "{line}");
        assert_eq!(error(&r), expected, "{line}");
        assert_eq!(r.engine.mode(), Mode::Normal, "{line}");
    }
}

#[test]
fn an_empty_board_has_nothing_to_export() {
    let r = run(Board::new(), ":exp a.png<CR>");
    assert_eq!(r.effects, []);
    assert_eq!(error(&r), "nothing to export");
    // A refused export takes no slot: the next one in the key goes.
    let rc = "nnoremap Q :exp a.png<CR>:ci 1<CR>:exp b.png<CR>\n";
    let r = run_with_rc(rc, Board::new(), "Q");
    assert_eq!(r.effects, [whole("b.png", ExportFormat::Png)]);
}

#[test]
fn export_from_block_normal_mode_exports_the_board_and_comes_back() {
    let r = run(shapes(), "G<CR>:exp a.pdf<CR>");
    assert_eq!(r.effects, [whole("a.pdf", ExportFormat::Pdf)]);
    assert_eq!(r.engine.mode(), Mode::BlockNormal);
}

#[test]
fn from_insert_mode_the_command_line_does_not_export() {
    let mut r = run(shapes(), "Ga<C-k>");
    // Take `sym ` back off the line and type an export instead.
    r.feed("<BS><BS><BS><BS>exp a.png<CR>");
    assert_eq!(r.effects, []);
    assert_eq!(
        error(&r),
        "from Insert mode the command line runs only `:sym`"
    );
    assert_eq!(r.engine.mode(), Mode::Insert);
}

// Visual mode.

#[test]
fn from_visual_mode_export_takes_the_selection() {
    // `v` on `c1`, `L` to `r1`: the rectangle holds both, not the text below.
    let mut r = run(shapes(), "ggvL:");
    assert_eq!(r.engine.selection(), [id("c1"), id("r1")]);
    r.feed("exp sel.pdf<CR>");
    assert_eq!(
        r.effects,
        [Effect::Export {
            path: "sel.pdf".to_owned(),
            options: ExportOptions {
                format: ExportFormat::Pdf,
                ..ExportOptions::default()
            },
            selection: Some(vec![id("c1"), id("r1")]),
        }]
    );
    // The command line closed, and the selection with it.
    assert_snapshot!(r.header());
    assert_eq!(r.engine.selection(), []);
    // `@:` runs the line again outside Visual mode: the whole board.
    r.effects.clear();
    r.feed("@:");
    assert_eq!(r.effects, [whole("sel.pdf", ExportFormat::Pdf)]);
}

#[test]
fn the_selection_goes_in_z_order() {
    // `v` on the text, then to the circle and across to the rectangle: everything, in the order
    // of the file, not the order the cursor went.
    let r = run(shapes(), "GvggL:exp all.svg<CR>");
    let Some(Effect::Export { selection, .. }) = r.effects.first() else {
        panic!("{:?}", r.effects);
    };
    assert_eq!(
        selection.as_deref(),
        Some([id("c1"), id("r1"), id("t1")].as_slice())
    );
}

#[test]
fn an_empty_selection_has_nothing_to_export() {
    // Visual mode from a grid point far from every object.
    let r = run(shapes(), "20jv:exp a.png<CR>");
    assert_eq!(r.effects, []);
    assert_eq!(error(&r), "nothing to export");
    assert_eq!(r.engine.mode(), Mode::Normal);
}

// The options.

#[test]
fn exports_take_the_exportbg_and_pngscale_options() {
    let r = run(
        shapes(),
        ":set pngscale=0.5 exportbg=none<CR>:exp a.png<CR>:exp b.svg<CR>",
    );
    let options = |format| ExportOptions {
        format,
        background: ExportBackground::None,
        png_scale_hundredths: 50,
    };
    let effect = |path: &str, format| Effect::Export {
        path: path.to_owned(),
        options: options(format),
        selection: None,
    };
    assert_eq!(
        r.effects,
        [
            effect("a.png", ExportFormat::Png),
            effect("b.svg", ExportFormat::Svg)
        ]
    );
}

#[test]
fn pngscale_and_exportbg_show_set_and_reset_like_other_options() {
    let mut r = run(Board::new(), ":set pngscale?<CR>");
    assert_eq!(info(&r), "pngscale=2");
    r.feed(":set exportbg<CR>");
    assert_eq!(info(&r), "exportbg=paper");
    r.feed(":set pngscale=1.25 exportbg=none<CR>:set pngscale exportbg?<CR>");
    assert_eq!(info(&r), "pngscale=1.25  exportbg=none");
    assert_eq!(r.engine.settings().pngscale_hundredths, 125);
    assert_eq!(r.engine.settings().exportbg, ExportBackground::None);
    // The ends of the range, and `:` for `=`.
    r.feed(":set pngscale=.25<CR>:set pngscale?<CR>");
    assert_eq!(info(&r), "pngscale=0.25");
    r.feed(":set pngscale:8<CR>:set pngscale?<CR>");
    assert_eq!(info(&r), "pngscale=8");
    assert_eq!(
        r.engine.settings().pngscale_hundredths,
        MAX_PNG_SCALE_HUNDREDTHS
    );
    r.feed(":set pngscale& exportbg&<CR>:set pngscale? exportbg?<CR>");
    assert_eq!(info(&r), "pngscale=2  exportbg=paper");
    assert_eq!(
        r.engine.settings().pngscale_hundredths,
        DEFAULT_PNG_SCALE_HUNDREDTHS
    );
    // Bad values are refused and change nothing.
    for (value, usage) in [
        (
            "pngscale=9",
            "a scale from 0.25 to 8, with at most two decimals",
        ),
        (
            "pngscale=8.01",
            "a scale from 0.25 to 8, with at most two decimals",
        ),
        (
            "pngscale=0.2",
            "a scale from 0.25 to 8, with at most two decimals",
        ),
        (
            "pngscale=0",
            "a scale from 0.25 to 8, with at most two decimals",
        ),
        (
            "pngscale=1.234",
            "a scale from 0.25 to 8, with at most two decimals",
        ),
        (
            "pngscale=-1",
            "a scale from 0.25 to 8, with at most two decimals",
        ),
        (
            "pngscale=2x",
            "a scale from 0.25 to 8, with at most two decimals",
        ),
        (
            "pngscale=",
            "a scale from 0.25 to 8, with at most two decimals",
        ),
        ("exportbg=white", "`paper` or `none`"),
        ("exportbg=Paper", "`paper` or `none`"),
        ("exportbg=", "`paper` or `none`"),
    ] {
        r.feed(&format!(":set {value}<CR>"));
        let (option, typed) = value.split_once('=').unwrap();
        assert_eq!(
            error(&r),
            format!("bad value `{typed}` for `{option}`: it takes {usage}"),
            "{value}"
        );
    }
    assert_eq!(r.engine.settings().pngscale_hundredths, 200);
    assert_eq!(r.engine.settings().exportbg, ExportBackground::Paper);
}

#[test]
fn lawhrc_sets_the_export_options_but_does_not_export() {
    let mut engine = Engine::new(shapes());
    let warnings = engine.source("set pngscale=4 exportbg=none\nexport a.png\nexp b.svg\n");
    assert_eq!(
        warnings,
        [
            RcWarning {
                line: 2,
                message: "`:export` is not allowed in lawhrc, only `:set` and mapping commands \
                          are"
                .to_owned(),
            },
            RcWarning {
                line: 3,
                message: "`:exp` is not allowed in lawhrc, only `:set` and mapping commands are"
                    .to_owned(),
            },
        ]
    );
    let r = run_engine(engine, ":exp a.png<CR>");
    assert_eq!(
        r.effects,
        [Effect::Export {
            path: "a.png".to_owned(),
            options: ExportOptions {
                format: ExportFormat::Png,
                background: ExportBackground::None,
                png_scale_hundredths: 400,
            },
            selection: None,
        }]
    );
}

// `:w` to an export's name.

#[test]
fn w_refuses_a_file_named_as_an_export_and_names_export() {
    for (line, command, file, extension) in [
        (":w out.png<CR>", "w", "out.png", "png"),
        (":write  out.svg <CR>", "write", "out.svg", "svg"),
        (":w! OUT.PDF<CR>", "w!", "OUT.PDF", "pdf"),
        (":wq a.pdf<CR>", "wq", "a.pdf", "pdf"),
        (":wq! ~/b.png<CR>", "wq!", "~/b.png", "png"),
        (":x c.svg<CR>", "x", "c.svg", "svg"),
        (":xit! d.Png<CR>", "xit!", "d.Png", "png"),
    ] {
        // An unnamed board with changes: the write would have named it.
        let r = run(Board::new(), &format!(":ci 1<CR>{line}"));
        assert_eq!(r.effects, [], "{line}");
        assert_eq!(
            error(&r),
            format!("`:{command}` writes lawh boards: use `:export {file}` for .{extension}"),
            "{line}"
        );
        assert_eq!(r.engine.file_name(), None, "{line}");
        assert!(r.engine.is_modified(), "{line}");
    }
    // `:x` to such a file on a board without changes is refused too: it doesn't quit.
    let r = run(shapes(), ":x out.png<CR>");
    assert_eq!(r.effects, []);
    assert_eq!(
        error(&r),
        "`:x` writes lawh boards: use `:export out.png` for .png"
    );
}

#[test]
fn a_board_opened_under_an_exports_name_still_saves_with_w() {
    // `lawh notes.svg`: only a file typed after `:w` is checked.
    let engine = Engine::new(Board::new()).with_file_name(Some("notes.svg".to_owned()));
    let mut r = run_engine(engine, ":ci 1<CR>:w<CR>");
    assert_eq!(
        r.effects,
        [Effect::Write {
            path: "notes.svg".to_owned(),
            quit_after: false
        }]
    );
    r.write_done("notes.svg", Ok(()));
    assert!(!r.engine.is_modified());
    // And `:w` to another lawh file still writes a copy.
    r.effects.clear();
    r.feed(":w copy.lawh<CR>");
    assert_eq!(exports_and_writes(&r), (0, 1));
}

// The cap on exports per key.

#[test]
fn one_key_asks_for_at_most_one_export() {
    let mut r = named("nnoremap Q :exp a.png<CR>:exp b.pdf<CR>\n", "Q");
    assert_eq!(r.effects, [whole("a.png", ExportFormat::Png)]);
    assert_eq!(error(&r), too_many_exports());
    // The key stopped: the typeahead is dropped and the command line closed.
    assert_eq!(r.engine.pending(), "");
    assert_eq!(r.engine.mode(), Mode::Normal);
    // The next keys work, and the next `Q` exports again.
    r.effects.clear();
    r.feed(":ci 2<CR>");
    assert_eq!(message(&r), None);
    assert_eq!(r.engine.board().len(), 4);
    r.feed("Q");
    assert_eq!(exports_and_writes(&r), (1, 0));
    assert_eq!(error(&r), too_many_exports());
    // Typed one key at a time, each export goes.
    r.effects.clear();
    r.feed(":exp a.png<CR>:exp b.pdf<CR>");
    assert_eq!(exports_and_writes(&r), (2, 0));
    assert_eq!(message(&r), None);
}

#[test]
fn a_mapping_that_exports_then_runs_itself_exports_once() {
    let r = named("nmap Q :exp a.png<CR>Q\n", "Q");
    assert_eq!(r.effects, [whole("a.png", ExportFormat::Png)]);
    assert_eq!(error(&r), too_many_exports());
    assert_eq!(r.engine.pending(), "");
}

#[test]
fn at_colon_and_counts_export_once_a_key() {
    // `@:` inside a mapping is a second export of the same key.
    let r = named("nnoremap Q :exp a.png<CR>@:\n", "Q");
    assert_eq!(exports_and_writes(&r), (1, 0));
    assert_eq!(error(&r), too_many_exports());
    // `@:` typed after it is a key of its own; a count on `@:` is ignored.
    let mut r = named("", ":exp a.png<CR>@:");
    assert_eq!(exports_and_writes(&r), (2, 0));
    assert_eq!(message(&r), None);
    r.effects.clear();
    r.feed("3@:");
    assert_eq!(exports_and_writes(&r), (1, 0));
    assert_eq!(message(&r), None);
    // `.` repeats changes, never `:` lines: after an insert and an export, no export.
    r.feed("Gaz<Esc>:exp a.png<CR>");
    r.effects.clear();
    r.feed(".3.");
    assert_eq!(exports_and_writes(&r), (0, 0));
    assert_eq!(r.engine.board().len(), 3);
}

#[test]
fn an_export_takes_one_of_the_keys_writes() {
    // An export and three writes fit; a fourth write does not.
    let writes = ":w<CR>".repeat(MAX_WRITES_PER_KEY - 1);
    let r = named(
        &format!("nnoremap Q :exp a.png<CR>{writes}\n"),
        ":ci 2<CR>Q",
    );
    assert_eq!(exports_and_writes(&r), (1, MAX_WRITES_PER_KEY - 1));
    assert_eq!(message(&r), None);
    let r = named(
        &format!("nnoremap Q :exp a.png<CR>{writes}:w<CR>\n"),
        ":ci 2<CR>Q",
    );
    assert_eq!(exports_and_writes(&r), (1, MAX_WRITES_PER_KEY - 1));
    assert_eq!(error(&r), too_many_writes());
    // After as many writes as the cap, the export is refused as a write.
    let writes = ":w<CR>".repeat(MAX_WRITES_PER_KEY);
    let r = named(
        &format!("nnoremap Q {writes}:exp a.png<CR>\n"),
        ":ci 2<CR>Q",
    );
    assert_eq!(exports_and_writes(&r), (0, MAX_WRITES_PER_KEY));
    assert_eq!(error(&r), too_many_writes());
}

// What the app reports.

#[test]
fn a_good_export_says_so_and_leaves_the_board_unnamed_and_modified() {
    let mut r = run(Board::new(), ":ci 1<CR>:exp out.pdf<CR>");
    r.export_done("out.pdf", Ok(ExportReport::default()));
    assert_eq!(info(&r), "\"out.pdf\" exported");
    assert_eq!(r.engine.file_name(), None);
    assert!(r.engine.is_modified());
    // On a named board, the board's own file is still the one `:w` writes.
    let mut r = named("", ":ci 2<CR>:exp a.png<CR>");
    r.export_done("a.png", Ok(ExportReport::default()));
    assert_eq!(r.engine.file_name(), Some("a.lawh"));
    assert!(r.engine.is_modified());
}

#[test]
fn the_report_names_failed_formulas_and_a_shrink() {
    let report = |failed_formulas, shrunk| ExportReport {
        failed_formulas,
        shrunk,
    };
    for (report, expected) in [
        (report(1, None), "\"out.png\" exported, 1 formula failed"),
        (report(2, None), "\"out.png\" exported, 2 formulas failed"),
        (
            report(500, None),
            "\"out.png\" exported, 500 formulas failed",
        ),
        (
            report(0, Some(0.5)),
            "\"out.png\" exported, shrunk to 0.5x to fit",
        ),
        (
            report(0, Some(0.999)),
            "\"out.png\" exported, shrunk to 0.99x to fit",
        ),
        (
            report(0, Some(0.000_164)),
            "\"out.png\" exported, shrunk to 0.00016x to fit",
        ),
        (
            report(3, Some(0.25)),
            "\"out.png\" exported, 3 formulas failed, shrunk to 0.25x to fit",
        ),
        // A factor the renderer should never report shows no number.
        (report(0, Some(1e-9)), "\"out.png\" exported, shrunk to fit"),
        (
            report(0, Some(f64::NAN)),
            "\"out.png\" exported, shrunk to fit",
        ),
    ] {
        let mut r = run(shapes(), ":exp out.png<CR>");
        r.export_done("out.png", Ok(report));
        assert_eq!(info(&r), expected);
    }
}

#[test]
fn a_failed_export_is_an_error_with_the_file_name() {
    let mut r = run(shapes(), ":exp out.pdf<CR>");
    r.export_done("out.pdf", Err("export stopped: the file is over 64 MiB"));
    assert_eq!(
        error(&r),
        "cannot export \"out.pdf\": export stopped: the file is over 64 MiB"
    );
    // Nothing was saved or named.
    assert_eq!(r.engine.file_name(), None);
    // The next key starts with no message.
    r.feed("l");
    assert_eq!(message(&r), None);
}

#[test]
fn reports_follow_the_message_rules_of_writes() {
    // A key refused for a second export, then the first one's report: the refusal stays.
    let mut r = named("nnoremap Q :exp a.png<CR>:exp b.png<CR>\n", "Q");
    r.export_done("a.png", Ok(ExportReport::default()));
    assert_eq!(error(&r), too_many_exports());
    // Information from a command, then a failed export: the failure.
    let mut r = named("nnoremap Q :set pngscale?<CR>:exp a.png<CR>\n", "Q");
    assert_eq!(info(&r), "pngscale=2");
    r.export_done("a.png", Err("disk full"));
    assert_eq!(error(&r), "cannot export \"a.png\": disk full");
    // A failed write, then a good export: the failure.
    let mut r = named("nnoremap Q :w<CR>:exp a.png<CR>\n", ":ci 2<CR>Q");
    r.write_done("a.lawh", Err("disk full"));
    r.export_done("a.png", Ok(ExportReport::default()));
    assert_eq!(error(&r), "cannot write \"a.lawh\": disk full");
    // A good write, then a good export: the export, reported last.
    let mut r = named("nnoremap Q :w<CR>:exp a.png<CR>\n", ":ci 2<CR>Q");
    r.write_done("a.lawh", Ok(()));
    r.export_done("a.png", Ok(ExportReport::default()));
    assert_eq!(info(&r), "\"a.png\" exported");
    assert!(!r.engine.is_modified());
}

#[test]
fn a_report_the_engine_did_not_ask_for_only_sets_the_message() {
    let mut r = run(shapes(), "");
    r.export_done("stray.svg", Ok(ExportReport::default()));
    assert_eq!(info(&r), "\"stray.svg\" exported");
    assert_eq!(r.engine.file_name(), None);
    assert_eq!(r.engine.board(), &shapes());
}
