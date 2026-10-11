//! The `.lawh` parser: one pass over the file, without recursion, that hands the reader one node
//! at a time.
//!
//! A `.lawh` file is a flat KDL v2 document. The parser follows the KDL v2 grammar as the `kdl`
//! crate (6.7) reads it, and accepts only flat documents:
//!
//! - Nodes have no children: reading stops at the first `{` outside strings and comments.
//! - Block comments nest at most [`MAX_COMMENT_DEPTH`] deep.
//! - A slashdash `/-` may not apply to another slashdash.
//!
//! It rejects a few exotic forms that kdl accepts (`/- /-`, a keyword running straight into the
//! next token) and never accepts text kdl rejects. Property tests below compare the two on random
//! documents, values included.
//!
//! Every cap is checked while reading, so the time and memory a file costs follow what a board
//! holds, not the length of the file:
//!
//! - Reading stops at the node after the header and [`MAX_OBJECTS`] objects.
//! - A node keeps at most [`MAX_ENTRIES`] entries: reading stops where the next one starts.
//! - A string, bare word or number takes at most [`MAX_STRING_BYTES`] bytes of the file.
//! - A string keeps at most [`MAX_TEXT_CHARS`](crate::board::MAX_TEXT_CHARS) characters and
//!   [`MAX_TEXT_LINES`](crate::board::MAX_TEXT_LINES) lines of decoded text. One that passes
//!   either is still checked to its end, and the node ends after it.
//! - Slashdashed nodes and entries are checked, but nothing of them is kept.
//!
//! Positions are byte offsets into the file; errors turn them into lines and columns.

use std::ops::Range;

use super::read::line_column;
use super::{FormatError, MAX_COMMENT_DEPTH, MAX_ENTRIES, MAX_STRING_BYTES};
use crate::board::{MAX_OBJECTS, TextCount, TextProblem};

/// A string from the file: quoted, raw, multi-line or a bare word.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Text {
    /// The decoded text, up to the cap it passed, if any.
    pub(super) value: String,
    /// Characters in `value`.
    pub(super) chars: usize,
    /// The cap on decoded text the string passed. The parser stops reading its node after it.
    pub(super) over: Option<TextProblem>,
}

/// What follows `=`, or an argument.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Value {
    String(Text),
    Integer(i128),
    Float(f64),
    Bool(bool),
    Null,
}

/// A property `key=value` or an argument `value`.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Entry {
    /// Byte offset of its first character: the key, the type annotation or the value.
    pub(super) at: usize,
    pub(super) key: Option<Text>,
    /// The value has a type annotation such as `(u8)`.
    pub(super) annotated: bool,
    /// [`Value::Null`] when the key passed a cap: the parser stops before the value.
    pub(super) value: Value,
}

impl Entry {
    /// True when its key or value passed a cap on decoded text.
    fn is_over(&self) -> bool {
        self.key.as_ref().is_some_and(|k| k.over.is_some())
            || matches!(&self.value, Value::String(text) if text.over.is_some())
    }
}

/// A node the file keeps: not slashdashed.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Node {
    /// Byte offset of its first character: the type annotation or the name.
    pub(super) at: usize,
    /// The node has a type annotation such as `(shape)`.
    pub(super) annotated: bool,
    pub(super) name: Text,
    /// At most [`MAX_ENTRIES`], in file order.
    pub(super) entries: Vec<Entry>,
    pub(super) end: End,
}

impl Node {
    /// True when the parser stopped reading the node at a cap, so entries may follow that it did
    /// not read.
    pub(super) fn is_cut(&self) -> bool {
        matches!(self.end, End::TooManyEntries { .. } | End::OverCap)
    }
}

/// Where the parser stopped reading a node. After anything but [`End::Complete`], it reads
/// nothing more of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum End {
    /// At its terminator or the end of the file.
    Complete,
    /// At a `{`, at byte `brace`, that opens a children block.
    Children { brace: usize },
    /// Where one entry more than [`MAX_ENTRIES`] starts, at byte `at`.
    TooManyEntries { at: usize },
    /// After a string that passed a cap on decoded text: the name, or the last entry's key or
    /// value.
    OverCap,
}

/// Why the parser stopped before the end of the file.
#[derive(Debug, PartialEq)]
pub(super) enum Stop {
    /// A syntax error, or a cap the parser words itself.
    Error(FormatError),
    /// A `{` at byte `brace` opens the children block of a slashdashed node, whose name is
    /// `owner` as written.
    DroppedChildren { brace: usize, owner: String },
    /// The node at byte `at` is the first past [`MAX_OBJECTS`] objects, the header not counted.
    TooManyObjects { at: usize },
}

/// What the parser's steps return. The error is boxed: it is rare, and a small `Result` keeps the
/// many `?` on the way cheap.
type Step<T = ()> = Result<T, Box<Stop>>;

/// KDL v2 newline characters. CRLF also counts as a single newline.
fn is_newline(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r' | '\u{85}' | '\u{B}' | '\u{C}' | '\u{2028}' | '\u{2029}'
    )
}

/// KDL v2 unicode-space.
fn is_space(c: char) -> bool {
    matches!(
        c,
        '\t' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}'
    )
}

/// Code points kdl rejects anywhere outside comments, even inside strings.
fn is_disallowed(c: char) -> bool {
    matches!(
        c,
        '\u{0}'..='\u{8}'
            | '\u{E}'..='\u{1F}'
            | '\u{200E}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
    )
}

/// Characters of a bare identifier or number.
fn is_word_char(c: char) -> bool {
    if c.is_ascii() {
        // Every ASCII character up to the space is a space, a newline or disallowed.
        return c > ' '
            && !matches!(
                c,
                '=' | '\\' | '/' | '(' | ')' | '{' | '}' | '[' | ']' | ';' | '"' | '#'
            );
    }
    !(is_space(c)
        || is_newline(c)
        || is_disallowed(c)
        || matches!(
            c,
            '=' | '\\' | '/' | '(' | ')' | '{' | '}' | '[' | ']' | ';' | '"' | '#'
        ))
}

/// True when a bare word is an identifier string, such as `c1`, `-` or `.a`: not a number, not a
/// number gone wrong such as `1a` or `.5`, and not a keyword written without its `#`.
fn is_identifier(word: &str) -> bool {
    let unsigned = word.strip_prefix(['+', '-']).unwrap_or(word);
    let mut chars = unsigned.chars();
    match (chars.next(), chars.next()) {
        (Some(c), _) if c.is_ascii_digit() => false,
        (Some('.'), Some(c)) if c.is_ascii_digit() => false,
        _ => !matches!(word, "true" | "false" | "null" | "inf" | "nan"),
    }
}

/// True when `s` reads back as the same string without quotes.
pub(super) fn is_bare_word(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_word_char) && is_identifier(s)
}

/// The keywords, after their `#`.
const KEYWORDS: [(&str, Value); 6] = [
    ("true", Value::Bool(true)),
    ("false", Value::Bool(false)),
    ("null", Value::Null),
    ("nan", Value::Float(f64::NAN)),
    ("inf", Value::Float(f64::INFINITY)),
    ("-inf", Value::Float(f64::NEG_INFINITY)),
];

/// Help for a word that is neither a string nor a number.
const QUOTE_TEXT: &str = "to write text, put it in quotes";

/// A group of digits in a number.
#[derive(Clone, Copy)]
enum Group {
    /// The digits before any `.` or exponent, in base `radix`, of the number that starts at byte
    /// `start`.
    Whole { start: usize, radix: u32 },
    /// The digits after the `.`.
    Fraction,
    /// The digits of the exponent, after `e` and its sign.
    Exponent,
}

/// What [`Parser::string_or_word`] read.
enum Token {
    /// A quoted, raw or multi-line string, or a bare identifier.
    String(Text),
    /// A bare word that is not an identifier: a number, or a word that is neither.
    Word,
}

/// Decoded text being built, up to the caps on text. Discards everything when not kept.
struct Builder {
    keep: bool,
    text: Text,
    count: TextCount,
}

impl Builder {
    fn new(keep: bool) -> Self {
        Self {
            keep,
            text: Text::default(),
            count: TextCount::new(),
        }
    }

    fn push(&mut self, c: char) {
        if !self.keep || self.text.over.is_some() {
            return;
        }
        match self.count.push(c) {
            Ok(()) => {
                self.text.value.push(c);
                self.text.chars += 1;
            }
            Err(problem) => self.text.over = Some(problem),
        }
    }

    /// Pushes `s`, which holds no line break.
    fn push_unbroken(&mut self, s: &str) {
        if !self.keep || self.text.over.is_some() {
            return;
        }
        let n = s.chars().count();
        if self.count.push_unbroken(n).is_ok() {
            self.text.value.push_str(s);
            self.text.chars += n;
            return;
        }
        // Over the cap: keep the characters up to it.
        for c in s.chars() {
            if self.text.over.is_some() {
                return;
            }
            self.push(c);
        }
    }
}

