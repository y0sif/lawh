//! Errors from reading a `.lawh` file.

use super::{MAX_COMMENT_DEPTH, MAX_FILE_BYTES};
use crate::board::{MAX_COORDINATE, MAX_OBJECTS, MAX_SIZE, MAX_TEXT_CHARS};

/// Why a `.lawh` file could not be read.
///
/// Every variant carries a 1-based `line` and `column` (counted in characters) pointing into the
/// file, and its message starts with `line:column:`. [`FormatError::FileTooLarge`] is about the
/// whole file, so it reports `1:1`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FormatError {
    /// The file is not valid KDL.
    #[error("{line}:{column}: {message}{}", help_suffix(help.as_deref()))]
    Syntax {
        line: usize,
        column: usize,
        message: String,
        help: Option<String>,
    },

    /// The first node is not the `lawh version=1` header.
    #[error("{line}:{column}: a lawh file must start with `lawh version=1`")]
    MissingHeader { line: usize, column: usize },

    /// The header names a format version this build does not know.
    #[error(
        "{line}:{column}: this file was made by a newer lawh (format version {version}); \
         this lawh reads version 1"
    )]
    NewerVersion {
        line: usize,
        column: usize,
        version: i128,
    },

    /// The header's version is not a positive integer.
    #[error("{line}:{column}: invalid format version {found}; expected `version=1`")]
    InvalidVersion {
        line: usize,
        column: usize,
        found: String,
    },

    /// A node name that is not an object kind.
    #[error(
        "{line}:{column}: unknown object kind `{kind}`; \
         expected text, math, circle, ellipse, rect, line or arrow"
    )]
    UnknownKind {
        line: usize,
        column: usize,
        kind: String,
    },

    /// A property this kind of object does not have.
    #[error("{line}:{column}: `{kind}` has no property `{property}`")]
    UnknownProperty {
        line: usize,
        column: usize,
        kind: String,
        property: String,
    },

    /// The same property given twice on one node.
    #[error("{line}:{column}: property `{property}` is given twice")]
    DuplicateProperty {
        line: usize,
        column: usize,
        property: String,
    },

    /// A required property is absent.
    #[error("{line}:{column}: `{kind}` is missing property `{property}`")]
    MissingProperty {
        line: usize,
        column: usize,
        kind: String,
        property: &'static str,
    },

    /// A property value of the wrong type, or a number that is not finite.
    #[error("{line}:{column}: property `{property}` must be {expected}")]
    WrongType {
        line: usize,
        column: usize,
        property: String,
        expected: &'static str,
    },

    /// An id with characters other than ASCII letters, digits, `_` and `-`, or an empty one.
    #[error(
        "{line}:{column}: invalid object id {id:?}: use only ASCII letters, digits, `_` and `-`"
    )]
    InvalidId {
        line: usize,
        column: usize,
        id: String,
    },

    /// Two objects with the same id.
    #[error("{line}:{column}: object id `{id}` is already used earlier in the file")]
    DuplicateId {
        line: usize,
        column: usize,
        id: String,
    },

    /// A node with a children block `{ ... }`.
    #[error("{line}:{column}: `{kind}` cannot have a children block `{{ }}`")]
    Children {
        line: usize,
        column: usize,
        kind: String,
    },

    /// A `text` or `math` node without its quoted string.
    #[error(
        "{line}:{column}: `{kind}` needs its content as a quoted string, like `{kind} id=… x=0 y=0 \"…\"`"
    )]
    MissingText {
        line: usize,
        column: usize,
        kind: String,
    },

    /// The content of a `text` or `math` node is not a string.
    #[error("{line}:{column}: the content of `{kind}` must be a quoted string")]
    TextNotString {
        line: usize,
        column: usize,
        kind: String,
    },

    /// A positional argument the node does not take.
    #[error("{line}:{column}: unexpected argument: {}", takes(kind))]
    ExtraArgument {
        line: usize,
        column: usize,
        kind: String,
    },

    /// A KDL type annotation such as `(u8)`, which lawh files do not use.
    #[error("{line}:{column}: type annotations like `(name)` are not supported")]
    TypeAnnotation { line: usize, column: usize },

    /// The file is longer than [`MAX_FILE_BYTES`]. Checked before anything is parsed.
    #[error("1:1: the file is {bytes} bytes; lawh reads files of up to {MAX_FILE_BYTES} bytes")]
    FileTooLarge { bytes: usize },

    /// Block comments `/* */` nested more than [`MAX_COMMENT_DEPTH`] deep.
    #[error("{line}:{column}: block comments are nested more than {MAX_COMMENT_DEPTH} deep")]
    CommentTooDeep { line: usize, column: usize },

    /// A coordinate outside `-MAX_COORDINATE..=MAX_COORDINATE`.
    #[error(
        "{line}:{column}: property `{property}` must be between -{max} and {max}",
        max = MAX_COORDINATE
    )]
    CoordinateOutOfRange {
        line: usize,
        column: usize,
        property: String,
    },

    /// A radius, width or height that is not above zero, or above [`MAX_SIZE`].
    #[error(
        "{line}:{column}: property `{property}` must be above 0 and at most {max}",
        max = MAX_SIZE
    )]
    InvalidSize {
        line: usize,
        column: usize,
        property: String,
    },

    /// The content of a `text` or `math` node is longer than [`MAX_TEXT_CHARS`] characters.
    #[error("{line}:{column}: the content of `{kind}` is longer than {MAX_TEXT_CHARS} characters")]
    TextTooLong {
        line: usize,
        column: usize,
        kind: String,
    },

    /// More than [`MAX_OBJECTS`] objects. Points at the first object over the limit.
    #[error("{line}:{column}: too many objects; a board holds at most {MAX_OBJECTS}")]
    TooManyObjects { line: usize, column: usize },
}

impl FormatError {
    /// 1-based line of the problem.
    pub fn line(&self) -> usize {
        self.position().0
    }

    /// 1-based column of the problem, in characters.
    pub fn column(&self) -> usize {
        self.position().1
    }

    fn position(&self) -> (usize, usize) {
        match self {
            Self::Syntax { line, column, .. }
            | Self::MissingHeader { line, column }
            | Self::NewerVersion { line, column, .. }
            | Self::InvalidVersion { line, column, .. }
            | Self::UnknownKind { line, column, .. }
            | Self::UnknownProperty { line, column, .. }
            | Self::DuplicateProperty { line, column, .. }
            | Self::MissingProperty { line, column, .. }
            | Self::WrongType { line, column, .. }
            | Self::InvalidId { line, column, .. }
            | Self::DuplicateId { line, column, .. }
            | Self::Children { line, column, .. }
            | Self::MissingText { line, column, .. }
            | Self::TextNotString { line, column, .. }
            | Self::ExtraArgument { line, column, .. }
            | Self::TypeAnnotation { line, column }
            | Self::CommentTooDeep { line, column }
            | Self::TooManyObjects { line, column }
            | Self::CoordinateOutOfRange { line, column, .. }
            | Self::InvalidSize { line, column, .. }
            | Self::TextTooLong { line, column, .. } => (*line, *column),
            Self::FileTooLarge { .. } => (1, 1),
        }
    }
}

fn help_suffix(help: Option<&str>) -> String {
    help.map(|h| format!(" ({h})")).unwrap_or_default()
}

fn takes(kind: &str) -> String {
    match kind {
        "text" | "math" => format!("`{kind}` takes exactly one quoted string"),
        _ => format!("`{kind}` takes only `key=value` properties"),
    }
}
