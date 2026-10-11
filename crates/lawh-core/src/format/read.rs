//! The `.lawh` reader: checks each node the parser reads and adds its object to the board.

use kurbo::Point;

use super::parse::{End, Entry, Node, Parser, Stop, Text, Value, is_bare_word};
use super::write::Quoted;
use super::{FormatError, MAX_FILE_BYTES};
use crate::board::{
    Board, BoardError, Object, ObjectId, Shape, TextProblem, is_valid_coordinate, is_valid_size,
};

/// The format version this build reads and writes.
const VERSION: i128 = 1;

/// Reads a `.lawh` file. Accepts LF and CRLF line endings.
///
/// Reading stops at the first problem, which the error points at. Files longer than
/// [`MAX_FILE_BYTES`] are rejected before anything is read, a file with more than
/// [`MAX_OBJECTS`](crate::board::MAX_OBJECTS) objects at its first object over the limit, and
/// one with more than [`MAX_BOARD_TEXT_CHARS`](crate::board::MAX_BOARD_TEXT_CHARS) characters
/// of text at the content that goes over it.
pub fn from_lawh(src: &str) -> Result<Board, FormatError> {
    if src.len() > MAX_FILE_BYTES {
        return Err(FormatError::FileTooLarge { bytes: src.len() });
    }
    let reader = Reader { src };
    let mut parser = Parser::new(src);
    match parser.next_node() {
        Ok(Some(header)) => reader.header(&header)?,
        // A slashdashed node with children before any other node: no header either.
        Ok(None) | Err(Stop::DroppedChildren { .. }) => {
            return Err(FormatError::MissingHeader { line: 1, column: 1 });
        }
        Err(stop) => return Err(reader.stopped(stop)),
    }
    let mut board = Board::new();
    loop {
        match parser.next_node() {
            Ok(Some(node)) => {
                let object = reader.object(node, &board)?;
                board.push_checked(object);
            }
            Ok(None) => return Ok(board),
            Err(stop) => return Err(reader.stopped(stop)),
        }
    }
}