/// True for a byte that starts no character a string needs to check on its own: not ASCII
/// control (newlines included), not `"` or `\`, and not the first byte of U+0085, U+2028,
/// U+2029, U+FEFF or a disallowed direction mark, all of which start with 0xC2, 0xE2 or 0xEF.
fn is_plain_byte(b: u8) -> bool {
    b >= 0x20 && !matches!(b, b'"' | b'\\' | 0xC2 | 0xE2 | 0xEF)
}

/// Reads a `.lawh` file one node at a time (see the module docs).
pub(super) struct Parser<'a> {
    src: &'a str,
    /// Byte offset of the next character.
    pos: usize,
    /// Nodes started so far, the header included.
    nodes: usize,
}

impl<'a> Parser<'a> {
    pub(super) fn new(src: &'a str) -> Self {
        let bom = if src.starts_with('\u{FEFF}') {
            '\u{FEFF}'.len_utf8()
        } else {
            0
        };
        Self {
            src,
            pos: bom,
            nodes: 0,
        }
    }

    /// The next node the file keeps, or `None` at the end of the file. Not to be called again
    /// after a node that does not end [`End::Complete`].
    pub(super) fn next_node(&mut self) -> Result<Option<Node>, Stop> {
        self.read_node().map_err(|stop| *stop)
    }

    fn read_node(&mut self) -> Step<Option<Node>> {
        self.leading()?;
        if self.at_end() {
            return Ok(None);
        }
        if self.nodes > MAX_OBJECTS {
            return Err(Box::new(Stop::TooManyObjects { at: self.pos }));
        }
        self.nodes += 1;
        let node = self.node(true)?;
        if node.as_ref().is_some_and(|n| n.end == End::Complete)
            && !(self.at_end() || self.terminator())
        {
            return self.unexpected(self.pos, "expected the end of the node", None);
        }
        Ok(node)
    }

