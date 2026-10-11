//! The `:` command parser on its own: every command and short form, numbers, and the errors.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

use lawh_core::engine::{Command, CommandError, End, Segment, parse_command};

fn parsed(line: &str) -> Command {
    parse_command(line).unwrap_or_else(|e| panic!("{line:?}: {e}"))
}

fn error(line: &str) -> String {
    match parse_command(line) {
        Ok(command) => panic!("{line:?} parsed as {command:?}"),
        Err(e) => e.to_string(),
    }
}

/// Every abbreviation of `name` from `min` letters on.
fn short_forms(name: &str, min: usize) -> Vec<&str> {
    (min..=name.len()).map(|n| &name[..n]).collect()
}

#[test]
fn every_short_form_names_its_command() {
    let cases = [
        ("arrow", 2, "ar 1 2"),
        ("circle", 2, "ci 1"),
        ("ellipse", 2, "el 1 2"),
        ("line", 2, "li 1 2"),
        ("math", 2, "ma x"),
        ("quit", 1, "q"),
        ("rect", 2, "re 1 2"),
        ("redo", 3, "red"),
        ("text", 2, "te x"),
        ("undo", 1, "u"),
        ("wq", 2, "wq"),
        ("write", 1, "w"),
        ("xit", 1, "x"),
    ];
    for (name, min, example) in cases {
        let expected = parsed(example);
        let args = example.split_once(' ').map_or("", |(_, rest)| rest);
        for short in short_forms(name, min) {
            let line = if args.is_empty() {
                short.to_owned()
            } else {
                format!("{short} {args}")
            };
            assert_eq!(parse_command(&line), Ok(expected.clone()), "{line:?}");
        }
        // One letter less is not this command.
        if min > 1 {
            let line = format!("{} {args}", &name[..min - 1]);
            assert_ne!(parse_command(&line), Ok(expected), "{line:?}");
        }
    }
}

#[test]
fn one_letter_forms_are_only_w_q_x_and_u() {
    for letter in 'a'..='z' {
        let line = letter.to_string();
        let ok = parse_command(&line).is_ok();
        assert_eq!(ok, "qwxu".contains(letter), ":{letter}");
    }
    for line in [
        "c 1", "e 1 2", "r 1 2", "l 1 2", "a 1 2", "t x", "m x", "s", "e file",
    ] {
        assert!(error(line).starts_with("unknown command"), "{line:?}");
    }
}

#[test]
fn shapes_take_positionals_and_keys() {
    let circle = |x, y, r| Command::Circle { x, y, r };
    assert_eq!(parsed("ci 3"), circle(None, None, 3.0));
    assert_eq!(parsed("cir   2.5  "), circle(None, None, 2.5));
    assert_eq!(parsed("ci r=2"), circle(None, None, 2.0));
    assert_eq!(parsed("ci y=-1 x=.5 4"), circle(Some(0.5), Some(-1.0), 4.0));
    // `r=` wins over the positional radius.
    assert_eq!(parsed("ci 4 r=1"), circle(None, None, 1.0));
    assert_eq!(parsed(" ::ci\t+2."), circle(None, None, 2.0));

    let ellipse = |x, y, rx, ry| Command::Ellipse { x, y, rx, ry };
    assert_eq!(parsed("el 3 1"), ellipse(None, None, 3.0, 1.0));
    assert_eq!(parsed("ellipse ry=2 3 1"), ellipse(None, None, 3.0, 2.0));
    assert_eq!(
        parsed("el rx=1 ry=2 x=-3 y=4"),
        ellipse(Some(-3.0), Some(4.0), 1.0, 2.0)
    );

    let rect = |x, y, w, h| Command::Rect { x, y, w, h };
    assert_eq!(parsed("re 4 2"), rect(None, None, 4.0, 2.0));
    assert_eq!(parsed("rect x=1 4 h=.5 2"), rect(Some(1.0), None, 4.0, 0.5));
    assert_eq!(parsed("rec w=2. h=3 y=0"), rect(None, Some(0.0), 2.0, 3.0));

    let segment = |x1, y1, x2, y2| Segment { x1, y1, x2, y2 };
    let (rel, abs) = (End::Relative, End::Absolute);
    assert_eq!(
        parsed("li 3 -4"),
        Command::Line(segment(None, None, rel(3.0), rel(-4.0)))
    );
    assert_eq!(
        parsed("line x2=5 y2=6"),
        Command::Line(segment(None, None, abs(5.0), abs(6.0)))
    );
    // Keys win over positionals, one end at a time.
    assert_eq!(
        parsed("ar 1 2 y2=0 x1=-1"),
        Command::Arrow(segment(Some(-1.0), None, rel(1.0), abs(0.0)))
    );
    assert_eq!(
        parsed("arrow 1 y2=7"),
        Command::Arrow(segment(None, None, rel(1.0), abs(7.0)))
    );
    // A line may have no length.
    assert_eq!(
        parsed("li 0 0"),
        Command::Line(segment(None, None, rel(0.0), rel(0.0)))
    );
}

