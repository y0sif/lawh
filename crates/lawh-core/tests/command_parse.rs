//! The `:` command parser on its own: every command and short form, numbers, mapping commands,
//! `:set`, and the errors.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

use lawh_core::engine::{
    Command, CommandError, End, Escape, MAX_MAP_KEYS, MAX_TIMEOUTLEN, MapKey, MapMode, OptionName,
    OptionValue, Segment, SetArg, parse_command,
};
use lawh_core::{Key, parse_keys};

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
        // Mapping commands, with Vim's short forms. `:map` takes all three letters, since `:ma`
        // is `:math`; `:un` is `:undo`.
        ("map", 3, "map a b"),
        ("noremap", 2, "no a b"),
        ("unmap", 3, "unm a"),
        ("mapclear", 4, "mapc"),
        ("nmap", 2, "nm a b"),
        ("nnoremap", 2, "nn a b"),
        ("nunmap", 3, "nun a"),
        ("nmapclear", 5, "nmapc"),
        ("vmap", 2, "vm a b"),
        ("vnoremap", 2, "vn a b"),
        ("vunmap", 2, "vu a"),
        ("vmapclear", 5, "vmapc"),
        ("xmap", 2, "xm a b"),
        ("xnoremap", 2, "xn a b"),
        ("xunmap", 2, "xu a"),
        ("xmapclear", 5, "xmapc"),
        ("omap", 2, "om a b"),
        ("onoremap", 3, "ono a b"),
        ("ounmap", 2, "ou a"),
        ("omapclear", 5, "omapc"),
        ("imap", 2, "im a b"),
        ("inoremap", 3, "ino a b"),
        ("iunmap", 2, "iu a"),
        ("imapclear", 5, "imapc"),
        ("cmap", 2, "cm a b"),
        ("cnoremap", 3, "cno a b"),
        ("cunmap", 2, "cu a"),
        ("cmapclear", 5, "cmapc"),
        ("set", 2, "se tm=5"),
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

fn keys(script: &str) -> Vec<MapKey> {
    parse_keys(script)
        .unwrap()
        .into_iter()
        .map(MapKey::Key)
        .collect()
}

#[test]
fn mapping_commands_take_their_modes_from_the_name() {
    use MapMode::{CommandLine, Insert, Normal, OperatorPending, Visual};
    let modes = |line: &str| match parsed(line) {
        Command::Map { modes, .. }
        | Command::ShowMaps { modes, .. }
        | Command::Unmap { modes, .. }
        | Command::MapClear { modes } => modes.to_vec(),
        other => panic!("{other:?}"),
    };
    let nvo = vec![Normal, Visual, OperatorPending];
    let ic = vec![Insert, CommandLine];
    for (prefix, expected) in [
        ("", nvo.clone()),
        ("n", vec![Normal]),
        ("v", vec![Visual]),
        ("x", vec![Visual]),
        ("o", vec![OperatorPending]),
        ("i", vec![Insert]),
        ("c", vec![CommandLine]),
    ] {
        assert_eq!(modes(&format!("{prefix}map a b")), expected, "{prefix}map");
        assert_eq!(modes(&format!("{prefix}noremap a b")), expected);
        assert_eq!(modes(&format!("{prefix}unmap a")), expected);
        assert_eq!(modes(&format!("{prefix}mapclear")), expected);
    }
    // `!` on the commands without a letter: Insert and command-line mode.
    assert_eq!(modes("map! a b"), ic);
    assert_eq!(modes("noremap! a b"), ic);
    assert_eq!(modes("unmap! a"), ic);
    assert_eq!(modes("mapclear!"), ic);
    assert_eq!(modes("map a b"), nvo);
    // `noremap` and `map` differ only in whether what keys map to is mapped again.
    let noremap = |line: &str| match parsed(line) {
        Command::Map { noremap, .. } => noremap,
        other => panic!("{other:?}"),
    };
    assert!(noremap("nn a b") && noremap("no! a b") && noremap("ino a b"));
    assert!(!noremap("nm a b") && !noremap("map! a b") && !noremap("im a b"));
}

