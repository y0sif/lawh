//! Every colour lawh draws with comes from the `Palette` (`src/palette.rs`), so a theme restyles
//! the window and every export alike: no colour is made anywhere else in lawh-render's sources or
//! the app's (`crates/lawh/src`). Test code is exempt: `#[cfg(test)]` modules (from their
//! `#[cfg(test)] mod name {` line to the end of the file, where they all are) and files only
//! tests build (`#[cfg(test)] mod name;`, such as the calibrations).
//!
//! What counts as making a colour, outside comment lines:
//! - a colour constructor given a literal: `from_rgb8(`, `from_rgba8(`, `from_rgb_f32(`,
//!   `from_rgba_f32(`, `Color::new(` (`AlphaColor`, `OpaqueColor`, `PremulColor`, krilla's
//!   `rgb::Color` alike, with a colour space named too: `AlphaColor::<Srgb>::new(`) or
//!   `Paint::Rgba(` followed by a number or `[` and a number, and an
//!   `Rgba8 { r: ` or `PremulRgba8 { r: ` literal with a number. Passing on a palette colour's
//!   bytes, as `rgb::Color::new(r, g, b)` does, is no new colour;
//! - a named colour: `css::`, or `Color::`/`AlphaColor::`/... `BLACK`, `WHITE`, `TRANSPARENT`;
//! - a colour derived from another: `.with_alpha(`;
//! - a hex colour in a string: `"#` and 3, 4, 6 or 8 hex digits, then `"` or `\`;
//! - colour bytes: `[u8; 3] = [` or `[u8; 4] = [` followed by a number.
//!
//! Simple on purpose: it reads the files as text, so it can miss a colour made in a way none of
//! these spells (it checks that it finds every colour in `palette.rs`), and a new way of making
//! colours belongs in the list above.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

use std::fs;
use std::path::{Path, PathBuf};

/// Calls and literals that make a colour when what follows them is a number.
const MAKERS_WITH_LITERALS: &[&str] = &[
    "from_rgb8(",
    "from_rgba8(",
    "from_rgb_f32(",
    "from_rgba_f32(",
    "Color::new(",
    "Paint::Rgba(",
    "[u8; 3] = ",
    "[u8; 4] = ",
];

/// Struct literals of colour bytes, made when their `r:` is a number.
const STRUCT_MAKERS: &[&str] = &["Rgba8 {"];

/// Colours by name, or derived from another, whatever follows.
const ALWAYS: &[&str] = &[
    "css::",
    "Color::BLACK",
    "Color::WHITE",
    "Color::TRANSPARENT",
    ".with_alpha(",
];

/// Every `.rs` file under `dir`, sorted.
fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// The modules `source` (the file at `path`) declares test-only (`#[cfg(test)] mod name;`), as
/// the files they are in.
fn test_only_files(path: &Path, source: &str) -> Vec<PathBuf> {
    let dir = path.parent().unwrap();
    let stem = path.file_stem().unwrap().to_str().unwrap();
    // `lib.rs`, `main.rs` and `mod.rs` keep their modules beside them; `x.rs` in `x/`.
    let base = if matches!(stem, "lib" | "main" | "mod") {
        dir.to_path_buf()
    } else {
        dir.join(stem)
    };
    let lines: Vec<&str> = source.lines().map(str::trim).collect();
    let mut found = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if *line != "#[cfg(test)]" {
            continue;
        }
        let Some(next) = lines[i + 1..].iter().find(|line| !line.is_empty()) else {
            continue;
        };
        let declared = next
            .trim_start_matches("pub(crate) ")
            .trim_start_matches("pub ")
            .strip_prefix("mod ")
            .and_then(|rest| rest.strip_suffix(';'));
        if let Some(name) = declared {
            found.push(base.join(format!("{name}.rs")));
            found.push(base.join(name).join("mod.rs"));
        }
    }
    found
}