#[test]
fn numbers_are_plain_decimals() {
    let r = |line: &str| match parsed(line) {
        Command::Circle { r, .. } => r,
        other => panic!("{other:?}"),
    };
    assert_eq!(r("ci 3"), 3.0);
    assert_eq!(r("ci .5"), 0.5);
    assert_eq!(r("ci 2."), 2.0);
    assert_eq!(r("ci +1.25"), 1.25);
    assert_eq!(r("ci 007"), 7.0);
    assert_eq!(
        parsed("ci 1 x=-2.5"),
        Command::Circle {
            x: Some(-2.5),
            y: None,
            r: 1.0
        }
    );
    for bad in [
        "1e3", "inf", "nan", "--1", "-inf", "1_0", "0x1", "1.2.3", ".", "-",
    ] {
        assert_eq!(
            parse_command(&format!("ci {bad}")),
            Err(CommandError::NotANumber(bad.to_owned())),
            "{bad}"
        );
        assert_eq!(
            parse_command(&format!("ci 1 x={bad}")),
            Err(CommandError::NotANumber(bad.to_owned())),
            "x={bad}"
        );
    }
}

#[test]
fn text_and_math_are_verbatim_after_the_first_space() {
    let special = r#"a | b "c" # d \n %e $f <g> \"#;
    assert_eq!(
        parsed(&format!("te {special}")),
        Command::Text(special.to_owned())
    );
    assert_eq!(
        parsed(&format!("ma {special}")),
        Command::Math(special.to_owned())
    );
    // Only the first space goes: more spaces at the start, and any at the end, are kept.
    assert_eq!(parsed("te   two  "), Command::Text("  two  ".to_owned()));
    assert_eq!(parsed("text\tx"), Command::Text("x".to_owned()));
    assert_eq!(
        parsed("math A = pi r^2"),
        Command::Math("A = pi r^2".to_owned())
    );
    // Nothing, or only spaces, is no text.
    assert_eq!(error("te"), "missing the text: `:te[xt] <text>`");
    assert_eq!(error("te    "), "missing the text: `:te[xt] <text>`");
    assert_eq!(error("ma "), "missing the formula: `:ma[th] <formula>`");
    assert_eq!(error("te! x"), "`:text` takes no `!`");
    // The name must end at a space.
    assert_eq!(error("te|x"), "unknown command `:te|x`");
}

#[test]
fn write_and_quit_forms() {
    let write = |file: Option<&str>| Command::Write {
        file: file.map(str::to_owned),
    };
    assert_eq!(parsed("w"), write(None));
    assert_eq!(parsed("write"), write(None));
    assert_eq!(parsed("w!"), write(None));
    assert_eq!(parsed("w board.lawh"), write(Some("board.lawh")));
    assert_eq!(parsed("w!  my board.lawh  "), write(Some("my board.lawh")));
    assert_eq!(parsed("q"), Command::Quit { force: false });
    assert_eq!(parsed("quit!"), Command::Quit { force: true });
    assert_eq!(parsed("wq"), Command::WriteQuit { file: None });
    assert_eq!(
        parsed("wq! b.lawh"),
        Command::WriteQuit {
            file: Some("b.lawh".to_owned())
        }
    );
    assert_eq!(parsed("x"), Command::Exit { file: None });
    assert_eq!(
        parsed("xit! c"),
        Command::Exit {
            file: Some("c".to_owned())
        }
    );
    assert_eq!(parsed("u"), Command::Undo);
    assert_eq!(parsed("undo  "), Command::Undo);
    assert_eq!(parsed("redo"), Command::Redo);
}