#[test]
fn mappings_read_keys_leader_and_nop() {
    assert_eq!(
        parsed("nnoremap <Leader>s :w<CR>"),
        Command::Map {
            modes: &[MapMode::Normal],
            noremap: true,
            nowait: false,
            lhs: [vec![MapKey::Leader], keys("s")].concat(),
            rhs: keys(":w<CR>"),
        }
    );
    let sides = |line: &str| match parsed(line) {
        Command::Map { lhs, rhs, .. } => (lhs, rhs),
        other => panic!("{other:?}"),
    };
    // `<Leader>` ignores case and may come anywhere, more than once.
    assert_eq!(
        sides("map <leader>a<LEADER> x<Leader>"),
        (
            [vec![MapKey::Leader], keys("a"), vec![MapKey::Leader]].concat(),
            [keys("x"), vec![MapKey::Leader]].concat()
        )
    );
    // `<lt>Leader>` is the characters, not the leader.
    assert_eq!(sides("map <lt>Leader> x").0, keys("<lt>Leader>"));
    // What keys map to is the rest of the line: spaces inside are keys, spaces at the end are
    // not, and `|` is a key as `<Bar>` is.
    assert_eq!(sides("nn Q   :ci 3<CR>   ").1, keys(":ci 3<CR>"));
    assert_eq!(sides("nn Q a|b<Bar>").1, keys("a<Bar>b<Bar>"));
    assert_eq!(sides("nn Q \"x").1, keys("\"x"));
    // `<Nop>` is no keys.
    assert_eq!(sides("nn <Space> <Nop>").1, vec![]);
    assert_eq!(sides("nn Q a<nop>b").1, keys("ab"));
    // Ctrl chords and named keys.
    assert_eq!(sides("cno <C-a> <Home>"), (keys("<C-a>"), keys("<Home>")));
    // The longest keys allowed.
    let long = "a".repeat(MAX_MAP_KEYS);
    assert_eq!(sides(&format!("map {long} {long}")).1.len(), MAX_MAP_KEYS);
}

#[test]
fn map_arguments_come_before_the_keys() {
    let nowait = |line: &str| match parsed(line) {
        Command::Map { nowait, lhs, .. } => (nowait, lhs),
        other => panic!("{other:?}"),
    };
    assert_eq!(nowait("nn <nowait> , x"), (true, keys(",")));
    assert_eq!(nowait("nn <silent><nowait> , x"), (true, keys(",")));
    assert_eq!(nowait("nn <nowait> <silent> , x"), (true, keys(",")));
    assert_eq!(nowait("nn <silent> , x"), (false, keys(",")));
    assert_eq!(nowait("nn <special> , x"), (false, keys(",")));
    // Spelled exactly, as in Vim: `<Silent>` is keys.
    assert_eq!(nowait("nn <Silent> x"), (false, keys("<Silent>")));
}

#[test]
fn keys_alone_show_mappings_and_unmap_takes_keys() {
    assert_eq!(
        parsed("nmap j"),
        Command::ShowMaps {
            modes: &[MapMode::Normal],
            lhs: keys("j"),
        }
    );
    assert_eq!(
        parsed("map"),
        Command::ShowMaps {
            modes: &[MapMode::Normal, MapMode::Visual, MapMode::OperatorPending],
            lhs: vec![],
        }
    );
    assert_eq!(
        parsed("iunmap <silent> jk  "),
        Command::Unmap {
            modes: &[MapMode::Insert],
            lhs: keys("jk"),
        }
    );
    assert_eq!(
        parsed("unmap <Leader>"),
        Command::Unmap {
            modes: &[MapMode::Normal, MapMode::Visual, MapMode::OperatorPending],
            lhs: vec![MapKey::Leader],
        }
    );
}

#[test]
fn set_takes_options() {
    use OptionName::{Escape as EscapeOption, Leader, TimeoutLen};
    let set = |line: &str| match parsed(line) {
        Command::Set(args) => args,
        other => panic!("{other:?}"),
    };
    assert_eq!(set("set"), vec![]);
    assert_eq!(
        set("se escape=block leader=<Space> tm=500"),
        vec![
            SetArg::Assign(OptionValue::Escape(Escape::Block)),
            SetArg::Assign(OptionValue::Leader(Key::char(' '))),
            SetArg::Assign(OptionValue::TimeoutLen(500)),
        ]
    );
    assert_eq!(
        set("set escape:board leader=, timeoutlen=0"),
        vec![
            SetArg::Assign(OptionValue::Escape(Escape::Board)),
            SetArg::Assign(OptionValue::Leader(Key::char(','))),
            SetArg::Assign(OptionValue::TimeoutLen(0)),
        ]
    );
    assert_eq!(
        set("set leader=\\"),
        vec![SetArg::Assign(OptionValue::Leader(Key::char('\\')))]
    );
    assert_eq!(
        set(&format!("set tm={MAX_TIMEOUTLEN}")),
        vec![SetArg::Assign(OptionValue::TimeoutLen(MAX_TIMEOUTLEN))]
    );
    assert_eq!(
        set("set timeoutlen? leader escape& \" a comment"),
        vec![
            SetArg::Show(TimeoutLen),
            SetArg::Show(Leader),
            SetArg::Reset(EscapeOption),
        ]
    );
}

