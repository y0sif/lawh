//! The `.lawh` reader, on top of the `kdl` crate's v2 parser.

use std::collections::HashSet;

use kdl::{KdlDocument, KdlEntry, KdlError, KdlNode, KdlValue};
use kurbo::Point;

use super::FormatError;
use crate::board::{Board, Object, ObjectId, Shape};

/// The format version this build reads and writes.
const VERSION: i128 = 1;

/// Reads a `.lawh` file. Accepts LF and CRLF line endings.
pub fn from_lawh(src: &str) -> Result<Board, FormatError> {
    let doc = KdlDocument::parse_v2(src).map_err(|e| syntax_error(src, &e))?;
    let reader = Reader { src };
    let mut nodes = doc.nodes().iter();
    let Some(header) = nodes.next() else {
        return Err(FormatError::MissingHeader { line: 1, column: 1 });
    };
    reader.header(header)?;
    let mut board = Board::new();
    for node in nodes {
        let object = reader.object(node, &board)?;
        board.push_checked(object);
    }
    Ok(board)
}

fn syntax_error(src: &str, error: &KdlError) -> FormatError {
    // kdl reports a raw bidi or control character as "Expected quoted string" at the opening
    // quote; name the character instead.
    if let Some((offset, c)) = src
        .char_indices()
        .find(|&(i, c)| is_forbidden_literal(c) && !(i == 0 && c == '\u{FEFF}'))
    {
        let (line, column) = line_column(src, offset);
        let code = u32::from(c);
        return FormatError::Syntax {
            line,
            column,
            message: format!("character U+{code:04X} cannot appear as itself in a lawh file"),
            help: Some(format!("inside a string, write it as \\u{{{code:X}}}")),
        };
    }
    let first = error.diagnostics.first();
    let (line, column) = first.map_or((1, 1), |d| {
        let (start, len) = (d.span.offset(), d.span.len());
        // Some kdl errors carry a span from the start of the document to the problem.
        let multi_line = src
            .get(start..start + len)
            .is_some_and(|s| s.contains('\n'));
        line_column(src, if start == 0 && multi_line { len } else { start })
    });
    FormatError::Syntax {
        line,
        column,
        message: first
            .and_then(|d| d.message.clone())
            .unwrap_or_else(|| "invalid KDL".to_owned()),
        help: first.and_then(|d| d.help.clone()),
    }
}

/// Code points KDL v2 forbids anywhere in a document, even inside strings.
fn is_forbidden_literal(c: char) -> bool {
    matches!(
        c,
        '\u{0}'..='\u{8}'
            | '\u{E}'..='\u{1F}'
            | '\u{7F}'
            | '\u{200E}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
    )
}

/// 1-based line and column (in characters) of byte `offset` in `src`.
///
/// LF, CRLF and a lone CR each end a line. A leading byte order mark takes no column.
fn line_column(src: &str, offset: usize) -> (usize, usize) {
    let (mut line, mut column) = (1, 1);
    let mut after_cr = false;
    for (i, c) in src.char_indices() {
        if i >= offset {
            break;
        }
        match c {
            '\n' if after_cr => {}
            '\n' | '\r' => {
                line += 1;
                column = 1;
            }
            '\u{FEFF}' if i == 0 => {}
            _ => column += 1,
        }
        after_cr = c == '\r';
    }
    (line, column)
}

/// The object kinds, with their numeric properties in file order.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Text,
    Math,
    Circle,
    Ellipse,
    Rect,
    Line,
    Arrow,
}

impl Kind {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "text" => Self::Text,
            "math" => Self::Math,
            "circle" => Self::Circle,
            "ellipse" => Self::Ellipse,
            "rect" => Self::Rect,
            "line" => Self::Line,
            "arrow" => Self::Arrow,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Math => "math",
            Self::Circle => "circle",
            Self::Ellipse => "ellipse",
            Self::Rect => "rect",
            Self::Line => "line",
            Self::Arrow => "arrow",
        }
    }

    /// Numeric properties, in the order the writer prints them.
    fn numbers(self) -> &'static [&'static str] {
        match self {
            Self::Text | Self::Math => &["x", "y"],
            Self::Circle => &["x", "y", "r"],
            Self::Ellipse => &["x", "y", "rx", "ry"],
            Self::Rect => &["x", "y", "w", "h"],
            Self::Line | Self::Arrow => &["x1", "y1", "x2", "y2"],
        }
    }

    fn has_content(self) -> bool {
        matches!(self, Self::Text | Self::Math)
    }
}