/// `source` without its test module (from `#[cfg(test)]` before `mod name {` to the end) and
/// with comment lines blanked, the line count kept.
fn code(source: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let mut end = lines.len();
    for (i, line) in lines.iter().enumerate() {
        if line.trim() != "#[cfg(test)]" {
            continue;
        }
        let next = lines[i + 1..]
            .iter()
            .map(|line| line.trim())
            .find(|line| !line.is_empty());
        if next.is_some_and(|next| {
            next.trim_start_matches("pub(crate) ").starts_with("mod ") && next.ends_with('{')
        }) {
            end = i;
            break;
        }
    }
    lines[..end]
        .iter()
        .map(|line| {
            if line.trim_start().starts_with("//") {
                ""
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether `rest` starts, past white space and opening brackets, with a number.
fn starts_with_number(rest: &str) -> bool {
    let rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '[' || c == '(');
    let rest = rest.strip_prefix('-').unwrap_or(rest);
    let rest = rest.strip_prefix('.').unwrap_or(rest);
    rest.starts_with(|c: char| c.is_ascii_digit())
}

/// Where `code` makes a colour: the line number and the line, for each.
fn colours_made(code: &str) -> Vec<(usize, String)> {
    let line_of = |at: usize| code[..at].matches('\n').count() + 1;
    let mut found = Vec::new();
    let mut push = |at: usize| {
        let line = line_of(at);
        let text = code.lines().nth(line - 1).unwrap_or("").trim().to_owned();
        if !found.iter().any(|(l, _)| *l == line) {
            found.push((line, text));
        }
    };
    for maker in MAKERS_WITH_LITERALS {
        for (at, _) in code.match_indices(maker) {
            if starts_with_number(&code[at + maker.len()..]) {
                push(at);
            }
        }
    }
    // With the colour space named: `AlphaColor::<Srgb>::new(`.
    for (at, _) in code.match_indices("Color::<") {
        let rest = &code[at..];
        let Some(close) = rest.find(">::") else {
            continue;
        };
        let after = &rest[close + 3..];
        let call = [
            "new(",
            "from_rgb8(",
            "from_rgba8(",
            "from_rgb_f32(",
            "from_rgba_f32(",
        ]
        .iter()
        .find(|call| after.starts_with(**call));
        if close < 40
            && let Some(call) = call
            && starts_with_number(&after[call.len()..])
        {
            push(at);
        }
    }
    for maker in STRUCT_MAKERS {
        for (at, _) in code.match_indices(maker) {
            let rest = code[at + maker.len()..].trim_start();
            if let Some(value) = rest.strip_prefix("r:")
                && starts_with_number(value)
            {
                push(at);
            }
        }
    }
    for name in ALWAYS {
        for (at, _) in code.match_indices(name) {
            push(at);
        }
    }
    for (at, _) in code.match_indices("\"#") {
        let digits: String = code[at + 2..]
            .chars()
            .take_while(char::is_ascii_hexdigit)
            .collect();
        let after = code[at + 2 + digits.len()..].chars().next();
        if matches!(digits.len(), 3 | 4 | 6 | 8) && matches!(after, Some('"' | '\\')) {
            push(at);
        }
    }
    found.sort();
    found
}

#[test]
fn no_colour_is_made_outside_the_palette() {
    let render = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let app = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("lawh")
        .join("src");
    let palette = render.join("palette.rs");
    let files: Vec<PathBuf> = rust_files(&render)
        .into_iter()
        .chain(rust_files(&app))
        .collect();
    let mut exempt = vec![palette.clone()];
    for file in &files {
        exempt.extend(test_only_files(file, &fs::read_to_string(file).unwrap()));
    }
    // The calibrations and the app's test directories are the test-only files today.
    assert!(exempt.contains(&render.join("renderer").join("calibrate.rs")));
    assert!(exempt.contains(&app.join("test_dir.rs")));

    let mut scanned = 0;
    let mut outside = Vec::new();
    for file in &files {
        if exempt.contains(file) {
            continue;
        }
        scanned += 1;
        for (line, text) in colours_made(&code(&fs::read_to_string(file).unwrap())) {
            outside.push(format!("{}:{line}: {text}", file.display()));
        }
    }
    assert!(scanned >= 25, "only {scanned} files scanned");
    assert!(
        outside.is_empty(),
        "colours made outside palette.rs (add them to the Palette):\n{}",
        outside.join("\n")
    );

    // Not vacuous: it finds every colour the palette makes, one a line.
    let in_palette = colours_made(&code(&fs::read_to_string(&palette).unwrap()));
    assert!(in_palette.len() >= 15, "{in_palette:?}");
}

#[test]
fn the_guard_finds_each_way_of_making_a_colour() {
    let made = [
        "let c = AlphaColor::from_rgb8(0x1F, 0x1F, 0x1F);",
        "let c = AlphaColor::from_rgba8(\n    12, 0, 0, 255,\n);",
        "let c = AlphaColor::<Srgb>::new([0.5, 0.5, 0.5, 1.0]);",
        "let c = OpaqueColor::<Srgb>::from_rgb_f32(.5, 0.0, 0.0);",
        "let c = rgb::Color::new(255, 0, 0);",
        "let p = Paint::Rgba([255, 0, 0, 255]);",
        "const RED: [u8; 4] = [0xC0, 0x39, 0x2B, 0xFF];",
        "let px = PremulRgba8 {\n    r: 0,\n    g: 0,\n    b: 0,\n    a: 0,\n};",
        "let c = css::RED;",
        "let c = AlphaColor::TRANSPARENT;",
        "let c = palette.ink.with_alpha(0.5);",
        "write!(out, \"fill=\\\"#1f1f1f\\\"\");",
        "let s = \"#abc\";",
    ];
    for source in made {
        assert_eq!(colours_made(source).len(), 1, "{source}");
    }
    let fine = [
        // Passing on a palette colour's bytes, and colour types.
        "let [r, g, b, a] = rgba; Fill { paint: rgb::Color::new(r, g, b).into() }",
        "fn stroke(color: AlphaColor<Srgb>) {}",
        "let paint = tools.palette.color(*paint);",
        // Comments, ids and attributes.
        "// AlphaColor::from_rgb8(0, 0, 0)",
        "xlink:href=\"#g12\"",
        "#[expect(clippy::cast_possible_truncation)]",
        "let ids = [0_u32; 4];",
    ];
    for source in fine {
        assert_eq!(colours_made(&code(source)), [], "{source}");
    }
    // Test modules are left out, and comment lines.
    let source = concat!(
        "fn a() {}\n// css::RED\n#[cfg(test)]\nmod tests {\n",
        "    const C: [u8; 4] = [0, 0, 0, 255];\n}\n",
    );
    assert_eq!(colours_made(&code(source)), []);
}