#[test]
fn errors_say_what_is_wrong() {
    let cases = [
        ("", "type a command after `:`"),
        (" : ", "type a command after `:`"),
        ("foo", "unknown command `:foo`"),
        ("foo bar", "unknown command `:foo`"),
        ("c 3", "unknown command `:c`"),
        ("r 3", "unknown command `:r`"),
        ("circles 3", "unknown command `:circles`"),
        ("ci3", "unknown command `:ci3`"),
        ("Ci 3", "unknown command `:Ci`"),
        ("3", "unknown command `:3`"),
        ("!ls", "unknown command `:!ls`"),
        ("ci! 3", "`:circle` takes no `!`"),
        ("u 2", "`:undo` takes no arguments"),
        ("q now", "`:quit` takes no arguments"),
        ("redo!", "`:redo` takes no `!`"),
        (
            "ci",
            "missing a number: `:ci[rcle] R`, with `x=`, `y=` or `r=`",
        ),
        (
            "ci x=1 y=1",
            "missing a number: `:ci[rcle] R`, with `x=`, `y=` or `r=`",
        ),
        (
            "ci 1 2",
            "too many numbers: `:ci[rcle] R`, with `x=`, `y=` or `r=`",
        ),
        (
            "el 1",
            "missing a number: `:el[lipse] RX RY`, with `x=`, `y=`, `rx=` or `ry=`",
        ),
        (
            "re 3",
            "missing a number: `:re[ct] W H`, with `x=`, `y=`, `w=` or `h=`",
        ),
        (
            "re 1 2 3",
            "too many numbers: `:re[ct] W H`, with `x=`, `y=`, `w=` or `h=`",
        ),
        ("li", "a line needs two numbers, or `x2=` and `y2=`"),
        ("li 1", "a line needs two numbers, or `x2=` and `y2=`"),
        ("ar x2=1", "an arrow needs two numbers, or `x2=` and `y2=`"),
        (
            "li 1 2 x=0",
            "unknown key `x=`: `:li[ne] DX DY`, with `x1=`, `y1=`, `x2=` or `y2=`",
        ),
        ("ci abc", "`abc` is not a number"),
        ("ci 1e3", "`1e3` is not a number"),
        ("ci r=", "`` is not a number"),
        (
            "ci 2 z=1",
            "unknown key `z=`: `:ci[rcle] R`, with `x=`, `y=` or `r=`",
        ),
        (
            "ci 2 =1",
            "unknown key `=`: `:ci[rcle] R`, with `x=`, `y=` or `r=`",
        ),
        ("ci x=1 x=2 3", "`x=` is given twice"),
        ("ci r=1 r=1", "`r=` is given twice"),
        ("ci 0", "radius must be above 0"),
        ("ci -2", "radius must be above 0"),
        ("ci -0", "radius must be above 0"),
        ("ci 1000000.5", "radius must be at most 1000000"),
        ("el 1 0", "each radius must be above 0"),
        ("re 0 1", "width must be above 0"),
        ("re 1 -1", "height must be above 0"),
        ("re 1 h=2000000", "height must be at most 1000000"),
        (
            "w !sudo tee %",
            "writing to a command or appending to a file is not supported: `:w !sudo tee %`",
        ),
        (
            "w >>log",
            "writing to a command or appending to a file is not supported: `:w >>log`",
        ),
    ];
    for (line, message) in cases {
        assert_eq!(
            parse_command(line).map_err(|e| e.to_string()),
            Err(message.to_owned()),
            "{line:?}"
        );
    }
}
