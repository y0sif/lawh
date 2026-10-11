//! The `.lawh` writer.
//!
//! The `kdl` crate's own writer is not used: it prints identifier-like strings bare, whole floats
//! as `3.0`, and leaves characters such as U+200F or U+0000 unescaped, which its own parser then
//! rejects. This writer prints one fixed shape per object so files diff cleanly.

use std::fmt::{self, Display, Formatter, Write as _};

use kurbo::Point;

use crate::board::{Board, Object, ObjectId, Shape};

/// Writes `board` as a `.lawh` file: the header, then one line per object in z-order.
pub fn to_lawh(board: &Board) -> String {
    Document(board).to_string()
}

struct Document<'a>(&'a Board);

impl Display for Document<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("lawh version=1\n")?;
        for object in self.0.objects() {
            write_object(f, object)?;
        }
        Ok(())
    }
}

fn write_object(f: &mut Formatter<'_>, object: &Object) -> fmt::Result {
    let id = Id(&object.id);
    match &object.shape {
        Shape::Text { at, text } => write_content(f, "text", id, *at, text),
        Shape::Math { at, src } => write_content(f, "math", id, *at, src),
        Shape::Ellipse { center, rx, ry } => {
            let (x, y) = (Num(center.x), Num(center.y));
            #[expect(
                clippy::float_cmp,
                reason = "a circle means exactly equal radii; any difference must survive as an ellipse"
            )]
            let is_circle = rx == ry;
            if is_circle {
                writeln!(f, "circle id={id} x={x} y={y} r={}", Num(*rx))
            } else {
                writeln!(
                    f,
                    "ellipse id={id} x={x} y={y} rx={} ry={}",
                    Num(*rx),
                    Num(*ry)
                )
            }
        }
        Shape::Rect { origin, w, h } => writeln!(
            f,
            "rect id={id} x={} y={} w={} h={}",
            Num(origin.x),
            Num(origin.y),
            Num(*w),
            Num(*h)
        ),
        Shape::Line { a, b } => write_segment(f, "line", id, *a, *b),
        Shape::Arrow { a, b } => write_segment(f, "arrow", id, *a, *b),
    }
}

fn write_content(f: &mut Formatter<'_>, kind: &str, id: Id<'_>, at: Point, s: &str) -> fmt::Result {
    writeln!(
        f,
        "{kind} id={id} x={} y={} {}",
        Num(at.x),
        Num(at.y),
        Quoted(s)
    )
}

fn write_segment(f: &mut Formatter<'_>, kind: &str, id: Id<'_>, a: Point, b: Point) -> fmt::Result {
    writeln!(
        f,
        "{kind} id={id} x1={} y1={} x2={} y2={}",
        Num(a.x),
        Num(a.y),
        Num(b.x),
        Num(b.y)
    )
}

/// A number as Rust prints it (`3`, `-0.5`, `0.30000000000000004`), with `-0` as `0`.
///
/// Very large and very small magnitudes use exponent form (`1e300`, `5e-324`), which still reads
/// back exactly:
/// - 1e38 and up: Rust prints them as long integers, and KDL integers past the `i128` range are a
///   parse error.
/// - Below 1e-6: Rust prints them with up to 300+ zeros after the point.
struct Num(f64);

impl Display for Num {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let v = self.0;
        if v == 0.0 {
            f.write_str("0")
        } else if v.abs() >= 1e38 || v.abs() < 1e-6 {
            write!(f, "{v:e}")
        } else {
            write!(f, "{v}")
        }
    }
}

/// An object id, bare when KDL reads it as an identifier (`c1`), quoted otherwise (`"1"`).
#[derive(Clone, Copy)]
struct Id<'a>(&'a ObjectId);

impl Display for Id<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let id = self.0.as_str();
        if is_bare_identifier(id) {
            f.write_str(id)
        } else {
            Quoted(id).fmt(f)
        }
    }
}

/// True for ids KDL v2 reads back as the same string without quotes.
///
/// Ids only use `[A-Za-z0-9_-]`, so starting with a letter or `_` rules out numbers, and the
/// keywords KDL v2 forbids as bare strings are excluded.
fn is_bare_identifier(id: &str) -> bool {
    id.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && !matches!(id, "true" | "false" | "null" | "inf" | "nan")
}

