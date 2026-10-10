//! A board is untrusted input: its math must not read files, fetch packages,
//! run plugins, read the clock or use fonts lawh doesn't bundle.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

use lawh_math::{MathEngine, MathError, MathItem, fonts};

/// Compiles `src` and returns Typst's error message, failing if it laid out.
fn compile_error(src: &str) -> String {
    match MathEngine::new().layout(src) {
        Err(MathError::Compile(message)) => message,
        other => panic!("{src:?} should not compile, got {other:?}"),
    }
}

#[test]
fn reading_a_file_is_denied() {
    let message = compile_error(r#"#read("/etc/passwd")"#);
    assert!(message.contains("access denied"), "{message}");
}

#[test]
fn reading_a_relative_file_is_denied() {
    let message = compile_error(r#"#read("main.typ")"#);
    assert!(message.contains("access denied"), "{message}");
}

#[test]
fn importing_a_package_fails_without_network() {
    let message = compile_error(r#"#import "@preview/cetz:0.3.0""#);
    assert!(message.contains("access denied"), "{message}");
}

#[test]
fn loading_an_image_file_is_denied() {
    let message = compile_error(r#"#image("x.png")"#);
    assert!(message.contains("access denied"), "{message}");
}

#[test]
fn plugin_is_not_in_the_library() {
    let message = compile_error("#plugin(bytes((0,)))");
    assert!(message.contains("unknown variable: plugin"), "{message}");
}

#[test]
fn plugin_is_not_reachable_through_std() {
    let message = compile_error("#std.plugin(bytes((0,)))");
    assert!(message.contains("does not contain `plugin`"), "{message}");
}

#[test]
fn citation_styles_cannot_be_parsed() {
    let message = compile_error(r#"#cite(<x>, style: bytes("<style/>"))"#);
    assert!(message.contains("unknown variable: cite"), "{message}");
    let message = compile_error(r#"#bibliography(bytes(""), style: bytes("<style/>"))"#);
    assert!(
        message.contains("unknown variable: bibliography"),
        "{message}"
    );
}

#[test]
fn there_is_no_clock() {
    let message = compile_error("#datetime.today().year()");
    assert!(message.contains("current date"), "{message}");
}

#[test]
fn fonts_outside_the_bundle_are_never_used() {
    // Typst either rejects a font it can't find or falls back to a bundled
    // one; it never reaches for a system font.
    for src in [
        r#"#text(font: "DejaVu Sans")[abc] x"#,
        r#"#box[#set text(font: "Arial"); abc] x"#,
    ] {
        match MathEngine::new().layout(src) {
            Ok(layout) => {
                for item in &layout.items {
                    if let MathItem::Glyphs { font, .. } = item {
                        assert!(*font < fonts().len(), "{src}");
                    }
                }
            }
            Err(MathError::Compile(message)) => assert!(message.contains("font"), "{message}"),
            Err(other) => panic!("{src}: {other:?}"),
        }
    }
}