    fn rest(&self) -> &'a str {
        self.src.get(self.pos..).unwrap_or_default()
    }

    fn peek(&self) -> Option<char> {
        match self.byte_at(self.pos) {
            Some(b) if b.is_ascii() => Some(char::from(b)),
            Some(_) => self.rest().chars().next(),
            None => None,
        }
    }

    fn peek_second(&self) -> Option<char> {
        self.rest().chars().nth(1)
    }

    fn byte_at(&self, at: usize) -> Option<u8> {
        self.src.as_bytes().get(at).copied()
    }

    fn starts(&self, s: &str) -> bool {
        (self.src.as_bytes().get(self.pos..)).is_some_and(|r| r.starts_with(s.as_bytes()))
    }

    fn at_end(&self) -> bool {
        self.pos >= self.src.len()
    }

    /// The rest of the file, cut one character past the most a token that starts at byte `start`
    /// may take, so a search in it stops soon after the cap.
    fn window(&self, start: usize) -> &'a str {
        let mut end = start
            .saturating_add(MAX_STRING_BYTES + 1)
            .min(self.src.len());
        while !self.src.is_char_boundary(end) {
            end += 1;
        }
        self.src.get(self.pos..end).unwrap_or_default()
    }

    /// Fails once the token that starts at byte `start` takes more than [`MAX_STRING_BYTES`].
    fn check_length(&self, start: usize) -> Step {
        if self.pos.saturating_sub(start) > MAX_STRING_BYTES {
            let (line, column) = line_column(self.src, start);
            return Err(Box::new(Stop::Error(FormatError::StringTooLong {
                line,
                column,
            })));
        }
        Ok(())
    }

    /// The characters from here that a string reads as themselves without checking each one (see
    /// [`is_plain_byte`]), within the cap of the token that starts at byte `start`.
    fn plain_run(&self, start: usize) -> &'a str {
        let window = self.window(start);
        let len = (window.bytes())
            .position(|b| !is_plain_byte(b))
            .unwrap_or(window.len());
        window.get(..len).unwrap_or_default()
    }

    /// Bytes of unicode-space from here, within the cap of the token that starts at byte `start`.
    fn spaces(&self, start: usize) -> usize {
        let window = self.window(start);
        let ascii = (window.bytes())
            .position(|b| !matches!(b, b' ' | b'\t'))
            .unwrap_or(window.len());
        let rest = window.get(ascii..).unwrap_or_default();
        if rest.starts_with(|c: char| !c.is_ascii()) {
            ascii + rest.find(|c| !is_space(c)).unwrap_or(rest.len())
        } else {
            ascii
        }
    }

    /// Length of the newline at byte `at`, if there is one. CRLF counts as one newline.
    fn newline_at(&self, at: usize) -> Option<usize> {
        let bytes = self.src.as_bytes();
        match *bytes.get(at)? {
            b'\r' if bytes.get(at + 1) == Some(&b'\n') => Some(2),
            b'\n' | b'\r' | 0x0B | 0x0C => Some(1),
            b if b.is_ascii() => None,
            _ => (self.src.get(at..)?.chars().next())
                .filter(|&c| is_newline(c))
                .map(char::len_utf8),
        }
    }

    /// A syntax error at byte `at`.
    fn error<T>(&self, at: usize, message: &str, help: Option<&str>) -> Step<T> {
        let (line, column) = line_column(self.src, at);
        Err(Box::new(Stop::Error(FormatError::Syntax {
            line,
            column,
            message: message.to_owned(),
            help: help.map(str::to_owned),
        })))
    }

    fn forbidden<T>(&self, at: usize, c: char) -> Step<T> {
        let code = u32::from(c);
        self.error(
            at,
            &format!("character U+{code:04X} cannot appear as itself in a lawh file"),
            Some(&format!("inside a string, write it as \\u{{{code:X}}}")),
        )
    }

    /// A syntax error for what is at byte `at`: `expected`, then what was found.
    fn unexpected<T>(&self, at: usize, expected: &str, help: Option<&str>) -> Step<T> {
        let found = match self.src.get(at..).and_then(|r| r.chars().next()) {
            None => "the end of the file".to_owned(),
            Some(c) if is_disallowed(c) => return self.forbidden(at, c),
            Some(c) if is_newline(c) => "a line break".to_owned(),
            Some(c) if is_space(c) => "a space".to_owned(),
            Some(c) => format!("`{c}`"),
        };
        self.error(at, &format!("{expected}, found {found}"), help)
    }

    /// Line space and slashdashed nodes before a node. The node before, if any, has already read
    /// its own terminator.
    fn leading(&mut self) -> Step {
        loop {
            if self.line_space()? {
                continue;
            }
            if !self.starts("/-") {
                return Ok(());
            }
            self.pos += 2;
            while self.line_space()? {}
            self.node(false)?;
        }
    }

    /// `;`, a newline or a line comment after a node.
    fn terminator(&mut self) -> bool {
        if self.peek() == Some(';') {
            self.pos += 1;
        } else if let Some(len) = self.newline_at(self.pos) {
            self.pos += len;
        } else if self.starts("//") {
            self.line_comment();
        } else {
            return false;
        }
        true
    }

    /// A node: its type annotation, name and entries. Only a kept node is built and returned; a
    /// slashdashed one is checked and dropped.
    fn node(&mut self, keep: bool) -> Step<Option<Node>> {
        let at = self.pos;
        let annotated = self.peek() == Some('(');
        if annotated {
            self.annotation()?;
            self.node_space()?;
        }
        let name_at = self.pos;
        let Some(Token::String(name)) = self.string_or_word(keep)? else {
            return self.unexpected(
                name_at,
                "expected a node name",
                Some("a node name is a word such as `circle`, or a quoted string"),
            );
        };
        let owner = name_at..self.pos;
        self.after_token(false)?;
        let mut node = keep.then(|| Node {
            at,
            annotated,
            name,
            entries: Vec::with_capacity(MAX_ENTRIES),
            end: End::Complete,
        });
        if let Some(n) = node.as_mut().filter(|n| n.name.over.is_some()) {
            n.end = End::OverCap;
            return Ok(node);
        }
        loop {
            let spaced = self.node_space()?;
            match self.peek() {
                None => break,
                Some('{') => return self.children(node, owner),
                Some(c) if is_newline(c) || c == ';' => break,
                Some(_) if !spaced || self.starts("//") => break,
                Some(_) if self.starts("/-") => {
                    self.pos += 2;
                    while self.line_space()? {}
                    if self.peek() == Some('{') {
                        return self.children(node, owner);
                    }
                    if self.starts("/-") {
                        // kdl accepts this, recursing once per `/-`.
                        return self.error(self.pos, "`/-` cannot follow another `/-`", None);
                    }
                    self.entry(false)?;
                }
                Some(_) => {
                    let Some(n) = node.as_mut() else {
                        self.entry(false)?;
                        continue;
                    };
                    if n.entries.len() == MAX_ENTRIES {
                        n.end = End::TooManyEntries { at: self.pos };
                        return Ok(node);
                    }
                    if let Some(entry) = self.entry(true)? {
                        let over = entry.is_over();
                        n.entries.push(entry);
                        if over {
                            n.end = End::OverCap;
                            return Ok(node);
                        }
                    }
                }
            }
        }
        Ok(node)
    }

    /// The `{` here opens a children block of `node`, or of the slashdashed node whose name is
    /// written at `owner`.
    fn children(&self, node: Option<Node>, owner: Range<usize>) -> Step<Option<Node>> {
        let brace = self.pos;
        if let Some(mut node) = node {
            node.end = End::Children { brace };
            return Ok(Some(node));
        }
        Err(Box::new(Stop::DroppedChildren {
            brace,
            owner: self.src.get(owner).unwrap_or_default().to_owned(),
        }))
    }

    /// An argument or property, after the space before it. Returns it only when `keep`.
    fn entry(&mut self, keep: bool) -> Step<Option<Entry>> {
        let at = self.pos;
        let positional = |annotated, value| {
            keep.then_some(Entry {
                at,
                key: None,
                annotated,
                value,
            })
        };
        if self.peek() == Some('(') {
            self.annotation()?;
            self.node_space()?;
            return Ok(positional(true, self.value(keep)?));
        }
        let key = match self.string_or_word(keep)? {
            Some(Token::String(text)) => text,
            token => return Ok(positional(false, self.finish_value(token, at, keep)?)),
        };
        // A string is a property name when `=` follows.
        self.after_token(true)?;
        let after = self.pos;
        self.node_space()?;
        if self.peek() != Some('=') {
            self.pos = after;
            return Ok(positional(false, Value::String(key)));
        }
        if key.over.is_some() {
            return Ok(Some(Entry {
                at,
                key: Some(key),
                annotated: false,
                value: Value::Null,
            }));
        }
        self.pos += 1;
        self.node_space()?;
        let annotated = self.peek() == Some('(');
        if annotated {
            self.annotation()?;
            self.node_space()?;
        }
        let value = self.value(keep)?;
        Ok(keep.then_some(Entry {
            at,
            key: Some(key),
            annotated,
            value,
        }))
    }

    /// A value: a string, number or keyword such as `#true`.
    fn value(&mut self, keep: bool) -> Step<Value> {
        let start = self.pos;
        let token = self.string_or_word(keep)?;
        self.finish_value(token, start, keep)
    }

    /// The value that starts at byte `start`, once [`Parser::string_or_word`] has read `token`.
    fn finish_value(&mut self, token: Option<Token>, start: usize, keep: bool) -> Step<Value> {
        let value = match token {
            Some(Token::String(text)) => Value::String(text),
            Some(Token::Word) => self.number(start, keep)?,
            None => self.keyword(start)?,
        };
        self.after_token(false)?;
        Ok(value)
    }

    /// A keyword such as `#true` at byte `start`, or an error for anything else.
    fn keyword(&mut self, start: usize) -> Step<Value> {
        let Some(after) = self.rest().strip_prefix('#') else {
            return self.unexpected(start, "expected a value", None);
        };
        let Some((word, value)) = KEYWORDS.into_iter().find(|(w, _)| after.starts_with(w)) else {
            return self.unexpected(
                start + 1,
                "expected `true`, `false`, `null`, `inf`, `-inf` or `nan` after `#`",
                None,
            );
        };
        self.pos += 1 + word.len();
        Ok(value)
    }

    /// Checks the bare word from byte `start` to here, which is not an identifier, as a number,
    /// and reads its value when `keep`. An error points at the first character that is wrong,
    /// or at the start of a group of digits too large for kdl (it reads each into an `i128`).
    fn number(&self, start: usize, keep: bool) -> Step<Value> {
        let word = self.src.get(start..self.pos).unwrap_or_default();
        if matches!(word, "true" | "false" | "null" | "inf" | "nan") {
            return self.error(
                start,
                &format!("`{word}` needs a `#` to be a keyword, like `#{word}`"),
                Some(QUOTE_TEXT),
            );
        }
        let negative = word.starts_with('-');
        let unsigned = start + usize::from(word.starts_with(['+', '-']));
        let radix = match self.src.get(unsigned..unsigned + 2) {
            Some("0x" | "0X") => 16,
            Some("0o" | "0O") => 8,
            Some("0b" | "0B") => 2,
            _ => 10,
        };
        let signed = |v: i128| Value::Integer(if negative { -v } else { v });
        if radix != 10 {
            let (value, end) = self.digits(unsigned + 2, Group::Whole { start, radix })?;
            self.number_end(end)?;
            return Ok(signed(value));
        }
        if !self.byte_at(unsigned).is_some_and(|b| b.is_ascii_digit()) {
            return self.error(
                start,
                "a number must start with a digit",
                Some("write a 0 before the `.`, like `0.5`"),
            );
        }
        let (integer, mut end) = self.digits(unsigned, Group::Whole { start, radix })?;
        let mut float = false;
        if self.byte_at(end) == Some(b'.') {
            (_, end) = self.digits(end + 1, Group::Fraction)?;
            float = true;
        }
        if matches!(self.byte_at(end), Some(b'e' | b'E')) {
            let mut exponent = end + 1;
            if matches!(self.byte_at(exponent), Some(b'+' | b'-')) {
                exponent += 1;
            }
            (_, end) = self.digits(exponent, Group::Exponent)?;
            float = true;
        }
        self.number_end(end)?;
        if !float {
            return Ok(signed(integer));
        }
        if !keep {
            return Ok(Value::Null);
        }
        let parsed = if word.contains('_') {
            word.replace('_', "").parse()
        } else {
            word.parse()
        };
        parsed
            .map(Value::Float)
            .or_else(|_| self.error(start, "invalid number", Some(QUOTE_TEXT)))
    }

    /// `digit (digit | '_')*` from byte `at`, within the word that ends here: the `group` of a
    /// number. Returns its value and where it ends.
    fn digits(&self, at: usize, group: Group) -> Step<(i128, usize)> {
        let radix = match group {
            Group::Whole { radix, .. } => radix,
            Group::Fraction | Group::Exponent => 10,
        };
        let digits = self.src.get(at..self.pos).unwrap_or_default();
        if !digits.starts_with(|c: char| c.is_digit(radix)) {
            let expected = match group {
                Group::Whole { radix: 16, .. } => "expected a hexadecimal digit after `0x`",
                Group::Whole { radix: 8, .. } => "expected an octal digit (0 to 7) after `0o`",
                Group::Whole { radix: 2, .. } => "expected a binary digit (0 or 1) after `0b`",
                Group::Whole { .. } => "expected a digit",
                Group::Fraction => "expected a digit after the `.`",
                Group::Exponent => "expected a digit in the exponent",
            };
            return self.unexpected(at, expected, None);
        }
        let mut value = Some(0_i128);
        let mut len = 0;
        for c in digits.chars() {
            if c != '_' {
                let Some(d) = c.to_digit(radix) else { break };
                value = value
                    .and_then(|v| v.checked_mul(i128::from(radix)))
                    .and_then(|v| v.checked_add(i128::from(d)));
            }
            len += 1;
        }
        // kdl reads every group of digits into an `i128`.
        let Some(value) = value else {
            let (at, message, help) = match group {
                Group::Whole { start, .. } => (
                    start,
                    "number is too large",
                    "whole numbers must fit in a 128-bit integer; write a larger one with an \
                     exponent, like `1e40`",
                ),
                Group::Fraction => (
                    at,
                    "too many digits after the `.`",
                    "the digits after the `.` must fit in a 128-bit integer, about 38 digits",
                ),
                Group::Exponent => (
                    at,
                    "exponent is too large",
                    "the exponent must fit in a 128-bit integer",
                ),
            };
            return self.error(at, message, Some(help));
        };
        Ok((value, at + len))
    }

    /// Fails unless the number's digits end where its word does.
    fn number_end(&self, end: usize) -> Step {
        if end == self.pos {
            return Ok(());
        }
        self.unexpected(end, "expected the end of the number", Some(QUOTE_TEXT))
    }

    /// A type annotation `(name)`.
    fn annotation(&mut self) -> Step {
        self.pos += 1;
        self.node_space()?;
        let start = self.pos;
        if !matches!(self.string_or_word(false)?, Some(Token::String(_))) {
            return self.unexpected(start, "expected a type name after `(`", None);
        }
        self.node_space()?;
        if self.peek() != Some(')') {
            return self.unexpected(self.pos, "expected `)` after the type name", None);
        }
        self.pos += 1;
        Ok(())
    }

    /// Reads a quoted string, raw string or bare word if one starts here, decoding strings when
    /// `keep`. Returns `None`, reading nothing, for anything else.
    fn string_or_word(&mut self, keep: bool) -> Step<Option<Token>> {
        match self.peek() {
            Some('"') => self.quoted(keep).map(|t| Some(Token::String(t))),
            Some('#') if matches!(self.peek_second(), Some('#' | '"')) => {
                self.raw(keep).map(|t| Some(Token::String(t)))
            }
            Some(c) if is_word_char(c) => {
                let start = self.pos;
                let window = self.window(start);
                // ASCII byte by byte, then any other characters one by one.
                let ascii = (window.bytes())
                    .position(|b| !(b.is_ascii() && is_word_char(char::from(b))))
                    .unwrap_or(window.len());
                let rest = window.get(ascii..).unwrap_or_default();
                let other = if rest.starts_with(|c: char| !c.is_ascii()) {
                    rest.find(|c| !is_word_char(c)).unwrap_or(rest.len())
                } else {
                    0
                };
                self.pos += ascii + other;
                self.check_length(start)?;
                let word = self.src.get(start..self.pos).unwrap_or_default();
                if !is_identifier(word) {
                    return Ok(Some(Token::Word));
                }
                let mut text = Builder::new(keep);
                // A bare word holds no line break.
                text.push_unbroken(word);
                Ok(Some(Token::String(text.text)))
            }
            _ => Ok(None),
        }
    }

    /// kdl's check that a name or value is followed by space, a terminator or (for a property
    /// name) `=`.
    fn after_token(&self, property_name: bool) -> Step {
        match self.peek() {
            None => Ok(()),
            Some(c) if is_space(c) || is_newline(c) || matches!(c, ';' | '{' | '\\') => Ok(()),
            Some('=') if property_name => Ok(()),
            Some('/') if self.starts("//") || self.starts("/*") => Ok(()),
            Some(_) => self.unexpected(self.pos, "expected a space or the end of the node", None),
        }
    }

    /// `"..."` or a multi-line `"""` string.
    fn quoted(&mut self, keep: bool) -> Step<Text> {
        let start = self.pos;
        if self.starts("\"\"\"") {
            self.pos += 3;
            let Some(newline) = self.newline_at(self.pos) else {
                return self.error(
                    start,
                    "Multi-line string opening quotes must be immediately followed by a newline",
                    None,
                );
            };
            self.pos += newline;
            return self.multi_line(start, None, keep);
        }
        self.pos += 1;
        let mut text = Builder::new(keep);
        loop {
            let run = self.plain_run(start);
            text.push_unbroken(run);
            self.pos += run.len();
            self.check_length(start)?;
            match self.peek() {
                None => return self.error(start, "unclosed string", None),
                Some('"') => {
                    self.pos += 1;
                    self.check_length(start)?;
                    return Ok(text.text);
                }
                Some('\\') => self.escape(start, &mut text)?,
                Some(c) if is_newline(c) => {
                    return self.error(
                        start,
                        "Unexpected newline in single-line quoted string",
                        Some(
                            "You can make a string multi-line by wrapping it in '\"\"\"', with a \
                             newline immediately after the opening quotes.",
                        ),
                    );
                }
                Some(c) if is_disallowed(c) => return self.forbidden(self.pos, c),
                Some(c) => {
                    text.push(c);
                    self.pos += c.len_utf8();
                }
            }
        }
    }

    /// An escape in the quoted string that starts at byte `start`, at its backslash.
    fn escape(&mut self, start: usize, text: &mut Builder) -> Step {
        let at = self.pos;
        self.pos += 1;
        let c = match self.peek() {
            Some('\\') => '\\',
            Some('"') => '"',
            Some('b') => '\u{8}',
            Some('f') => '\u{C}',
            Some('n') => '\n',
            Some('r') => '\r',
            Some('t') => '\t',
            Some('s') => ' ',
            Some('u') => return self.unicode_escape(at, text),
            // A whitespace escape: the backslash and all the space and newlines after it.
            Some(c) if is_space(c) || is_newline(c) => {
                let window = self.window(start);
                self.pos += window
                    .find(|c| !(is_space(c) || is_newline(c)))
                    .unwrap_or(window.len());
                return self.check_length(start);
            }
            _ => return self.invalid_escape(at),
        };
        text.push(c);
        self.pos += 1;
        Ok(())
    }

    /// `\u{...}` at byte `at`, with 1 to 6 hex digits naming a Unicode scalar value.
    fn unicode_escape(&mut self, at: usize, text: &mut Builder) -> Step {
        let digits_at = at + "\\u{".len();
        let len = (self.src.get(at + 2..))
            .and_then(|r| r.strip_prefix('{'))
            .map_or(0, |r| {
                r.bytes().take(7).take_while(u8::is_ascii_hexdigit).count()
            });
        let c = Some(len)
            .filter(|len| (1..=6).contains(len))
            .filter(|len| self.byte_at(digits_at + len) == Some(b'}'))
            .and_then(|len| self.src.get(digits_at..digits_at + len))
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .and_then(char::from_u32);
        let Some(c) = c else {
            return self.invalid_escape(at);
        };
        text.push(c);
        self.pos = digits_at + len + 1;
        Ok(())
    }

    fn invalid_escape<T>(&self, at: usize) -> Step<T> {
        let rest = self.src.get(at..).unwrap_or_default();
        // Show `\u{...}` up to its brace, otherwise the backslash and one character.
        let shown = match rest.get(..2) {
            Some("\\u") => rest
                .char_indices()
                .take(16)
                .find(|&(_, c)| c == '}')
                .map_or("\\u", |(i, _)| rest.get(..=i).unwrap_or("\\u")),
            _ => rest
                .char_indices()
                .nth(1)
                .filter(|&(_, c)| !(c.is_control() || is_disallowed(c)))
                .map_or("\\", |(i, c)| rest.get(..i + c.len_utf8()).unwrap_or("\\")),
        };
        self.error(
            at,
            &format!("invalid escape `{shown}`"),
            Some(
                "valid escapes are \\\\, \\\", \\b, \\f, \\n, \\r, \\s, \\t and \\u{...} with 1 to \
                 6 hex digits",
            ),
        )
    }

    /// `#"..."#` or a multi-line `#"""` raw string, with any number of `#`.
    fn raw(&mut self, keep: bool) -> Step<Text> {
        let start = self.pos;
        let hashes = self
            .window(start)
            .bytes()
            .take_while(|&b| b == b'#')
            .count();
        self.pos += hashes;
        self.check_length(start)?;
        if self.peek() != Some('"') {
            return self.unexpected(self.pos, "expected `\"` after `#`", None);
        }
        if self.starts("\"\"\"")
            && let Some(newline) = self.newline_at(self.pos + 3)
        {
            self.pos += 3 + newline;
            return self.multi_line(start, Some(hashes), keep);
        }
        self.pos += 1;
        let body = self.pos;
        let mut text = Builder::new(keep);
        loop {
            let run = self.plain_run(start);
            text.push_unbroken(run);
            self.pos += run.len();
            self.check_length(start)?;
            match self.peek() {
                None => return self.error(start, "unclosed raw string", None),
                Some('"') if self.closes_raw(self.pos, hashes) => {
                    if self.src.get(body..self.pos) == Some("\"") {
                        return self.error(
                            start,
                            "Single-line raw strings cannot look like multi-line ones",
                            None,
                        );
                    }
                    self.pos += 1 + hashes;
                    self.check_length(start)?;
                    return Ok(text.text);
                }
                Some(c) if is_newline(c) => {
                    return self.error(start, "Unexpected newline in single-line raw string", None);
                }
                Some(c) if is_disallowed(c) => return self.forbidden(self.pos, c),
                Some(c) => {
                    text.push(c);
                    self.pos += c.len_utf8();
                }
            }
        }
    }

    /// True when `"` and `hashes` times `#` start at byte `at`.
    fn closes_raw(&self, at: usize, hashes: usize) -> bool {
        let rest = self.src.get(at..).unwrap_or_default().as_bytes();
        rest.first() == Some(&b'"')
            && rest
                .get(1..=hashes)
                .is_some_and(|h| h.iter().all(|&b| b == b'#'))
    }

    /// True when the closing quotes of a multi-line string start at byte `at`.
    fn closes_multi_line(&self, at: usize, raw: Option<usize>) -> bool {
        self.src
            .get(at..)
            .and_then(|rest| rest.strip_prefix("\"\""))
            .is_some_and(|_| self.closes_raw(at + 2, raw.unwrap_or(0)))
    }

    /// The body of a multi-line string that starts at byte `start`, from the line after its
    /// opening quotes. `raw` is the number of `#` of a raw string.
    ///
    /// The closing quotes go on a line of their own, after nothing but spaces (and, in a quoted
    /// string, whitespace escapes). Every other line starts with those spaces, which are not part
    /// of the text, or holds nothing but spaces and reads as empty. Line breaks read as `\n`.
    /// The spaces are only known at the closing quotes, so the body is read twice: once to check
    /// it and find them, then to check each line's start and decode it.
    fn multi_line(&mut self, start: usize, raw: Option<usize>, keep: bool) -> Step<Text> {
        let body = self.pos;
        let (close, indent, quotes) = self.closing_line(start, raw)?;
        let prefix = self.src.get(close..indent).unwrap_or_default();
        self.pos = body;
        let mut text = Builder::new(keep);
        while self.pos < close {
            let line = self.pos;
            if line > body {
                text.push('\n');
            }
            let rest = self.src.get(line..close).unwrap_or_default();
            let spaces = rest.find(|c| !is_space(c)).unwrap_or(rest.len());
            if let Some(newline) = self.newline_at(line + spaces) {
                self.pos = line + spaces + newline;
                continue;
            }
            if !rest.starts_with(prefix) {
                return self.error(
                    line,
                    "this line of a multi-line string does not start with the whitespace before \
                     its closing quotes",
                    None,
                );
            }
            self.pos = line + prefix.len();
            loop {
                let run = self.plain_run(start);
                text.push_unbroken(run);
                self.pos += run.len();
                match self.peek() {
                    None => break,
                    Some(c) if is_newline(c) => {
                        self.pos += self.newline_at(self.pos).unwrap_or(1);
                        break;
                    }
                    Some('\\') if raw.is_none() => self.escape(start, &mut text)?,
                    Some(c) => {
                        text.push(c);
                        self.pos += c.len_utf8();
                    }
                }
            }
        }
        self.pos = quotes + 3 + raw.unwrap_or(0);
        self.check_length(start)?;
        Ok(text.text)
    }

    /// Checks the lines of a multi-line string from here to its closing line. Returns where that
    /// line starts, where its spaces end and where its closing quotes start.
    fn closing_line(&mut self, start: usize, raw: Option<usize>) -> Step<(usize, usize, usize)> {
        let mut skipped = Builder::new(false);
        loop {
            let line = self.pos;
            let indent = line + self.spaces(start);
            // A quoted string's closing line may also hold whitespace escapes after the indent.
            let mut quotes = indent;
            while raw.is_none()
                && self.src.get(quotes..).is_some_and(|r| {
                    r.strip_prefix('\\')
                        .is_some_and(|r| r.starts_with(|c| is_space(c) || is_newline(c)))
                })
            {
                self.pos = quotes + 1;
                let window = self.window(start);
                quotes = self.pos
                    + window
                        .find(|c| !(is_space(c) || is_newline(c)))
                        .unwrap_or(window.len());
            }
            self.pos = quotes;
            self.check_length(start)?;
            if self.closes_multi_line(quotes, raw) {
                return Ok((line, indent, quotes));
            }
            // The rest of the line, after its spaces and whitespace escapes.
            loop {
                self.pos += self.plain_run(start).len();
                self.check_length(start)?;
                match self.peek() {
                    None => {
                        return self.error(
                            start,
                            "unclosed multi-line string",
                            Some("close it with `\"\"\"` on a line of its own"),
                        );
                    }
                    Some(c) if is_newline(c) => {
                        self.pos += self.newline_at(self.pos).unwrap_or(1);
                        break;
                    }
                    Some('\\') if raw.is_none() => self.escape(start, &mut skipped)?,
                    Some('"') if self.closes_multi_line(self.pos, raw) => {
                        return self.error(
                            self.pos,
                            "the closing quotes of a multi-line string must be on a line of \
                             their own",
                            None,
                        );
                    }
                    Some(c) if is_disallowed(c) => return self.forbidden(self.pos, c),
                    Some(c) => self.pos += c.len_utf8(),
                }
            }
        }
    }

    /// node-space*: spaces, block comments and line continuations. True if it read any.
    fn node_space(&mut self) -> Step<bool> {
        let start = self.pos;
        loop {
            match self.peek() {
                Some(c) if is_space(c) => self.pos += c.len_utf8(),
                Some('/') if self.starts("/*") => self.block_comment()?,
                Some('\\') => self.line_continuation()?,
                _ => break,
            }
        }
        Ok(self.pos > start)
    }

    /// Some line-space: node-space, a newline or a line comment. True if it read any.
    fn line_space(&mut self) -> Step<bool> {
        if self.node_space()? {
            return Ok(true);
        }
        if let Some(len) = self.newline_at(self.pos) {
            self.pos += len;
        } else if self.starts("//") {
            self.line_comment();
        } else {
            return Ok(false);
        }
        Ok(true)
    }

    /// `\` then spaces and comments up to the end of the line (kdl's escline), and the newline.
    fn line_continuation(&mut self) -> Step {
        self.pos += 1;
        loop {
            match self.peek() {
                Some(c) if is_space(c) => self.pos += c.len_utf8(),
                Some('/') if self.starts("/*") => self.block_comment()?,
                _ => break,
            }
        }
        if self.starts("//") {
            self.line_comment();
        } else if let Some(len) = self.newline_at(self.pos) {
            self.pos += len;
        } else if !self.at_end() {
            return self.unexpected(self.pos, "expected a line break after `\\`", None);
        }
        Ok(())
    }

    /// `//` through the next newline, or to the end of the file.
    fn line_comment(&mut self) {
        match self.rest().find(is_newline) {
            None => self.pos = self.src.len(),
            Some(i) => {
                self.pos += i;
                self.pos += self.newline_at(self.pos).unwrap_or(1);
            }
        }
    }

    /// `/* ... */`, nested at most [`MAX_COMMENT_DEPTH`] deep.
    fn block_comment(&mut self) -> Step {
        let open = self.pos;
        self.pos += 2;
        let mut depth = 1;
        loop {
            let Some(i) = self.rest().find(['*', '/']) else {
                return self.error(open, "unclosed block comment", Some("close it with `*/`"));
            };
            self.pos += i;
            if self.starts("*/") {
                self.pos += 2;
                depth -= 1;
                if depth == 0 {
                    return Ok(());
                }
            } else if self.starts("/*") {
                if depth == MAX_COMMENT_DEPTH {
                    let (line, column) = line_column(self.src, self.pos);
                    return Err(Box::new(Stop::Error(FormatError::CommentTooDeep {
                        line,
                        column,
                    })));
                }
                depth += 1;
                self.pos += 2;
            } else {
                self.pos += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use kdl::{KdlDocument, KdlValue};
    use proptest::prelude::*;

    use super::*;
    use crate::board::{MAX_TEXT_CHARS, MAX_TEXT_LINES};

    /// Cases for the property tests: `LAWH_PROPTEST_CASES` if set, otherwise `default`.
    fn cases(default: u32) -> u32 {
        std::env::var("LAWH_PROPTEST_CASES")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(default)
    }

    /// A value as compared with kdl's: floats by their bits.
    #[derive(Debug, PartialEq)]
    enum Seen {
        String(String),
        Integer(i128),
        Float(u64),
        Bool(bool),
        Null,
    }

    /// Nodes as compared with kdl's: name, type annotation or not, and entries (key, type
    /// annotation or not, value).
    type Outline = Vec<(String, bool, Vec<(Option<String>, bool, Seen)>)>;

    fn seen_kdl(value: &KdlValue) -> Seen {
        match value {
            KdlValue::String(s) => Seen::String(s.clone()),
            KdlValue::Integer(i) => Seen::Integer(*i),
            KdlValue::Float(f) => Seen::Float(f.to_bits()),
            KdlValue::Bool(b) => Seen::Bool(*b),
            KdlValue::Null => Seen::Null,
        }
    }

    fn outline_kdl(doc: &KdlDocument) -> Outline {
        let nodes = doc.nodes().iter();
        nodes
            .map(|n| {
                let entries = (n.entries().iter())
                    .map(|e| {
                        let key = e.name().map(|k| k.value().to_owned());
                        (key, e.ty().is_some(), seen_kdl(e.value()))
                    })
                    .collect();
                (n.name().value().to_owned(), n.ty().is_some(), entries)
            })
            .collect()
    }

    fn seen(value: &Value) -> Seen {
        match value {
            Value::String(text) => Seen::String(text.value.clone()),
            Value::Integer(i) => Seen::Integer(*i),
            Value::Float(f) => Seen::Float(f.to_bits()),
            Value::Bool(b) => Seen::Bool(*b),
            Value::Null => Seen::Null,
        }
    }

    fn outline(nodes: &[Node]) -> Outline {
        nodes
            .iter()
            .map(|n| {
                let entries = (n.entries.iter())
                    .map(|e| {
                        let key = e.key.as_ref().map(|k| k.value.clone());
                        (key, e.annotated, seen(&e.value))
                    })
                    .collect();
                (n.name.value.clone(), n.annotated, entries)
            })
            .collect()
    }

    /// What the parser reads of a file.
    #[derive(Debug)]
    enum Read {
        /// Every node, to the end of the file.
        All(Vec<Node>),
        /// The nodes before a children block whose `{` is at byte `brace`, its owner included
        /// when the file keeps it.
        Children(Vec<Node>, usize),
        /// The nodes before a cap stopped reading.
        Cut(Vec<Node>),
    }

    fn read(src: &str) -> Result<Read, Stop> {
        let mut parser = Parser::new(src);
        let mut nodes = Vec::new();
        loop {
            match parser.next_node() {
                Ok(None) => return Ok(Read::All(nodes)),
                Ok(Some(node)) => {
                    let end = node.end;
                    nodes.push(node);
                    match end {
                        End::Complete => {}
                        End::Children { brace } => return Ok(Read::Children(nodes, brace)),
                        End::TooManyEntries { .. } | End::OverCap => return Ok(Read::Cut(nodes)),
                    }
                }
                Err(Stop::DroppedChildren { brace, .. }) => {
                    return Ok(Read::Children(nodes, brace));
                }
                Err(Stop::TooManyObjects { .. }) => return Ok(Read::Cut(nodes)),
                Err(stop) => return Err(stop),
            }
        }
    }

    /// The value of the first entry of `n {value}`, or the error's column and message.
    fn value(value: &str) -> Result<Seen, (usize, String)> {
        match read(&format!("n {value}")) {
            Ok(Read::All(nodes)) => Ok(seen(&nodes[0].entries[0].value)),
            Err(Stop::Error(e)) => {
                let message = e.to_string();
                Err((e.column(), message.split_once(": ").unwrap().1.to_owned()))
            }
            other => panic!("{value:?}: {other:?}"),
        }
    }

    #[test]
    fn words() {
        for word in [
            "c1", "-", "+x", "-inf", ".a", "-.", "_1", "..5", "+-1", "-true",
        ] {
            assert!(is_identifier(word), "{word}");
            assert_eq!(value(word), Ok(Seen::String(word.to_owned())), "{word}");
        }
        for (word, want) in [
            ("0", Seen::Integer(0)),
            ("-0", Seen::Integer(0)),
            ("-1_000", Seen::Integer(-1000)),
            ("+5", Seen::Integer(5)),
            ("0x1F_ff", Seen::Integer(0x1F_FF)),
            ("-0o17", Seen::Integer(-0o17)),
            ("0b1_0", Seen::Integer(2)),
            ("0B1", Seen::Integer(1)),
            (
                "170141183460469231731687303715884105727",
                Seen::Integer(i128::MAX),
            ),
            ("1.5e-3", Seen::Float(1.5e-3_f64.to_bits())),
            ("1E+300", Seen::Float(1e300_f64.to_bits())),
            ("1e999", Seen::Float(f64::INFINITY.to_bits())),
            ("5e-324", Seen::Float(5e-324_f64.to_bits())),
            ("-0.0", Seen::Float((-0.0_f64).to_bits())),
            ("1_.5_e1_0", Seen::Float(1.5e10_f64.to_bits())),
            ("#true", Seen::Bool(true)),
            ("#null", Seen::Null),
            ("#-inf", Seen::Float(f64::NEG_INFINITY.to_bits())),
        ] {
            assert_eq!(value(word), Ok(want), "{word}");
        }
    }

    /// Errors in a number point at the first character that is wrong, or at the start of a
    /// group of digits that is too large.
    #[test]
    fn bad_numbers_point_at_what_is_wrong() {
        for (word, column) in [
            ("true", 3),
            ("nan", 3),
            (".5", 3),
            ("-.5", 3),
            ("1.", 5),
            ("1._5", 5),
            ("1.5.5", 6),
            ("1a", 4),
            ("1e", 5),
            ("1e+", 6),
            ("1E_3", 5),
            ("0x", 5),
            ("0xg", 5),
            ("-0b12", 7),
            ("0o8", 5),
            ("170141183460469231731687303715884105728", 3),
            ("-170141183460469231731687303715884105728", 3),
            ("0x1_0000_0000_0000_0000_0000_0000_0000_0000", 3),
            ("1.170141183460469231731687303715884105728", 5),
            ("1e-170141183460469231731687303715884105728", 6),
            ("#x", 4),
            ("#truex", 8),
            ("##x", 5),
        ] {
            let (at, message) = value(word).unwrap_err();
            assert_eq!(at, column, "{word}: {message}");
        }
    }

    #[test]
    fn children_blocks_stop_reading() {
        let children = |src| match read(src) {
            Ok(Read::Children(nodes, brace)) => (nodes.len(), brace),
            other => panic!("{src:?}: {other:?}"),
        };
        assert_eq!(children("a\nb x=1 {\n}"), (2, 8));
        assert_eq!(children("b{}"), (1, 1));
        assert_eq!(children("b /-\n {}"), (1, 6));
        assert_eq!(children("a\n/-b {}"), (1, 6));
        assert_eq!(
            Parser::new("a\n/-b {}").next_node().map(|_| ()),
            Ok(()),
            "the node before is read first"
        );
        let mut parser = Parser::new("a\n/-\"b\" x {}");
        parser.next_node().unwrap();
        assert_eq!(
            parser.next_node(),
            Err(Stop::DroppedChildren {
                brace: 10,
                owner: "\"b\"".to_owned()
            })
        );
        // Braces inside strings and comments are text.
        assert!(matches!(
            read("a \"{\" #\"}\"# /* { */ // {"),
            Ok(Read::All(_))
        ));
    }

    #[test]
    fn objects_past_the_limit_stop_reading() {
        // The header and as many objects as a board holds.
        let full = "c\n".repeat(MAX_OBJECTS + 1);
        assert!(matches!(read(&full), Ok(Read::All(n)) if n.len() == MAX_OBJECTS + 1));
        // Slashdashed nodes and comments don't count; the next node does.
        let over = format!("{full}/-c\n// c\n  c x=1 {{");
        let mut parser = Parser::new(&over);
        for _ in 0..=MAX_OBJECTS {
            parser.next_node().unwrap();
        }
        assert_eq!(
            parser.next_node(),
            Err(Stop::TooManyObjects {
                at: full.len() + "/-c\n// c\n  ".len()
            })
        );
    }

    #[test]
    fn entries_past_the_limit_stop_reading() {
        let src = "n 1 /-2 x=3 \"4\" /-y=5 5 6 7 ) 8";
        let Ok(Read::Cut(nodes)) = read(src) else {
            panic!("{:?}", read(src));
        };
        let node = &nodes[0];
        assert_eq!(node.entries.len(), MAX_ENTRIES);
        // Reading stops before the 7th entry, so the `)` after the 6th is not read.
        assert_eq!(
            node.end,
            End::TooManyEntries {
                at: src.find(')').unwrap()
            }
        );
        // Slashdashed nodes are checked, not kept, so their entries are not counted.
        let dropped = format!("/-n {}\nm", "1 ".repeat(100));
        assert!(matches!(read(&dropped), Ok(Read::All(n)) if n.len() == 1));
    }

    /// A string past a cap on text keeps the text up to it, is checked to its end, and ends its
    /// node.
    #[test]
    fn text_past_the_caps_ends_the_node() {
        let long = format!("n \"{}\" )", "é".repeat(MAX_TEXT_CHARS + 1));
        let Ok(Read::Cut(nodes)) = read(&long) else {
            panic!();
        };
        let Value::String(text) = &nodes[0].entries[0].value else {
            panic!();
        };
        assert_eq!(text.over, Some(TextProblem::TooLong));
        assert_eq!(
            (text.chars, text.value.chars().count()),
            (MAX_TEXT_CHARS, MAX_TEXT_CHARS)
        );
        assert_eq!(nodes[0].end, End::OverCap);
        // Checked to its end: an escape after the cap is still wrong.
        let bad = format!("n \"{}\\x\"", "a".repeat(MAX_TEXT_CHARS + 1));
        assert!(matches!(
            read(&bad),
            Err(Stop::Error(FormatError::Syntax { .. }))
        ));

        let lines = format!("n \"\"\"\n{}\"\"\" )", "\n".repeat(MAX_TEXT_LINES + 1));
        let Ok(Read::Cut(nodes)) = read(&lines) else {
            panic!();
        };
        let Value::String(text) = &nodes[0].entries[0].value else {
            panic!();
        };
        assert_eq!(text.over, Some(TextProblem::TooManyLines));
        assert_eq!(text.value, "\n".repeat(MAX_TEXT_LINES - 1));
        // Names and keys too.
        let name = format!("{} x=1", "n".repeat(MAX_TEXT_CHARS + 1));
        assert!(matches!(read(&name), Ok(Read::Cut(n)) if n[0].name.over.is_some()));
        let key = format!("n {}=1 )", "k".repeat(MAX_TEXT_CHARS + 1));
        let Ok(Read::Cut(nodes)) = read(&key) else {
            panic!();
        };
        assert!(nodes[0].entries[0].key.as_ref().unwrap().over.is_some());
    }

    #[test]
    fn long_strings_stop_reading() {
        let at_limit = |len: usize| format!("n \"{}\"", "a".repeat(len - 2));
        assert!(read(&at_limit(MAX_STRING_BYTES)).is_ok());
        assert_eq!(
            read(&at_limit(MAX_STRING_BYTES + 1)).unwrap_err(),
            Stop::Error(FormatError::StringTooLong { line: 1, column: 3 })
        );
        // Whatever makes the source long: spaces in a whitespace escape, indentation in a
        // multi-line string, a bare word, a number or raw string hashes.
        let spaces = " ".repeat(MAX_STRING_BYTES);
        for src in [
            format!("n \"a\\{spaces}b\""),
            format!("n \"\"\"\n{spaces}a\n{spaces}\"\"\""),
            format!("n {}", "w".repeat(MAX_STRING_BYTES + 1)),
            format!("n 1{}", "_".repeat(MAX_STRING_BYTES)),
            format!("n {}\"a\"", "#".repeat(MAX_STRING_BYTES)),
        ] {
            assert_eq!(
                read(&src).unwrap_err(),
                Stop::Error(FormatError::StringTooLong { line: 1, column: 3 }),
                "{src:.20?}"
            );
        }
    }

    /// Multi-line strings lose the indentation of their closing quotes, and whitespace-only
    /// lines read as empty.
    #[test]
    fn multi_line_strings_are_decoded_like_kdl() {
        for (src, want) in [
            (
                "n \"\"\"\n    a\n      b \"q\"\n\n  \n    \"\"\"",
                "a\n  b \"q\"\n\n",
            ),
            ("n \"\"\"\n  one \\\n    two\\n\n  \"\"\"", "one two\n"),
            ("n \"\"\"\r\n  a\r\n  b\u{85}  c\r\n  \"\"\"", "a\nb\nc"),
            ("n #\"\"\"\n\t\"raw\" \\n\n\t\"\"\"#", "\"raw\" \\n"),
            ("n \"\"\"\n\"\"\"", ""),
            ("n \"\"\"\n  a\n  \\ \"\"\"", "a"),
        ] {
            let kdl = outline_kdl(&KdlDocument::parse_v2(src).unwrap());
            assert_eq!(kdl[0].2[0].2, Seen::String(want.to_owned()), "{src:?}");
            let Ok(Read::All(nodes)) = read(src) else {
                panic!("{src:?}");
            };
            assert_eq!(outline(&nodes), kdl, "{src:?}");
        }
    }

    /// Flat KDL that hand-written files may use.
    #[test]
    fn flat_kdl_is_read_as_kdl_reads_it() {
        for src in [
            "",
            "\u{FEFF}lawh version=1",
            "a; b;c\r\nd\u{85}e\u{2028}f",
            "a x=1 \"y\"=2 #\"z\"#=#true w = #-inf",
            "a #\"raw \"quoted\" \\n\"# ##\"a #\"b\"# c\"##",
            "a \"\"\"\n    line one\n      line two \"q\"\n\n    \"\"\"",
            "a #\"\"\"\n  raw \\n multi\n  \"\"\"#",
            "a \"\"\"\n  one \\\n    two\n  \"\"\" b",
            "a \"x\\ \n   y\" \"\\u{1F600}\\s\\t\\b\\f\\u{0}\"",
            "/-a 1\n/- \n b 2\nc /-x /- y=1 z",
            "a \\\n  b \\ // note\n  c",
            "a /* x */ b/**/c=/* y */1",
            "(t)a (u)1 x=(v)2 ( w )\"s\"",
            "a -1 +2 0x1F 0o7 0b1 1_000.5e-3\nb -inf .x - _",
            "// only a comment",
            "/* only a comment */",
            "a \u{7F}",
            "a\u{3000}b\u{205F}=\u{A0}1",
        ] {
            let kdl = outline_kdl(&KdlDocument::parse_v2(src).unwrap());
            let Ok(Read::All(nodes)) = read(src) else {
                panic!("{src:?}: {:?}", read(src));
            };
            assert_eq!(outline(&nodes), kdl, "{src:?}");
        }
    }

    /// Pieces of KDL, valid and not, joined at random by the property tests below.
    const PIECES: &[&str] = &[
        " ",
        " ",
        " ",
        "    ",
        "\n",
        "\n",
        "\t",
        "\r\n",
        "\r",
        "\u{85}",
        "\u{2028}",
        "\u{A0}",
        "\u{3000}",
        "\u{B}",
        ";",
        "a",
        "node",
        "x",
        "-",
        "+",
        ".",
        "_",
        "-a",
        ".a",
        "-.",
        "é",
        "*",
        "1",
        "-1",
        "+1",
        "1.5",
        "1e3",
        "1E+3",
        "1.5e-3",
        "0x1F",
        "-0o7",
        "0b1_0",
        "1_",
        "-0",
        "-0.0",
        "1e999",
        "5e-324",
        "1.",
        ".5",
        "1e",
        "0x",
        "1a",
        "0b2",
        "1.5.5",
        "170141183460469231731687303715884105727",
        "170141183460469231731687303715884105728",
        "true",
        "inf",
        "-inf",
        "#true",
        "#null",
        "#-inf",
        "#nan",
        "#x",
        "#",
        "=",
        "=",
        "x=",
        "a=1",
        "\"k\"=",
        "\"s\"",
        "\"\"",
        "\"a b\"",
        "\"\\n\"",
        "\"\\s\\t\\b\\f\"",
        "\"\\u{41}\"",
        "\"\\u{0}\"",
        "\"\\u{D800}\"",
        "\"\\x\"",
        "\"a\\\n  b\"",
        "\"\n",
        "\"",
        "\"\\\"\"",
        "\"{\"",
        "\"/*\"",
        "\"\u{202E}\"",
        "\"\u{7F}\"",
        "#\"r\"#",
        "##\"a\"#\"##",
        "#\"\"\"#",
        "#\"",
        "#\"\"\"x\"#",
        "##\"",
        "\"\"\"\nml\n\"\"\"",
        "\"\"\"\n  a\n  \"\"\"",
        "\"\"\"\n a\n  \"\"\"",
        "\"\"\"\n\n  a\n  \"\"\"",
        "\"\"\"\n  a\n\n    b\n  \"\"\"",
        "\"\"\"x",
        "\"\"\"\n a\"\"\"b\n\"\"\"",
        "#\"\"\"\nr\n\"\"\"#",
        "#\"\"\"\n  r\n \"\"\"#",
        "\"\"\"\n  a \\\n b\n  \"\"\"",
        "\"\"\"\n  a\u{85}  b\n  \"\"\"",
        "/-",
        "/- ",
        "/-\n",
        "// c\n",
        "//",
        "// c",
        "/* c */",
        "/*",
        "*/",
        "/* /* */ */",
        "/**/",
        "/*/",
        "/* * / */",
        "(",
        ")",
        "(t)",
        "( t )",
        "(\"t\")",
        "(1)",
        "()",
        "\\",
        "\\\n",
        "\\ // c\n",
        "\\ x",
        "[",
        "]",
        "{",
        "}",
        "{}",
        "\u{202E}",
        "\u{FEFF}",
        "\u{0}",
        "\u{1B}",
        "\"\"\"\r\n  a\r\n  \"\"\"",
        "\"\"\"\n  a\n\\ \"\"\"",
        "#\"\"\"\n\"\"\"#",
        "\"\"\"\n\"\"\"",
        "\"\\\n\"",
        "\"\\ \"",
        "\"\\u{10FFFF}\"",
        "\"\\u{110000}\"",
        "\"\\u{}\"",
        "\"\\u41\"",
        "#\"\"\"\n a\n\\ \"\"\"#",
        "##\"\"\"\n x\n \"\"\"#\n \"\"\"##",
        "\"\"\"\n a \"\" b\n \"\"\"",
        "\"\"\"\n  a\n\t\n  \"\"\"",
        "\"\"\"\n \\\n \"\"\"",
        "#\"a\"##",
        "#\"\"#",
    ];

    /// Random pieces, mostly invalid, or flat documents with at most one random piece spliced in.
    fn soup() -> impl Strategy<Value = String> {
        prop_oneof![
            prop::collection::vec(prop::sample::select(PIECES), 0..14).prop_map(|p| p.concat()),
            flat_document(),
        ]
    }

    /// Digits in base 10 with the odd `_`, up to `max` long: long enough to pass `i128`.
    fn digits(max: usize) -> impl Strategy<Value = String> {
        let digit = prop_oneof![9 => prop::char::range('0', '9'), 1 => Just('_')];
        (
            prop::char::range('0', '9'),
            prop::collection::vec(digit, 0..max),
        )
            .prop_map(|(first, rest)| std::iter::once(first).chain(rest).collect())
    }

    /// Numbers in every KDL form: signs, `_`, fractions, exponents, other bases, and digit groups
    /// around the size of an `i128`.
    fn number() -> impl Strategy<Value = String> {
        let sign = prop::sample::select(&["", "", "-", "+"][..]);
        let decimal = (
            sign.clone(),
            prop_oneof![3 => digits(4), 1 => digits(42)],
            prop::option::of(prop_oneof![3 => digits(4), 1 => digits(42)]),
            prop::option::of((
                prop::sample::select(&["e", "E", "e-", "E+", "e+"][..]),
                prop_oneof![3 => digits(3), 1 => digits(42)],
            )),
        )
            .prop_map(|(sign, integer, fraction, exponent)| {
                let fraction = fraction.map(|f| format!(".{f}")).unwrap_or_default();
                let exponent = exponent.map(|(e, d)| format!("{e}{d}")).unwrap_or_default();
                format!("{sign}{integer}{fraction}{exponent}")
            });
        let based = (
            sign,
            prop::sample::select(&["0x", "0X", "0o", "0b", "0B"][..]),
            prop::collection::vec(
                prop::sample::select(&["0", "1", "7", "9", "a", "F", "_"][..]),
                1..36,
            ),
        )
            .prop_map(|(sign, base, d)| format!("{sign}{base}{}", d.concat()));
        let float = any::<f64>().prop_map(|f| {
            if f.is_finite() {
                format!("{f:?}")
            } else {
                "1e309".to_owned()
            }
        });
        prop_oneof![3 => decimal, 1 => based, 1 => float]
    }

    /// Quoted strings with every kind of escape, and characters KDL treats specially.
    fn quoted() -> impl Strategy<Value = String> {
        let piece = prop_oneof![
            // Characters that cannot be written as themselves become `\u{...}` escapes.
            4 => any::<char>().prop_map(|c| {
                if is_disallowed(c) || is_newline(c) || c == '"' || c == '\\' {
                    format!("\\u{{{:X}}}", u32::from(c))
                } else {
                    c.to_string()
                }
            }),
            1 => prop::sample::select(
                &[
                    "\\n", "\\t", "\\r", "\\\"", "\\\\", "\\s", "\\b", "\\f", "\\u{0}",
                    "\\u{1F600}", "\\u{10FFFF}", "\\  \n\t ", "\\\n", " ", "é", "{", "/*",
                ][..]
            )
            .prop_map(str::to_owned),
        ];
        prop::collection::vec(piece, 0..8).prop_map(|p| format!("\"{}\"", p.concat()))
    }

    /// Multi-line strings, quoted or raw, with random indentation, blank lines and line breaks.
    fn multi_line() -> impl Strategy<Value = String> {
        let indent = prop::sample::select(&["", " ", "  ", "\t", " \u{A0}"][..]);
        let line = prop::sample::select(
            &[
                "a",
                "b c ",
                "\"q\"",
                "é日",
                "\\n",
                "x \\\n  y",
                "",
                "  ",
                "\\s",
                "\"\"x",
            ][..],
        );
        let newline = prop::sample::select(&["\n", "\r\n", "\u{85}", "\u{2028}"][..]);
        (
            any::<bool>(),
            indent,
            prop::collection::vec((line, any::<bool>(), newline.clone()), 0..5),
            newline,
        )
            .prop_map(|(raw, indent, lines, newline)| {
                let (open, close) = if raw {
                    ("#\"\"\"", "\"\"\"#")
                } else {
                    ("\"\"\"", "\"\"\"")
                };
                let mut s = format!("{open}{newline}");
                for (line, extra, end) in lines {
                    let extra = if extra { " " } else { "" };
                    write!(s, "{indent}{extra}{line}{end}").unwrap();
                }
                format!("{s}{indent}{close}")
            })
    }

    fn flat_document() -> impl Strategy<Value = String> {
        let name = || {
            prop_oneof![
                4 => prop::sample::select(
                    &[
                        "a",
                        "node",
                        "x",
                        "\"k\"",
                        "#\"r\"#",
                        "-",
                        ".a",
                        "é",
                        "\"\"\"\n  n\n  \"\"\"",
                    ][..],
                )
                .prop_map(str::to_owned),
                1 => quoted(),
            ]
        };
        let value = prop_oneof![
            4 => prop::sample::select(
                &[
                    "1",
                    "-1.5e-3",
                    "0x1F",
                    "#true",
                    "#false",
                    "#null",
                    "#nan",
                    "#inf",
                    "#-inf",
                    "\"s\"",
                    "\"\\u{41}\\n\"",
                    "##\"a\"#\"##",
                    "\"\"\"\n  v\n  \"\"\"",
                    "#\"\"\"\n r\n \"\"\"#",
                    "b",
                    "-inf",
                    "(t)1",
                ][..],
            )
            .prop_map(str::to_owned),
            3 => number(),
            2 => quoted(),
            1 => multi_line(),
        ];
        let space = prop::sample::select(
            &[
                " ",
                "\t",
                " /* c */ ",
                " \\\n ",
                " \\ // c\n ",
                "  ",
                "/* * */",
                "\u{3000}",
                " \\\r\n\u{A0}",
            ][..],
        );
        let end = prop::sample::select(
            &[
                "\n", ";", "\r\n", " // c\n", "\u{85}", "\n\n", "; ", "\u{2029}", "\u{C}",
            ][..],
        );
        let entry = (any::<bool>(), any::<bool>(), name(), value, space).prop_map(
            |(dash, property, key, value, space)| {
                let dash = if dash { "/-" } else { "" };
                let key = if property {
                    format!("{key}=")
                } else {
                    String::new()
                };
                format!("{space}{dash}{key}{value}")
            },
        );
        let node = (
            any::<bool>(),
            name(),
            prop::collection::vec(entry, 0..5),
            end,
        )
            .prop_map(|(dash, name, entries, end)| {
                format!(
                    "{}{name}{}{end}",
                    if dash { "/-" } else { "" },
                    entries.concat()
                )
            });
        let junk = prop::option::of((any::<prop::sample::Index>(), prop::sample::select(PIECES)));
        (prop::collection::vec(node, 0..5), junk).prop_map(|(nodes, junk)| {
            let mut doc = nodes.concat();
            if let Some((at, piece)) = junk {
                let mut at = at.index(doc.len() + 1);
                while !doc.is_char_boundary(at) {
                    at -= 1;
                }
                doc.insert_str(at, piece);
            }
            doc
        })
    }

    /// True when a keyword such as `#true` runs straight into another token.
    fn has_glued_keyword(src: &str) -> bool {
        ["#true", "#false", "#null", "#inf", "#-inf", "#nan"]
            .iter()
            .any(|k| {
                src.match_indices(k).any(|(i, k)| {
                    src[i + k.len()..]
                        .chars()
                        .next()
                        .is_some_and(|c| !(is_space(c) || is_newline(c) || c == ';'))
                })
            })
    }

    /// What kdl reads of the text before a children block whose `{` is at byte `brace`: up to
    /// the brace, or, when that ends in a slashdash that drops the block, up to the slashdash.
    fn kdl_before(src: &str, brace: usize) -> Result<KdlDocument, kdl::KdlError> {
        KdlDocument::parse_v2(&src[..brace]).or_else(|e| {
            src[..brace]
                .rfind("/-")
                .map_or(Err(e), |dash| KdlDocument::parse_v2(&src[..dash]))
        })
    }

    /// The parser and kdl agree on `src`: whatever the parser reads, kdl reads the same, values
    /// included, and whatever flat KDL kdl reads, the parser reads too. The exceptions are
    /// slashdash chains, which the parser rejects, and keywords glued to what follows: kdl does
    /// not check the end of a keyword, so it reads `/-a #true"s"` as a dropped node `a #true`,
    /// then a node `"s"`.
    fn agree_with_kdl(src: &str) -> Result<(), TestCaseError> {
        let ours = read(src);
        match &ours {
            Ok(Read::All(nodes)) => {
                let kdl = KdlDocument::parse_v2(src);
                prop_assert!(kdl.is_ok(), "the parser read {:?}, kdl: {:?}", src, kdl);
                if let Ok(kdl) = kdl {
                    prop_assert_eq!(outline(nodes), outline_kdl(&kdl), "{:?}", src);
                }
            }
            Ok(Read::Children(nodes, brace)) => {
                let kdl = kdl_before(src, *brace);
                prop_assert!(kdl.is_ok(), "the parser read {:?}, kdl: {:?}", src, kdl);
                if let Ok(kdl) = kdl {
                    prop_assert_eq!(outline(nodes), outline_kdl(&kdl), "{:?}", src);
                }
            }
            Ok(Read::Cut(_)) | Err(_) => {}
        }
        if KdlDocument::parse_v2(src).is_ok() && !has_glued_keyword(src) {
            match ours {
                Ok(_) => {}
                Err(Stop::Error(FormatError::Syntax { message, .. }))
                    if message.contains("`/-` cannot follow") => {}
                Err(e) => prop_assert!(false, "kdl reads {:?}, the parser stopped: {:?}", src, e),
            }
        }
        Ok(())
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(cases(4096)))]

        #[test]
        fn parser_agrees_with_kdl(src in soup()) {
            agree_with_kdl(&src)?;
        }

        #[test]
        fn parser_reads_flat_documents_as_kdl_does(src in flat_document()) {
            agree_with_kdl(&src)?;
        }
    }
}