/// A KDL v2 double-quoted string.
///
/// `\\`, `"`, newline, carriage return and tab use short escapes. Every other control character
/// and the invisible or bidi formatting characters use `\u{X}`, so the file shows them plainly
/// and KDL parsers accept it (KDL forbids several of them as literal characters).
pub(super) struct Quoted<'a>(pub(super) &'a str);

impl Display for Quoted<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let s = self.0;
        f.write_char('"')?;
        let mut plain_from = 0;
        for (i, c) in s.char_indices() {
            let short = match c {
                '\\' => "\\\\",
                '"' => "\\\"",
                '\n' => "\\n",
                '\r' => "\\r",
                '\t' => "\\t",
                c if needs_unicode_escape(c) => "",
                _ => continue,
            };
            f.write_str(s.get(plain_from..i).unwrap_or_default())?;
            if short.is_empty() {
                write!(f, "\\u{{{:X}}}", u32::from(c))?;
            } else {
                f.write_str(short)?;
            }
            plain_from = i + c.len_utf8();
        }
        f.write_str(s.get(plain_from..).unwrap_or_default())?;
        f.write_char('"')
    }
}

/// Characters written as `\u{X}` escapes.
///
/// - Control characters: C0 (U+0000-U+001F), DEL and C1 (U+0080-U+009F, which includes NEL).
/// - U+00AD soft hyphen, U+061C Arabic letter mark, U+180E Mongolian vowel separator.
/// - U+200B-U+200F zero-width space and joiners, LRM, RLM.
/// - U+2028-U+202E line and paragraph separators, bidi embeddings and overrides.
/// - U+2060-U+206F word joiner, invisible operators, bidi isolates.
/// - U+FEFF byte order mark, U+FFF9-U+FFFB interlinear annotation marks.
fn needs_unicode_escape(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{00AD}'
                | '\u{061C}'
                | '\u{180E}'
                | '\u{200B}'..='\u{200F}'
                | '\u{2028}'..='\u{202E}'
                | '\u{2060}'..='\u{206F}'
                | '\u{FEFF}'
                | '\u{FFF9}'..='\u{FFFB}'
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::parse::{Parser, Value};

    fn quoted(s: &str) -> String {
        Quoted(s).to_string()
    }

    /// The first value of the first node of `src`, through lawh's own parser.
    fn lawh_value(src: &str) -> Value {
        let node = Parser::new(src).next_node().unwrap().unwrap();
        node.entries.into_iter().next().unwrap().value
    }

    #[test]
    fn numbers_use_rust_display() {
        let cases = [
            (3.0, "3"),
            (-0.5, "-0.5"),
            (0.1 + 0.2, "0.30000000000000004"),
            (0.0, "0"),
            (-0.0, "0"),
            (1e-6, "0.000001"),
            (-0.000_123, "-0.000123"),
            (9.99e-7, "9.99e-7"),
            (1e-7, "1e-7"),
            (-2.5e-10, "-2.5e-10"),
            (f64::MIN_POSITIVE, "2.2250738585072014e-308"),
            (4.9e-324, "5e-324"),
            (123_456.789, "123456.789"),
            (1e20, "100000000000000000000"),
            (1e38, "1e38"),
            (-1.5e300, "-1.5e300"),
            (f64::MAX, "1.7976931348623157e308"),
        ];
        for (v, want) in cases {
            assert_eq!(Num(v).to_string(), want, "{v:?}");
        }
    }

    /// Numbers in every form the writer uses read back as the same `f64`, through lawh's parser
    /// and kdl.
    #[test]
    fn numbers_parse_back_exactly() {
        let samples = [
            0.1 + 0.2,
            -1.0 / 3.0,
            1e-6,
            9.99e-7,
            -1e-7,
            1.234_567_890_123_456_7e-200,
            f64::MIN_POSITIVE,
            5e-324,
            -5e-324,
            1e37,
            9.999_999_999_999_998e37,
            1e38,
            -f64::MAX,
        ];
        for v in samples {
            let doc = kdl::KdlDocument::parse_v2(&format!("n {}", Num(v))).unwrap();
            let back = match doc.nodes()[0].entries()[0].value() {
                kdl::KdlValue::Float(f) => *f,
                kdl::KdlValue::Integer(i) => *i as f64,
                other => panic!("{other:?}"),
            };
            assert_eq!(back.to_bits(), v.to_bits(), "{v:e} written as {}", Num(v));
            let ours = match lawh_value(&format!("n {}", Num(v))) {
                Value::Float(f) => f,
                Value::Integer(i) => i as f64,
                other => panic!("{other:?}"),
            };
            assert_eq!(ours.to_bits(), v.to_bits(), "{v:e} written as {}", Num(v));
        }
    }

    #[test]
    fn strings_are_always_quoted() {
        assert_eq!(quoted(""), r#""""#);
        assert_eq!(quoted("r"), r#""r""#);
        assert_eq!(quoted("A = pi r^2"), r#""A = pi r^2""#);
        assert_eq!(quoted("true"), r#""true""#);
    }

    #[test]
    fn short_escapes() {
        assert_eq!(quoted("a\\b\"c\nd\re\tf"), r#""a\\b\"c\nd\re\tf""#);
    }

    #[test]
    fn unicode_escapes() {
        assert_eq!(quoted("\u{0}"), r#""\u{0}""#);
        assert_eq!(quoted("\u{1B}[0m"), r#""\u{1B}[0m""#);
        assert_eq!(quoted("\u{7F}\u{85}"), r#""\u{7F}\u{85}""#);
        assert_eq!(quoted("a\u{200F}b"), r#""a\u{200F}b""#);
        assert_eq!(quoted("\u{202E}x"), r#""\u{202E}x""#);
        assert_eq!(quoted("\u{FEFF}"), r#""\u{FEFF}""#);
        assert_eq!(quoted("\u{2028}\u{2029}"), r#""\u{2028}\u{2029}""#);
    }

    #[test]
    fn visible_unicode_is_kept() {
        assert_eq!(quoted("مساحة الدائرة"), "\"مساحة الدائرة\"");
        assert_eq!(quoted("π r² 😀"), "\"π r² 😀\"");
        assert_eq!(quoted("\u{A0}"), "\"\u{A0}\"");
    }

    #[test]
    fn every_escape_reads_back() {
        let mut escaped: Vec<char> = (0..=0x10_FFFF)
            .filter_map(char::from_u32)
            .filter(|&c| needs_unicode_escape(c) || matches!(c, '\\' | '"' | '\n' | '\r' | '\t'))
            .collect();
        escaped.sort_unstable();
        assert!(escaped.len() > 70, "{}", escaped.len());
        for c in escaped {
            let s = format!("a{c}b");
            let doc = format!("node {}", quoted(&s));
            let parsed = kdl::KdlDocument::parse_v2(&doc)
                .unwrap_or_else(|e| panic!("U+{:04X} escape does not parse: {e:?}", u32::from(c)));
            let value = parsed.nodes()[0].entries()[0].value().as_string();
            assert_eq!(value, Some(s.as_str()), "U+{:04X}", u32::from(c));
            let ours = lawh_value(&doc);
            assert!(
                matches!(&ours, Value::String(t) if t.value == s),
                "U+{:04X}: {ours:?}",
                u32::from(c)
            );
        }
    }

    /// Every BMP character, and a sample of the other planes, survives the quoting.
    #[test]
    fn unicode_round_trips_through_kdl() {
        let chars = (0..=0xFFFF).chain((0x1_0000..=0x10_FFFF).step_by(31));
        let all: String = chars.filter_map(char::from_u32).collect();
        let doc = format!("node {}", quoted(&all));
        let parsed = kdl::KdlDocument::parse_v2(&doc).unwrap();
        assert_eq!(
            parsed.nodes()[0].entries()[0].value().as_string(),
            Some(all.as_str())
        );
    }

    #[test]
    fn ids_are_bare_unless_kdl_would_misread_them() {
        let write = |s: &str| Id(&ObjectId::new(s).unwrap()).to_string();
        assert_eq!(write("c1"), "c1");
        assert_eq!(write("_x"), "_x");
        assert_eq!(write("A-b_2"), "A-b_2");
        assert_eq!(write("1"), r#""1""#);
        assert_eq!(write("1a"), r#""1a""#);
        assert_eq!(write("-1"), r#""-1""#);
        assert_eq!(write("-a"), r#""-a""#);
        assert_eq!(write("true"), r#""true""#);
        assert_eq!(write("nan"), r#""nan""#);
        assert_eq!(write("nanny"), "nanny");
    }
}