/// The most numeric properties any kind has.
const MAX_NUMBERS: usize = 4;

/// What one object node holds, before checking that nothing is missing.
#[derive(Default)]
struct Parts {
    id: Option<ObjectId>,
    /// In [`Kind::numbers`] order.
    numbers: [Option<f64>; MAX_NUMBERS],
    content: Option<String>,
}

struct Reader<'a> {
    src: &'a str,
}

impl Reader<'_> {
    /// Line and column of a byte offset. Only called on errors: it scans from the file start.
    fn at(&self, offset: usize) -> (usize, usize) {
        line_column(self.src, offset)
    }

    fn header(&self, node: &KdlNode) -> Result<(), FormatError> {
        let start = node.span().offset();
        if node.name().value() != "lawh" {
            let (line, column) = self.at(start);
            return Err(FormatError::MissingHeader { line, column });
        }
        // Check the version before anything else: a newer format may add properties or children.
        let version = node
            .entries()
            .iter()
            .find(|e| prop_name(e) == Some("version"));
        let Some(version) = version else {
            let (line, column) = self.at(start);
            return Err(FormatError::MissingProperty {
                line,
                column,
                kind: "lawh".to_owned(),
                property: "version",
            });
        };
        match version.value() {
            KdlValue::Integer(VERSION) => {}
            KdlValue::Integer(v) if *v > VERSION => {
                let (line, column) = self.at(version.span().offset());
                return Err(FormatError::NewerVersion {
                    line,
                    column,
                    version: *v,
                });
            }
            other => {
                let (line, column) = self.at(version.span().offset());
                return Err(FormatError::InvalidVersion {
                    line,
                    column,
                    found: other.to_string(),
                });
            }
        }
        self.no_annotation_or_children(node, "lawh")?;
        let mut seen_version = false;
        for entry in node.entries() {
            self.no_annotation(entry)?;
            let name = prop_name(entry);
            if name == Some("version") && !seen_version {
                seen_version = true;
                continue;
            }
            let (line, column) = self.at(entry.span().offset());
            return Err(match name {
                None => FormatError::ExtraArgument {
                    line,
                    column,
                    kind: "lawh".to_owned(),
                },
                Some("version") => FormatError::DuplicateProperty {
                    line,
                    column,
                    property: "version".to_owned(),
                },
                Some(name) => FormatError::UnknownProperty {
                    line,
                    column,
                    kind: "lawh".to_owned(),
                    property: name.to_owned(),
                },
            });
        }
        Ok(())
    }

    fn object(&self, node: &KdlNode, board: &Board) -> Result<Object, FormatError> {
        let start = node.span().offset();
        let name = node.name().value();
        if node.ty().is_some() {
            let (line, column) = self.at(start);
            return Err(FormatError::TypeAnnotation { line, column });
        }
        let Some(kind) = Kind::from_name(name) else {
            let (line, column) = self.at(start);
            return Err(FormatError::UnknownKind {
                line,
                column,
                kind: name.to_owned(),
            });
        };
        self.no_annotation_or_children(node, kind.name())?;
        let parts = self.parts(node, kind, board)?;

        let missing = |property: &'static str| {
            let (line, column) = self.at(start);
            FormatError::MissingProperty {
                line,
                column,
                kind: kind.name().to_owned(),
                property,
            }
        };
        let id = parts.id.ok_or_else(|| missing("id"))?;
        let mut values = [0.0; MAX_NUMBERS];
        let names = kind.numbers().iter().copied();
        for ((value, slot), name) in values.iter_mut().zip(parts.numbers).zip(names) {
            *value = slot.ok_or_else(|| missing(name))?;
        }
        let content = if kind.has_content() {
            parts.content.ok_or_else(|| {
                let (line, column) = self.at(start);
                FormatError::MissingText {
                    line,
                    column,
                    kind: kind.name().to_owned(),
                }
            })?
        } else {
            String::new()
        };
        Ok(Object::new(id, shape(kind, values, content)))
    }

    /// Collects a node's id, numbers and content, rejecting anything the kind does not take.
    fn parts(&self, node: &KdlNode, kind: Kind, board: &Board) -> Result<Parts, FormatError> {
        let mut parts = Parts::default();
        let mut seen = HashSet::new();
        for entry in node.entries() {
            self.no_annotation(entry)?;
            let here = || self.at(entry.span().offset());
            let Some(prop) = prop_name(entry) else {
                if !kind.has_content() || parts.content.is_some() {
                    let (line, column) = here();
                    return Err(FormatError::ExtraArgument {
                        line,
                        column,
                        kind: kind.name().to_owned(),
                    });
                }
                let KdlValue::String(s) = entry.value() else {
                    let (line, column) = here();
                    return Err(FormatError::TextNotString {
                        line,
                        column,
                        kind: kind.name().to_owned(),
                    });
                };
                parts.content = Some(s.clone());
                continue;
            };
            if !seen.insert(prop) {
                let (line, column) = here();
                return Err(FormatError::DuplicateProperty {
                    line,
                    column,
                    property: prop.to_owned(),
                });
            }
            if prop == "id" {
                let id = self.id(entry)?;
                if board.contains(&id) {
                    let (line, column) = here();
                    return Err(FormatError::DuplicateId {
                        line,
                        column,
                        id: id.as_str().to_owned(),
                    });
                }
                parts.id = Some(id);
            } else if let Some(slot) = (kind.numbers().iter())
                .position(|n| *n == prop)
                .and_then(|i| parts.numbers.get_mut(i))
            {
                *slot = Some(self.number(entry, prop)?);
            } else {
                let (line, column) = here();
                return Err(FormatError::UnknownProperty {
                    line,
                    column,
                    kind: kind.name().to_owned(),
                    property: prop.to_owned(),
                });
            }
        }
        Ok(parts)
    }

    fn id(&self, entry: &KdlEntry) -> Result<ObjectId, FormatError> {
        let KdlValue::String(s) = entry.value() else {
            let (line, column) = self.at(entry.span().offset());
            return Err(FormatError::WrongType {
                line,
                column,
                property: "id".to_owned(),
                expected: "a string such as `c1`",
            });
        };
        ObjectId::new(s.as_str()).map_err(|_| {
            let (line, column) = self.at(entry.span().offset());
            FormatError::InvalidId {
                line,
                column,
                id: s.clone(),
            }
        })
    }

    fn number(&self, entry: &KdlEntry, prop: &str) -> Result<f64, FormatError> {
        let expected = match entry.value() {
            KdlValue::Integer(i) => return Ok(*i as f64),
            KdlValue::Float(f) if f.is_finite() => return Ok(*f),
            KdlValue::Float(_) => "a finite number",
            KdlValue::String(_) | KdlValue::Bool(_) | KdlValue::Null => "a number",
        };
        let (line, column) = self.at(entry.span().offset());
        Err(FormatError::WrongType {
            line,
            column,
            property: prop.to_owned(),
            expected,
        })
    }

    fn no_annotation(&self, entry: &KdlEntry) -> Result<(), FormatError> {
        if entry.ty().is_some() {
            let (line, column) = self.at(entry.span().offset());
            return Err(FormatError::TypeAnnotation { line, column });
        }
        Ok(())
    }

    fn no_annotation_or_children(&self, node: &KdlNode, kind: &str) -> Result<(), FormatError> {
        let start = node.span().offset();
        if node.ty().is_some() {
            let (line, column) = self.at(start);
            return Err(FormatError::TypeAnnotation { line, column });
        }
        if let Some(children) = node.children() {
            // Point at the `{`: the last one between the node start and its first child.
            let inner = children.span().offset();
            let brace = self
                .src
                .get(start..inner)
                .and_then(|s| s.rfind('{'))
                .map_or(start, |i| start + i);
            let (line, column) = self.at(brace);
            return Err(FormatError::Children {
                line,
                column,
                kind: kind.to_owned(),
            });
        }
        Ok(())
    }
}