/// 1-based line and column (in characters) of byte `offset` in `src`.
///
/// LF, CRLF and a lone CR each end a line. A leading byte order mark takes no column.
pub(super) fn line_column(src: &str, offset: usize) -> (usize, usize) {
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

    /// The error for why the parser stopped.
    fn stopped(&self, stop: Stop) -> FormatError {
        match stop {
            Stop::Error(e) => e,
            Stop::DroppedChildren { brace, owner } => {
                let (line, column) = self.at(brace);
                FormatError::Children {
                    line,
                    column,
                    kind: owner,
                }
            }
            Stop::TooManyObjects { at } => {
                let (line, column) = self.at(at);
                FormatError::TooManyObjects { line, column }
            }
        }
    }

    /// Checks the `lawh version=1` header.
    fn header(&self, node: &Node) -> Result<(), FormatError> {
        if node.name.over.is_some() || node.name.value != "lawh" {
            let (line, column) = self.at(node.at);
            return Err(FormatError::MissingHeader { line, column });
        }
        // Check the version before anything else: a newer format may add properties or children.
        let version = (node.entries.iter()).find(|e| key(e) == Some("version"));
        match (version, node.end) {
            (Some(version), _) => self.version(version)?,
            // Past the entries the parser read, there may be a newer version.
            (None, End::TooManyEntries { at }) => return Err(self.too_many_entries(at, "lawh")),
            // Reported below, with the entry that passed the cap.
            (None, End::OverCap) => {}
            (None, End::Complete | End::Children { .. }) => {
                let (line, column) = self.at(node.at);
                return Err(FormatError::MissingProperty {
                    line,
                    column,
                    kind: "lawh".to_owned(),
                    property: "version",
                });
            }
        }
        self.no_annotation_or_children(node, "lawh")?;
        let mut seen_version = false;
        for entry in &node.entries {
            self.no_annotation(entry)?;
            let name = key(entry);
            if name == Some("version") && !seen_version {
                seen_version = true;
                continue;
            }
            let (line, column) = self.at(entry.at);
            return Err(match (&entry.key, name) {
                (None, _) => FormatError::ExtraArgument {
                    line,
                    column,
                    kind: "lawh".to_owned(),
                },
                (_, Some("version")) => FormatError::DuplicateProperty {
                    line,
                    column,
                    property: "version".to_owned(),
                },
                (Some(property), _) => FormatError::UnknownProperty {
                    line,
                    column,
                    kind: "lawh".to_owned(),
                    property: shown_name(property),
                },
            });
        }
        self.cut(node, "lawh")
    }

    /// Checks the header's `version` property.
    fn version(&self, entry: &Entry) -> Result<(), FormatError> {
        let (line, column) = self.at(entry.at);
        match &entry.value {
            Value::Integer(VERSION) => Ok(()),
            Value::Integer(v) if *v > VERSION => Err(FormatError::NewerVersion {
                line,
                column,
                version: *v,
            }),
            other => Err(FormatError::InvalidVersion {
                line,
                column,
                found: shown_value(other),
            }),
        }
    }

    /// Reads one object.
    fn object(&self, node: Node, board: &Board) -> Result<Object, FormatError> {
        let start = node.at;
        // The parser stops before any object past the limit; this only guards against a node it
        // did not count.
        if board.is_full() {
            let (line, column) = self.at(start);
            return Err(FormatError::TooManyObjects { line, column });
        }
        if node.annotated {
            let (line, column) = self.at(start);
            return Err(FormatError::TypeAnnotation { line, column });
        }
        let kind = (node.name.over.is_none())
            .then(|| Kind::from_name(&node.name.value))
            .flatten();
        let Some(kind) = kind else {
            let (line, column) = self.at(start);
            return Err(FormatError::UnknownKind {
                line,
                column,
                kind: shown_name(&node.name),
            });
        };
        self.no_annotation_or_children(&node, kind.name())?;
        let (end, cut) = (node.end, node.is_cut());
        let parts = self.parts(node.entries, kind, board)?;
        if cut {
            return Err(self.cut_error(end, start, kind.name()));
        }

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
    fn parts(&self, entries: Vec<Entry>, kind: Kind, board: &Board) -> Result<Parts, FormatError> {
        let mut parts = Parts::default();
        for entry in entries {
            self.no_annotation(&entry)?;
            let here = || self.at(entry.at);
            let Some(property) = &entry.key else {
                if !kind.has_content() || parts.content.is_some() {
                    let (line, column) = here();
                    return Err(FormatError::ExtraArgument {
                        line,
                        column,
                        kind: kind.name().to_owned(),
                    });
                }
                let Value::String(text) = entry.value else {
                    let (line, column) = here();
                    return Err(FormatError::TextNotString {
                        line,
                        column,
                        kind: kind.name().to_owned(),
                    });
                };
                parts.content = Some(self.content(text, entry.at, kind, board)?);
                continue;
            };
            let duplicate = || {
                let (line, column) = here();
                FormatError::DuplicateProperty {
                    line,
                    column,
                    property: property.value.clone(),
                }
            };
            let name = property.over.is_none().then_some(property.value.as_str());
            if name == Some("id") {
                if parts.id.is_some() {
                    return Err(duplicate());
                }
                let id = self.id(entry.at, &entry.value)?;
                if board.contains(&id) {
                    let (line, column) = here();
                    return Err(FormatError::DuplicateId {
                        line,
                        column,
                        id: id.as_str().to_owned(),
                    });
                }
                parts.id = Some(id);
            } else if let Some((name, slot)) =
                (kind.numbers().iter().zip(&mut parts.numbers)).find(|(n, _)| Some(**n) == name)
            {
                if slot.is_some() {
                    return Err(duplicate());
                }
                *slot = Some(self.number(entry.at, &entry.value, name)?);
            } else {
                let (line, column) = here();
                return Err(FormatError::UnknownProperty {
                    line,
                    column,
                    kind: kind.name().to_owned(),
                    property: shown_name(property),
                });
            }
        }
        Ok(parts)
    }

    /// The content of a `text` or `math` node, the entry at byte `at`, within the caps on text.
    fn content(
        &self,
        text: Text,
        at: usize,
        kind: Kind,
        board: &Board,
    ) -> Result<String, FormatError> {
        let (line, column) = if text.over.is_some() || !board.has_room_for_text(text.chars) {
            self.at(at)
        } else {
            return Ok(text.value);
        };
        let kind = kind.name().to_owned();
        Err(match text.over {
            Some(TextProblem::TooLong) => FormatError::TextTooLong { line, column, kind },
            Some(TextProblem::TooManyLines) => FormatError::TooManyLines { line, column, kind },
            None => FormatError::TooMuchText { line, column },
        })
    }

    /// The `id` property, the entry at byte `at`.
    fn id(&self, at: usize, value: &Value) -> Result<ObjectId, FormatError> {
        let Value::String(text) = value else {
            let (line, column) = self.at(at);
            return Err(FormatError::WrongType {
                line,
                column,
                property: "id".to_owned(),
                expected: "a string such as `c1`",
            });
        };
        ObjectId::new(text.value.as_str()).map_err(|e| {
            let (line, column) = self.at(at);
            match e {
                BoardError::IdTooLong(_) => FormatError::IdTooLong { line, column },
                _ => FormatError::InvalidId {
                    line,
                    column,
                    id: text.value.clone(),
                },
            }
        })
    }

    /// A finite number within the board's limits, the entry at byte `at`: a size for `r`, `rx`,
    /// `ry`, `w` and `h`, a coordinate otherwise.
    fn number(&self, at: usize, value: &Value, prop: &str) -> Result<f64, FormatError> {
        let v = match value {
            Value::Integer(i) => *i as f64,
            Value::Float(f) => *f,
            Value::String(_) | Value::Bool(_) | Value::Null => f64::NAN,
        };
        let is_size = matches!(prop, "r" | "rx" | "ry" | "w" | "h");
        let in_range = if is_size {
            is_valid_size(v)
        } else {
            is_valid_coordinate(v)
        };
        if in_range {
            return Ok(v);
        }
        // Only now find the position: it scans from the file start.
        let (line, column) = self.at(at);
        let property = prop.to_owned();
        Err(match value {
            Value::Integer(_) | Value::Float(_) if v.is_finite() => {
                if is_size {
                    FormatError::InvalidSize {
                        line,
                        column,
                        property,
                    }
                } else {
                    FormatError::CoordinateOutOfRange {
                        line,
                        column,
                        property,
                    }
                }
            }
            Value::Integer(_) | Value::Float(_) => FormatError::WrongType {
                line,
                column,
                property,
                expected: "a finite number",
            },
            Value::String(_) | Value::Bool(_) | Value::Null => FormatError::WrongType {
                line,
                column,
                property,
                expected: "a number",
            },
        })
    }

    fn no_annotation(&self, entry: &Entry) -> Result<(), FormatError> {
        if entry.annotated {
            let (line, column) = self.at(entry.at);
            return Err(FormatError::TypeAnnotation { line, column });
        }
        Ok(())
    }

    /// Rejects a type annotation on the node, then a children block.
    fn no_annotation_or_children(&self, node: &Node, kind: &str) -> Result<(), FormatError> {
        if node.annotated {
            let (line, column) = self.at(node.at);
            return Err(FormatError::TypeAnnotation { line, column });
        }
        if let End::Children { brace } = node.end {
            let (line, column) = self.at(brace);
            return Err(FormatError::Children {
                line,
                column,
                kind: kind.to_owned(),
            });
        }
        Ok(())
    }

    /// Fails if the parser stopped reading the node at a cap.
    fn cut(&self, node: &Node, kind: &str) -> Result<(), FormatError> {
        match node.end {
            End::Complete | End::Children { .. } => Ok(()),
            end => Err(self.cut_error(end, node.at, kind)),
        }
    }

    /// The error for a node the parser stopped reading at a cap, `end`. The entry that passed a
    /// cap on text has always been reported before this, as an unknown property, an extra
    /// argument, a text too long or the like; this only guards against one that was not.
    fn cut_error(&self, end: End, start: usize, kind: &str) -> FormatError {
        if let End::TooManyEntries { at } = end {
            return self.too_many_entries(at, kind);
        }
        let (line, column) = self.at(start);
        FormatError::TextTooLong {
            line,
            column,
            kind: kind.to_owned(),
        }
    }

    fn too_many_entries(&self, at: usize, kind: &str) -> FormatError {
        let (line, column) = self.at(at);
        FormatError::TooManyEntries {
            line,
            column,
            kind: kind.to_owned(),
        }
    }
}

/// The key of a property, unless it passed a cap.
fn key(entry: &Entry) -> Option<&str> {
    (entry.key.as_ref())
        .filter(|k| k.over.is_none())
        .map(|k| k.value.as_str())
}

/// A name for messages: as read, with `…` after one cut at a cap.
fn shown_name(text: &Text) -> String {
    match text.over {
        None => text.value.clone(),
        Some(_) => format!("{}…", text.value),
    }
}

/// A value for messages, as KDL writes it: `#true`, `1.0`, `c1`, `"1"`.
fn shown_value(value: &Value) -> String {
    match value {
        Value::String(text) if text.over.is_none() && is_bare_word(&text.value) => {
            text.value.clone()
        }
        Value::String(text) => {
            let cut = if text.over.is_some() { "…" } else { "" };
            format!("{}{cut}", Quoted(&text.value))
        }
        Value::Integer(i) => i.to_string(),
        Value::Float(f) if f.is_nan() => "#nan".to_owned(),
        Value::Float(f) if *f == f64::INFINITY => "#inf".to_owned(),
        Value::Float(f) if *f == f64::NEG_INFINITY => "#-inf".to_owned(),
        Value::Float(f) => format!("{f:?}"),
        Value::Bool(b) => format!("#{b}"),
        Value::Null => "#null".to_owned(),
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