#[test]
fn mapping_errors_say_what_is_wrong() {
    let cases = [
        (
            "nn <C-Nope> x",
            "in `<C-Nope>`: `<C-Nope>` at byte 0 is not a key: modifiers need a key name and `>`, as in `<C-r>`",
        ),
        (
            "nn <Leader><F25> x",
            "in `<Leader><F25>`: `<F25>` at byte 8 is not a key: function keys go from <F1> to <F24>",
        ),
        (
            "nn x a<C-",
            "in `a<C-`: `<C-` at byte 1 is not a key: modifiers need a key name and `>`, as in `<C-r>`",
        ),
        (
            "nnoremap <expr> j gj",
            "`<expr>` is not supported in lawh mappings",
        ),
        (
            "nn <buffer> j gj",
            "`<buffer>` is not supported in lawh mappings",
        ),
        (
            "nn <unique> j gj",
            "`<unique>` is not supported in lawh mappings",
        ),
        (
            "nn x <Cmd>w<CR>",
            "`<Cmd>` is not supported in lawh mappings",
        ),
        (
            "nn x <plug>(foo)",
            "`<plug>` is not supported in lawh mappings",
        ),
        (
            "nn <LocalLeader>x y",
            "`<LocalLeader>` is not supported in lawh mappings",
        ),
        ("nn <Nop> x", "missing the keys for `:nnoremap`"),
        ("unmap", "missing the keys for `:unmap`"),
        ("iu <Nop>", "missing the keys for `:iunmap`"),
        ("unmap a b", "`:unmap` takes only the keys to unmap"),
        ("mapclear x", "`:mapclear` takes no arguments"),
        ("nmap! a b", "`:nmap` takes no `!`"),
        ("inoremap! a b", "`:inoremap` takes no `!`"),
        ("nunmap! a", "`:nunmap` takes no `!`"),
        ("cmapclear!", "`:cmapclear` takes no `!`"),
        ("set! tm=1", "`:set` takes no `!`"),
        ("ma", "missing the formula: `:ma[th] <formula>`"),
        ("nu a", "unknown command `:nu`"),
        ("cn a b", "unknown command `:cn`"),
        ("lmap a b", "unknown command `:lmap`"),
        (
            "nnoremap<silent> a b",
            "unknown command `:nnoremap<silent>`",
        ),
    ];
    assert_messages(&cases);
    let too_long = "a".repeat(MAX_MAP_KEYS + 1);
    assert_eq!(
        parse_command(&format!("map {too_long} x")),
        Err(CommandError::TooManyKeys("the keys to map"))
    );
    assert_eq!(
        parse_command(&format!("map x {too_long}")),
        Err(CommandError::TooManyKeys("what they map to"))
    );
    assert_eq!(
        parse_command(&format!("unmap {too_long}")),
        Err(CommandError::TooManyKeys("the keys to unmap"))
    );
    assert_eq!(
        CommandError::TooManyKeys("what they map to").to_string(),
        format!("what they map to may be at most {MAX_MAP_KEYS} keys long")
    );
}

#[test]
fn set_errors_say_what_is_wrong() {
    let cases = [
        ("set foo", "unknown option `foo`"),
        ("set nowrap", "unknown option `nowrap`"),
        ("set =3", "unknown option `=3`"),
        (
            "set tm+=100",
            "`tm+=100`: use `option=value`, `option?` or `option&`",
        ),
        (
            "set escape=blok",
            "bad value `blok` for `escape`: it takes `board` or `block`",
        ),
        (
            "set escape=",
            "bad value `` for `escape`: it takes `board` or `block`",
        ),
        (
            "set leader=ab",
            "bad value `ab` for `leader`: it takes one key, such as `<Space>` or `,`",
        ),
        (
            "set leader=",
            "bad value `` for `leader`: it takes one key, such as `<Space>` or `,`",
        ),
        (
            "set leader=<C-Nope>",
            "bad value `<C-Nope>` for `leader`: it takes one key, such as `<Space>` or `,`",
        ),
        (
            "set tm=-1",
            "bad value `-1` for `timeoutlen`: it takes milliseconds from 0 to 10000",
        ),
        (
            "set tm=1.5",
            "bad value `1.5` for `timeoutlen`: it takes milliseconds from 0 to 10000",
        ),
        (
            "set timeoutlen=10001",
            "bad value `10001` for `timeoutlen`: it takes milliseconds from 0 to 10000",
        ),
        (
            "set tm=99999999999999999999999",
            "bad value `99999999999999999999999` for `timeoutlen`: it takes milliseconds from 0 to 10000",
        ),
    ];
    assert_messages(&cases);
}

/// Each line fails to parse with its message.
fn assert_messages(cases: &[(&str, &str)]) {
    for (line, message) in cases {
        assert_eq!(
            parse_command(line).map_err(|e| e.to_string()),
            Err((*message).to_owned()),
            "{line:?}"
        );
    }
}