/// Builds the shape from its numbers, in [`Kind::numbers`] order, and its text or math content.
fn shape(kind: Kind, values: [f64; MAX_NUMBERS], content: String) -> Shape {
    let [v0, v1, v2, v3] = values;
    match kind {
        Kind::Text => Shape::Text {
            at: Point::new(v0, v1),
            text: content,
        },
        Kind::Math => Shape::Math {
            at: Point::new(v0, v1),
            src: content,
        },
        Kind::Circle => Shape::Ellipse {
            center: Point::new(v0, v1),
            rx: v2,
            ry: v2,
        },
        Kind::Ellipse => Shape::Ellipse {
            center: Point::new(v0, v1),
            rx: v2,
            ry: v3,
        },
        Kind::Rect => Shape::Rect {
            origin: Point::new(v0, v1),
            w: v2,
            h: v3,
        },
        Kind::Line => Shape::Line {
            a: Point::new(v0, v1),
            b: Point::new(v2, v3),
        },
        Kind::Arrow => Shape::Arrow {
            a: Point::new(v0, v1),
            b: Point::new(v2, v3),
        },
    }
}

fn prop_name(entry: &KdlEntry) -> Option<&str> {
    entry.name().map(kdl::KdlIdentifier::value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_column_counts_lf_crlf_and_cr() {
        let src = "ab\ncd\r\nef\rgh";
        assert_eq!(line_column(src, 0), (1, 1));
        assert_eq!(line_column(src, 1), (1, 2));
        assert_eq!(line_column(src, 3), (2, 1));
        assert_eq!(line_column(src, 4), (2, 2));
        assert_eq!(line_column(src, 7), (3, 1));
        assert_eq!(line_column(src, 10), (4, 1));
        assert_eq!(line_column(src, 99), (4, 3));
    }

    #[test]
    fn line_column_counts_characters_not_bytes() {
        let src = "\u{FEFF}é日😀x";
        let x = src.find('x').unwrap();
        assert_eq!(line_column(src, x), (1, 4));
    }

    #[test]
    fn integers_and_floats_are_both_numbers() {
        let board = from_lawh("lawh version=1\ncircle id=c1 x=0 y=-2 r=1.5\n").unwrap();
        assert_eq!(
            board.objects()[0].shape,
            Shape::Ellipse {
                center: Point::new(0.0, -2.0),
                rx: 1.5,
                ry: 1.5
            }
        );
    }

    #[test]
    fn ellipse_with_equal_radii_is_read() {
        let board = from_lawh("lawh version=1\nellipse id=e1 x=0 y=0 rx=2 ry=2\n").unwrap();
        assert_eq!(
            crate::format::to_lawh(&board),
            "lawh version=1\ncircle id=e1 x=0 y=0 r=2\n"
        );
    }

    #[test]
    fn property_order_does_not_matter() {
        let a = from_lawh("lawh version=1\nrect h=2 w=4 y=1 x=0 id=r1\n").unwrap();
        let b = from_lawh("lawh version=1\nrect id=r1 x=0 y=1 w=4 h=2\n").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn quoted_and_bare_strings_are_accepted() {
        let board = from_lawh("lawh version=1\ntext id=\"1\" x=0 y=0 hello\n").unwrap();
        assert_eq!(board.objects()[0].id.as_str(), "1");
        assert_eq!(
            board.objects()[0].shape,
            Shape::Text {
                at: Point::ZERO,
                text: "hello".to_owned()
            }
        );
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let src = "// note\nlawh version=1 // header\n\n/* block */ circle id=c1 x=0 y=0 r=1\n/-circle id=c2 x=0 y=0 r=1\n";
        let board = from_lawh(src).unwrap();
        assert_eq!(board.len(), 1);
    }

    #[test]
    fn header_only_is_an_empty_board() {
        assert_eq!(from_lawh("lawh version=1\n").unwrap(), Board::new());
        assert_eq!(from_lawh("lawh version=1").unwrap(), Board::new());
    }

    #[test]
    fn byte_order_mark_is_accepted() {
        let board = from_lawh("\u{FEFF}lawh version=1\ncircle id=c1 x=0 y=0 r=1\n").unwrap();
        assert_eq!(board.len(), 1);
    }
}
