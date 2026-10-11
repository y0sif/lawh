//! A linear check of a `.lawh` file's KDL structure, run before the `kdl` crate parses it.
//!
//! kdl's parser recurses once per nested children block, nested block comment, token inside a
//! block comment, chained slashdash and error it recovers from: a few kilobytes of `{`, `/*`,
//! `)` or `a=` overflow a 2 MiB stack. lawh files are flat, so this pass walks the text once,
//! without recursion, following the KDL v2 grammar as kdl implements it, and accepts only flat
//! documents:
//!
//! - Nodes have no children: the scan stops at the first `{` outside strings and comments.
//! - Block comments nest at most [`MAX_COMMENT_DEPTH`] deep.
//! - A slashdash `/-` may not apply to another slashdash.
//! - At most [`MAX_OBJECTS`] nodes follow the first (the header): the scan stops at the first
//!   node past them, so kdl never reads more objects than a board holds.
//! - Anything kdl would reject stops the scan. kdl then reads only the text before it plus the
//!   one bad token, to word the error, so its error recovery has nothing to recurse over.
//!
//! The scan rejects a few exotic forms that kdl accepts (`/- /-`, a keyword running straight
//! into the next token), and it never accepts text kdl rejects. Property tests below compare the
//! two on random text.
//!
//! Once checked, everything kdl would skip is blanked: comments, slashdashed nodes and entries,
//! line continuations, runs of spaces, and the newlines between nodes. Each run of them becomes
//! an empty block comment `/*   */` of the same length (spaces when shorter than `/**/`), so kdl
//! reads the nodes the file keeps, each ended by its own `;` or newline, and empty comments.
//!
//! kdl is slow on filler. In release it takes about 1.2 us a byte of line comment, 2.1 to 4 us a
//! byte of slashdashed nodes or properties and 0.4 us a newline. It reads spaces at 0.03 us a
//! byte between nodes and 0.14 us after a node's last entry, and the inside of a block comment
//! about five times faster, without the recursion once per `*` and `/` it does on a written
//! comment. Byte offsets do not change, and line and column are always counted in the source, so
//! error positions stay exact.

use std::borrow::Cow;

use super::read::line_column;
use super::{FormatError, MAX_COMMENT_DEPTH};
use crate::board::MAX_OBJECTS;

/// The most bytes of a bad token handed to kdl to word an error.
const MAX_BAD_TOKEN: usize = 1024;

/// What [`scan`] found.
pub(super) struct Scan<'a> {
    /// The text kdl reads: the source with everything kdl would skip blanked (see the module
    /// docs). Byte offsets are the same as in the source.
    pub(super) text: Cow<'a, str>,
    /// Why the scan stopped before the end, if it did.
    pub(super) stop: Option<Stop>,
}

/// Why the scan stopped before the end of the file.
#[derive(Debug, PartialEq)]
pub(super) enum Stop {
    /// A `{` at byte `brace` opens a children block. kdl reads `text[..cut]`, which ends before
    /// the brace (or before the slashdash that drops it).
    ///
    /// `dropped_owner` holds the name, as written, of the node the block belongs to when that
    /// node is slashdashed, so kdl drops it. Otherwise the owner is the last node kdl reads.
    Children {
        brace: usize,
        cut: usize,
        dropped_owner: Option<String>,
    },
    /// The node at byte `at` is the first past [`MAX_OBJECTS`] objects, the header not counted.
    /// kdl reads `text[..at]`, so the objects before it are checked first.
    TooManyObjects { at: usize },
    /// Text kdl would reject, starting at byte `at`. kdl reads `text[..end]`, which ends after
    /// the bad token, to word the error; `message` is used if kdl accepts it.
    Rejected {
        at: usize,
        end: usize,
        message: &'static str,
    },
    /// An error the scan words itself.
    Error(FormatError),
}

/// Checks the structure of `src` (see the module docs).
pub(super) fn scan(src: &str) -> Scan<'_> {
    let mut scanner = Scanner {
        src,
        pos: 0,
        blanks: Vec::new(),
    };
    let stop = scanner.document().err();
    Scan {
        text: blank(src, &scanner.blanks),
        stop,
    }
}

/// Blanks the given ranges of `src`, which are in order, do not overlap, and start and end on
/// character boundaries: each becomes an empty block comment `/*   */` of the same length, or
/// spaces when shorter than `/**/`.
fn blank<'a>(src: &'a str, ranges: &[(usize, usize)]) -> Cow<'a, str> {
    let bytes = src.as_bytes();
    let unchanged = |&(start, end): &(usize, usize)| {
        end.saturating_sub(start) < "/**/".len()
            && bytes
                .get(start..end)
                .is_none_or(|r| r.iter().all(|&b| b == b' '))
    };
    if ranges.iter().all(unchanged) {
        return Cow::Borrowed(src);
    }
    let mut text = String::with_capacity(src.len());
    let mut from = 0;
    for &(start, end) in ranges {
        text.push_str(src.get(from..start).unwrap_or_default());
        let len = end.saturating_sub(start);
        match len.checked_sub("/**/".len()) {
            Some(inside) => {
                text.push_str("/*");
                text.extend(std::iter::repeat_n(' ', inside));
                text.push_str("*/");
            }
            None => text.extend(std::iter::repeat_n(' ', len)),
        }
        from = end;
    }
    text.push_str(src.get(from..).unwrap_or_default());
    Cow::Owned(text)
}

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
    !(is_space(c)
        || is_newline(c)
        || is_disallowed(c)
        || matches!(
            c,
            '=' | '\\' | '/' | '(' | ')' | '{' | '}' | '[' | ']' | ';' | '"' | '#'
        ))
}

/// What a bare word is to kdl.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Word {
    /// An identifier string, such as `c1`, `-` or `.a`.
    Ident,
    /// A number, such as `3`, `-0.5`, `1e300` or `0x1F`.
    Number,
    /// Neither, such as `1.`, `.5`, `1a`, `true` or an integer past `i128`.
    Invalid,
}

fn classify(word: &str) -> Word {
    let unsigned = word.strip_prefix(['+', '-']).unwrap_or(word);
    let mut chars = unsigned.chars();
    match (chars.next(), chars.next()) {
        (Some(c), _) if c.is_ascii_digit() => {
            if is_number(unsigned) {
                Word::Number
            } else {
                Word::Invalid
            }
        }
        (Some('.'), Some(c)) if c.is_ascii_digit() => Word::Invalid,
        _ if matches!(word, "true" | "false" | "null" | "inf" | "nan") => Word::Invalid,
        _ => Word::Ident,
    }
}

/// An unsigned KDL v2 number, with every digit group fitting in `i128` as kdl requires.
fn is_number(s: &str) -> bool {
    let radix = match s.get(..2) {
        Some("0x" | "0X") => 16,
        Some("0o" | "0O") => 8,
        Some("0b" | "0B") => 2,
        _ => return is_decimal(s),
    };
    is_digits(s.get(2..).unwrap_or_default(), radix)
}

/// `digit (digit | '_')*` in `radix`, with a value that fits in `i128`.
fn is_digits(s: &str, radix: u32) -> bool {
    s.starts_with(|c: char| c.is_digit(radix))
        && s.chars().all(|c| c == '_' || c.is_digit(radix))
        && i128::from_str_radix(&s.replace('_', ""), radix).is_ok()
}

/// Splits `s` after its leading digits and underscores.
fn split_digits(s: &str) -> (&str, &str) {
    let end = s
        .find(|c: char| !(c == '_' || c.is_ascii_digit()))
        .unwrap_or(s.len());
    s.split_at(end)
}

/// `integer ('.' integer)? (('e' | 'E') sign? integer)?`
fn is_decimal(s: &str) -> bool {
    let split = split_digits;
    let (integer, mut rest) = split(s);
    if !is_digits(integer, 10) {
        return false;
    }
    if let Some(after) = rest.strip_prefix('.') {
        let (fraction, after) = split(after);
        if !is_digits(fraction, 10) {
            return false;
        }
        rest = after;
    }
    if let Some(after) = rest.strip_prefix(['e', 'E']) {
        let (exponent, after) = split(after.strip_prefix(['+', '-']).unwrap_or(after));
        if !is_digits(exponent, 10) {
            return false;
        }
        rest = after;
    }
    rest.is_empty()
}

type Step<T = ()> = Result<T, Stop>;

struct Scanner<'a> {
    src: &'a str,
    /// Byte offset of the next character.
    pos: usize,
    /// Byte ranges kdl skips, to blank: in order, not overlapping, and merged when nothing but
    /// spaces lies between two of them.
    blanks: Vec<(usize, usize)>,
}

/// Where the scanner was, to go back to: its position, how many ranges it had to blank and the
/// last of them, which [`Scanner::blank`] may have extended since.
struct Mark {
    pos: usize,
    blanks: usize,
    last: Option<(usize, usize)>,
}

impl<'a> Scanner<'a> {
    fn rest(&self) -> &'a str {
        self.src.get(self.pos..).unwrap_or_default()
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn peek_second(&self) -> Option<char> {
        self.rest().chars().nth(1)
    }

    fn starts(&self, s: &str) -> bool {
        self.rest().starts_with(s)
    }

    fn at_end(&self) -> bool {
        self.pos >= self.src.len()
    }

    fn mark(&self) -> Mark {
        Mark {
            pos: self.pos,
            blanks: self.blanks.len(),
            last: self.blanks.last().copied(),
        }
    }

    /// Goes back to `mark`, dropping the ranges to blank found since.
    fn reset(&mut self, mark: &Mark) {
        self.pos = mark.pos;
        self.blanks.truncate(mark.blanks);
        if let (Some(last), Some(saved)) = (self.blanks.last_mut(), mark.last) {
            *last = saved;
        }
    }

    /// Blanks `from..pos`, which replaces the ranges found since there were `count`. Spaces
    /// between it and the range before are blanked too, so kdl reads one comment, not many.
    fn blank(&mut self, from: usize, count: usize) {
        self.blanks.truncate(count);
        if from >= self.pos {
            return;
        }
        match self.blanks.last_mut() {
            Some(last)
                if last.1 >= from
                    || self
                        .src
                        .get(last.1..from)
                        .is_some_and(|gap| gap.chars().all(is_space)) =>
            {
                last.1 = last.1.max(self.pos);
            }
            _ => self.blanks.push((from, self.pos)),
        }
    }

    /// Length of the newline at byte `at`, if there is one. CRLF counts as one newline.
    fn newline_at(&self, at: usize) -> Option<usize> {
        let rest = self.src.get(at..)?;
        if rest.starts_with("\r\n") {
            return Some(2);
        }
        rest.chars()
            .next()
            .filter(|&c| is_newline(c))
            .map(char::len_utf8)
    }

    /// A syntax error worded here, at byte `at`.
    fn error(&self, at: usize, message: &str, help: Option<&str>) -> Stop {
        let (line, column) = line_column(self.src, at);
        Stop::Error(FormatError::Syntax {
            line,
            column,
            message: message.to_owned(),
            help: help.map(str::to_owned),
        })
    }

    fn forbidden(&self, at: usize, c: char) -> Stop {
        let code = u32::from(c);
        self.error(
            at,
            &format!("character U+{code:04X} cannot appear as itself in a lawh file"),
            Some(&format!("inside a string, write it as \\u{{{code:X}}}")),
        )
    }

    /// Text kdl would reject at byte `at`. kdl words the error from the token there: a bare
    /// word (with its leading `#`, if any) or a single character.
    fn reject(&self, at: usize, message: &'static str) -> Stop {
        let rest = self.src.get(at..).unwrap_or_default();
        let len = match rest.chars().next() {
            None => 0,
            Some(c) if is_disallowed(c) => return self.forbidden(at, c),
            Some(c) if c == '#' || is_word_char(c) => rest
                .char_indices()
                .skip(1)
                .find(|&(_, c)| !is_word_char(c))
                .map_or(rest.len(), |(i, _)| i),
            Some(c) => c.len_utf8(),
        };
        let mut end = at + len.min(MAX_BAD_TOKEN);
        while !self.src.is_char_boundary(end) {
            end -= 1;
        }
        Stop::Rejected { at, end, message }
    }

    /// `bom? nodes`, with each node checked in turn, up to the header and [`MAX_OBJECTS`] objects.
    fn document(&mut self) -> Step {
        if self.peek() == Some('\u{FEFF}') {
            self.pos += '\u{FEFF}'.len_utf8();
        }
        let mut nodes = 0;
        loop {
            self.leading()?;
            if self.at_end() {
                return Ok(());
            }
            if nodes > MAX_OBJECTS {
                return Err(Stop::TooManyObjects { at: self.pos });
            }
            nodes += 1;
            self.node(None)?;
            if !(self.at_end() || self.terminator()) {
                return Err(self.reject(self.pos, "expected the end of the node"));
            }
        }
    }

    /// Line space and slashdashed nodes before a node, all blanked: newlines between nodes are
    /// line space, and the node before, if any, has already read its own terminator.
    fn leading(&mut self) -> Step {
        loop {
            let (from, count) = (self.pos, self.blanks.len());
            if self.line_space()? {
                self.blank(from, count);
                continue;
            }
            if !self.starts("/-") {
                return Ok(());
            }
            self.pos += 2;
            while self.line_space()? {}
            self.node(Some(from))?;
            self.blank(from, count);
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

    /// A node: its type annotation, name and entries. `dash` is where the slashdash before it
    /// starts, when it has one.
    fn node(&mut self, dash: Option<usize>) -> Step {
        if self.peek() == Some('(') {
            self.annotation()?;
            self.node_space()?;
        }
        let name = self.pos;
        if self.string_or_word()? != Some(Word::Ident) {
            return Err(self.reject(name, "expected a node name"));
        }
        let name = self.src.get(name..self.pos).unwrap_or_default();
        self.after_token(false)?;
        loop {
            let spaced = self.node_space()?;
            let children = |brace, cut| match dash {
                None => Stop::Children {
                    brace,
                    cut,
                    dropped_owner: None,
                },
                Some(dash) => Stop::Children {
                    brace,
                    cut: dash,
                    dropped_owner: Some(name.to_owned()),
                },
            };
            match self.peek() {
                None => return Ok(()),
                Some('{') => return Err(children(self.pos, self.pos)),
                Some(c) if is_newline(c) || c == ';' => return Ok(()),
                Some(_) if !spaced || self.starts("//") => return Ok(()),
                Some(_) if self.starts("/-") => {
                    let (entry_dash, count) = (self.pos, self.blanks.len());
                    self.pos += 2;
                    while self.line_space()? {}
                    if self.peek() == Some('{') {
                        return Err(children(self.pos, entry_dash));
                    }
                    if self.starts("/-") {
                        // kdl accepts this, recursing once per `/-`.
                        return Err(self.error(self.pos, "`/-` cannot follow another `/-`", None));
                    }
                    self.entry()?;
                    // Newlines included: the space before the entry keeps the node going.
                    self.blank(entry_dash, count);
                }
                Some(_) => self.entry()?,
            }
        }
    }

    /// An argument or property, after the space before it.
    fn entry(&mut self) -> Step {
        if self.peek() == Some('(') {
            self.annotation()?;
            self.node_space()?;
            return self.value();
        }
        let start = self.pos;
        match self.string_or_word()? {
            None => return self.value(),
            Some(Word::Number) => return self.after_token(false),
            Some(Word::Invalid) => return Err(self.reject(start, "expected a number or string")),
            Some(Word::Ident) => {}
        }
        // A string is a property name when `=` follows.
        self.after_token(true)?;
        let after = self.mark();
        self.node_space()?;
        if self.peek() != Some('=') {
            self.reset(&after);
            return Ok(());
        }
        self.pos += 1;
        self.node_space()?;
        if self.peek() == Some('(') {
            self.annotation()?;
            self.node_space()?;
        }
        self.value()
    }

    /// A value: a string, number or keyword such as `#true`.
    fn value(&mut self) -> Step {
        let start = self.pos;
        match self.string_or_word()? {
            Some(Word::Ident | Word::Number) => {}
            Some(Word::Invalid) => return Err(self.reject(start, "expected a number or string")),
            None => {
                let rest = self.rest();
                let keyword = ["true", "false", "null", "nan", "inf", "-inf"]
                    .into_iter()
                    .find(|k| rest.strip_prefix('#').is_some_and(|r| r.starts_with(k)));
                let Some(keyword) = keyword else {
                    return Err(self.reject(start, "expected a value"));
                };
                self.pos += 1 + keyword.len();
            }
        }
        self.after_token(false)
    }

    /// A type annotation `(name)`.
    fn annotation(&mut self) -> Step {
        self.pos += 1;
        self.node_space()?;
        let start = self.pos;
        if self.string_or_word()? != Some(Word::Ident) {
            return Err(self.reject(start, "expected a type name"));
        }
        self.node_space()?;
        if self.peek() != Some(')') {
            return Err(self.reject(self.pos, "expected `)`"));
        }
        self.pos += 1;
        Ok(())
    }

    /// Reads a quoted string, raw string or bare word if one starts here. A string counts as
    /// [`Word::Ident`]. Returns `None`, reading nothing, for anything else.
    fn string_or_word(&mut self) -> Step<Option<Word>> {
        match self.peek() {
            Some('"') => self.quoted().map(|()| Some(Word::Ident)),
            Some('#') if matches!(self.peek_second(), Some('#' | '"')) => {
                self.raw().map(|()| Some(Word::Ident))
            }
            Some(c) if is_word_char(c) => {
                let start = self.pos;
                let rest = self.rest();
                self.pos += rest.find(|c| !is_word_char(c)).unwrap_or(rest.len());
                Ok(Some(classify(
                    self.src.get(start..self.pos).unwrap_or_default(),
                )))
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
            Some(_) => Err(self.reject(self.pos, "expected a space here")),
        }
    }

    /// `"..."` or a multi-line `"""` string.
    fn quoted(&mut self) -> Step {
        let start = self.pos;
        if self.starts("\"\"\"") {
            self.pos += 3;
            let Some(newline) = self.newline_at(self.pos) else {
                return Err(self.error(
                    start,
                    "Multi-line string opening quotes must be immediately followed by a newline",
                    None,
                ));
            };
            self.pos += newline;
            return self.multi_line(start, None);
        }
        self.pos += 1;
        loop {
            match self.peek() {
                None => return Err(self.error(start, "unclosed string", None)),
                Some('"') => {
                    self.pos += 1;
                    return Ok(());
                }
                Some('\\') => self.escape()?,
                Some(c) if is_newline(c) => {
                    return Err(self.error(
                        start,
                        "Unexpected newline in single-line quoted string",
                        Some(
                            "You can make a string multi-line by wrapping it in '\"\"\"', with a \
                             newline immediately after the opening quotes.",
                        ),
                    ));
                }
                Some(c) if is_disallowed(c) => return Err(self.forbidden(self.pos, c)),
                Some(c) => self.pos += c.len_utf8(),
            }
        }
    }

    /// An escape in a quoted string, at its backslash.
    fn escape(&mut self) -> Step {
        let at = self.pos;
        self.pos += 1;
        match self.peek() {
            Some('\\' | '"' | 'b' | 'f' | 'n' | 'r' | 't' | 's') => self.pos += 1,
            Some('u') => {
                let rest = self.src.get(self.pos + 1..).unwrap_or_default();
                let hex = rest.strip_prefix('{').map(|r| {
                    let end = r.find(|c: char| !c.is_ascii_hexdigit()).unwrap_or(r.len());
                    (
                        r.get(..end).unwrap_or_default(),
                        r.get(end..).unwrap_or_default(),
                    )
                });
                let Some((hex, _)) = hex.filter(|(hex, after)| {
                    (1..=6).contains(&hex.len())
                        && after.starts_with('}')
                        && u32::from_str_radix(hex, 16)
                            .ok()
                            .and_then(char::from_u32)
                            .is_some()
                }) else {
                    return Err(self.invalid_escape(at));
                };
                self.pos += "u{}".len() + hex.len();
            }
            // A whitespace escape: the backslash and all the space and newlines after it.
            Some(c) if is_space(c) || is_newline(c) => {
                let rest = self.rest();
                self.pos += rest
                    .find(|c| !(is_space(c) || is_newline(c)))
                    .unwrap_or(rest.len());
            }
            _ => return Err(self.invalid_escape(at)),
        }
        Ok(())
    }

    fn invalid_escape(&self, at: usize) -> Stop {
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
    fn raw(&mut self) -> Step {
        let start = self.pos;
        let hashes = self.rest().bytes().take_while(|&b| b == b'#').count();
        self.pos += hashes;
        if self.peek() != Some('"') {
            return Err(self.reject(start, "expected `\"` after `#`"));
        }
        if self.starts("\"\"\"")
            && let Some(newline) = self.newline_at(self.pos + 3)
        {
            self.pos += 3 + newline;
            return self.multi_line(start, Some(hashes));
        }
        self.pos += 1;
        let body = self.pos;
        loop {
            match self.peek() {
                None => return Err(self.error(start, "unclosed raw string", None)),
                Some('"') if self.closes_raw(self.pos, hashes) => {
                    if self.src.get(body..self.pos) == Some("\"") {
                        return Err(self.error(
                            start,
                            "Single-line raw strings cannot look like multi-line ones",
                            None,
                        ));
                    }
                    self.pos += 1 + hashes;
                    return Ok(());
                }
                Some(c) if is_newline(c) => {
                    return Err(self.error(
                        start,
                        "Unexpected newline in single-line raw string",
                        None,
                    ));
                }
                Some(c) if is_disallowed(c) => return Err(self.forbidden(self.pos, c)),
                Some(c) => self.pos += c.len_utf8(),
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

    /// The body of a multi-line string, from the line after its opening quotes. `raw` is the
    /// number of `#` of a raw string.
    ///
    /// The closing quotes go on a line of their own, after nothing but spaces (and, in a quoted
    /// string, whitespace escapes). Every other line starts with those spaces or holds nothing
    /// but spaces.
    fn multi_line(&mut self, start: usize, raw: Option<usize>) -> Step {
        let mut lines = Vec::new();
        loop {
            let line = self.pos;
            let rest = self.rest();
            let indent = line + rest.find(|c| !is_space(c)).unwrap_or(rest.len());
            // A quoted string's closing line may also hold whitespace escapes after the indent.
            let mut quotes = indent;
            while raw.is_none()
                && self.src.get(quotes..).is_some_and(|r| {
                    r.strip_prefix('\\')
                        .is_some_and(|r| r.starts_with(|c| is_space(c) || is_newline(c)))
                })
            {
                let after = self.src.get(quotes + 1..).unwrap_or_default();
                quotes += 1 + after
                    .find(|c| !(is_space(c) || is_newline(c)))
                    .unwrap_or(after.len());
            }
            if self.closes_multi_line(quotes, raw) {
                let prefix = self.src.get(line..indent).unwrap_or_default();
                for &line in &lines {
                    let text = self.src.get(line..).unwrap_or_default();
                    let blank = text
                        .find(|c| !is_space(c))
                        .is_some_and(|i| self.newline_at(line + i).is_some());
                    if !(text.starts_with(prefix) || blank) {
                        return Err(self.error(
                            line,
                            "this line of a multi-line string does not start with the \
                             whitespace before its closing quotes",
                            None,
                        ));
                    }
                }
                self.pos = quotes + 3 + raw.unwrap_or(0);
                return Ok(());
            }
            lines.push(line);
            loop {
                match self.peek() {
                    None => {
                        return Err(self.error(
                            start,
                            "unclosed multi-line string",
                            Some("close it with `\"\"\"` on a line of its own"),
                        ));
                    }
                    Some(c) if is_newline(c) => {
                        self.pos += self.newline_at(self.pos).unwrap_or(1);
                        break;
                    }
                    Some('\\') if raw.is_none() => self.escape()?,
                    Some('"') if self.closes_multi_line(self.pos, raw) => {
                        return Err(self.error(
                            self.pos,
                            "the closing quotes of a multi-line string must be on a line of \
                             their own",
                            None,
                        ));
                    }
                    Some(c) if is_disallowed(c) => return Err(self.forbidden(self.pos, c)),
                    Some(c) => self.pos += c.len_utf8(),
                }
            }
        }
    }

    /// node-space*: spaces, block comments and line continuations. True if it read any.
    ///
    /// The run is blanked whole once it is as long as `/**/`: inside a node, kdl reads a run of
    /// spaces several times over.
    fn node_space(&mut self) -> Step<bool> {
        let (start, count) = (self.pos, self.blanks.len());
        loop {
            match self.peek() {
                Some(c) if is_space(c) => self.pos += c.len_utf8(),
                Some('/') if self.starts("/*") => self.block_comment()?,
                Some('\\') => self.line_continuation()?,
                _ => break,
            }
        }
        if self.pos - start >= "/**/".len() {
            self.blank(start, count);
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

    /// `\` then spaces and comments up to the end of the line (kdl's escline), blanked with its
    /// newline: it is node space, as spaces are.
    fn line_continuation(&mut self) -> Step {
        let (start, count) = (self.pos, self.blanks.len());
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
            return Err(self.reject(self.pos, "expected a newline after `\\`"));
        }
        self.blank(start, count);
        Ok(())
    }

    /// `//` through the next newline, or to the end of the file. The comment is blanked, its
    /// newline is not: it may end a node.
    fn line_comment(&mut self) {
        let (start, count) = (self.pos, self.blanks.len());
        match self.rest().find(is_newline) {
            None => {
                self.pos = self.src.len();
                self.blank(start, count);
            }
            Some(i) => {
                self.pos += i;
                self.blank(start, count);
                self.pos += self.newline_at(self.pos).unwrap_or(1);
            }
        }
    }

    /// `/* ... */`, nested at most [`MAX_COMMENT_DEPTH`] deep. Blanked with any newlines inside:
    /// it is node space, as spaces are.
    fn block_comment(&mut self) -> Step {
        let (open, count) = (self.pos, self.blanks.len());
        self.pos += 2;
        let mut depth = 1;
        loop {
            let Some(i) = self.rest().find(['*', '/']) else {
                return Err(self.error(open, "unclosed block comment", Some("close it with `*/`")));
            };
            self.pos += i;
            if self.starts("*/") {
                self.pos += 2;
                depth -= 1;
                if depth == 0 {
                    self.blank(open, count);
                    return Ok(());
                }
            } else if self.starts("/*") {
                if depth == MAX_COMMENT_DEPTH {
                    let (line, column) = line_column(self.src, self.pos);
                    return Err(Stop::Error(FormatError::CommentTooDeep { line, column }));
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
    use kdl::KdlDocument;
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn words() {
        for (word, kind) in [
            ("c1", Word::Ident),
            ("-", Word::Ident),
            ("+x", Word::Ident),
            ("-inf", Word::Ident),
            (".a", Word::Ident),
            ("-.", Word::Ident),
            ("_1", Word::Ident),
            ("true", Word::Invalid),
            ("nan", Word::Invalid),
            ("0", Word::Number),
            ("-1_000", Word::Number),
            ("1.5e-3", Word::Number),
            ("1E+300", Word::Number),
            ("1e999", Word::Number),
            ("0x1F_ff", Word::Number),
            ("-0o17", Word::Number),
            ("0b1_0", Word::Number),
            ("5e-324", Word::Number),
            ("170141183460469231731687303715884105727", Word::Number),
            ("170141183460469231731687303715884105728", Word::Invalid),
            ("1.", Word::Invalid),
            (".5", Word::Invalid),
            ("-.5", Word::Invalid),
            ("1._5", Word::Invalid),
            ("1e", Word::Invalid),
            ("1e+", Word::Invalid),
            ("1.5.5", Word::Invalid),
            ("1a", Word::Invalid),
            ("0x", Word::Invalid),
            ("0xg", Word::Invalid),
            ("0o8", Word::Invalid),
            ("0b2", Word::Invalid),
        ] {
            assert_eq!(classify(word), kind, "{word}");
        }
    }

    /// Comments, slashdashed nodes and entries, line continuations, long runs of spaces and the
    /// newlines between nodes become empty block comments of the same length, merged across
    /// spaces. Each kept node keeps its terminator.
    #[test]
    fn filler_is_blanked_in_place() {
        for (src, text) in [
            (
                "a /* x * y / z /* in */ */ b // c * /\n",
                "a/*                      */b /*    */\n",
            ),
            (
                "// c\n/-a 1\n\n/- b \"s\"\r\nc /-x=1 /-\n 2 3; d \\ // e\n f\n/-g",
                "/*                  */c /*       */ 3; d/*     */f\n   ",
            ),
            (
                "\u{FEFF}// é\r\na // d\r\nb /*\n*/ c",
                "\u{FEFF}/*   */a /**/\r\nb/*   */c",
            ),
            ("a /-x=1 /-y=2 b", "a /*       */ b"),
        ] {
            let scan = scan(src);
            assert_eq!(scan.stop, None, "{src:?}");
            assert_eq!(scan.text, text, "{src:?}");
            let (kept, original) = (
                KdlDocument::parse_v2(&scan.text),
                KdlDocument::parse_v2(src),
            );
            assert_eq!(
                outline(&kept.unwrap()),
                outline(&original.unwrap()),
                "{src:?}"
            );
        }
        assert!(matches!(scan("a b").text, Cow::Borrowed(_)));
        assert!(matches!(scan("a\n   b").text, Cow::Borrowed(_)));
    }

    /// Ranges to blank merge, so filler between nodes, or after a node's name, blanks as one
    /// range, and kdl reads one comment.
    #[test]
    fn filler_is_one_range() {
        let blanks = |src: &str| {
            let mut scanner = Scanner {
                src,
                pos: 0,
                blanks: Vec::new(),
            };
            assert_eq!(scanner.document(), Ok(()));
            scanner.blanks
        };
        let between = format!("a\n{}b", "// c\n/-d 1\n/* e */\n\n".repeat(1000));
        assert_eq!(blanks(&between), [(2, between.len() - 1)]);
        let in_node = format!("a{} // c\nb", " /-x=0 \\\n /- 1     /* e */".repeat(1000));
        assert_eq!(blanks(&in_node), [(2, in_node.len() - 2)]);
    }

    /// Flat KDL that hand-written files may use.
    #[test]
    fn flat_kdl_is_accepted() {
        for src in [
            "",
            "\u{FEFF}lawh version=1",
            "a; b;c\r\nd\u{85}e\u{2028}f",
            "a x=1 \"y\"=2 #\"z\"#=#true w = #-inf",
            "a #\"raw \"quoted\" \\n\"# ##\"a #\"b\"# c\"##",
            "a \"\"\"\n    line one\n      line two \"q\"\n\n    \"\"\"",
            "a #\"\"\"\n  raw \\n multi\n  \"\"\"#",
            "a \"\"\"\n  one \\\n    two\n  \"\"\" b",
            "a \"x\\ \n   y\" \"\\u{1F600}\\s\\t\"",
            "/-a 1\n/- \n b 2\nc /-x /- y=1 z",
            "a \\\n  b \\ // note\n  c",
            "a /* x */ b/**/c=/* y */1",
            "(t)a (u)1 x=(v)2 ( w )\"s\"",
            "a -1 +2 0x1F 0o7 0b1 1_000.5e-3 -inf .x - _",
            "// only a comment",
            "/* only a comment */",
            "a \u{7F}",
        ] {
            let scan = scan(src);
            assert_eq!(scan.stop, None, "{src:?}");
            assert!(KdlDocument::parse_v2(&scan.text).is_ok(), "{src:?}");
        }
    }

    #[test]
    fn children_blocks_stop_the_scan() {
        let stop = |src| scan(src).stop;
        assert_eq!(
            stop("a\nb x=1 {\n}"),
            Some(Stop::Children {
                brace: 8,
                cut: 8,
                dropped_owner: None
            })
        );
        assert_eq!(
            stop("b{}"),
            Some(Stop::Children {
                brace: 1,
                cut: 1,
                dropped_owner: None
            })
        );
        assert_eq!(
            stop("b /-\n {}"),
            Some(Stop::Children {
                brace: 6,
                cut: 2,
                dropped_owner: None
            })
        );
        assert_eq!(
            stop("a\n/-b {}"),
            Some(Stop::Children {
                brace: 6,
                cut: 2,
                dropped_owner: Some("b".to_owned())
            })
        );
        // Braces inside strings and comments are text.
        assert_eq!(stop("a \"{\" #\"}\"# /* { */ // {"), None);
    }

    #[test]
    fn objects_past_the_limit_stop_the_scan() {
        // The header and as many objects as a board holds.
        let full = "c\n".repeat(MAX_OBJECTS + 1);
        assert_eq!(scan(&full).stop, None);
        // Slashdashed nodes and comments don't count; the next node does.
        let over = format!("{full}/-c\n// c\n  c x=1 {{");
        assert_eq!(
            scan(&over).stop,
            Some(Stop::TooManyObjects {
                at: full.len() + "/-c\n// c\n  ".len()
            })
        );
    }

    #[test]
    fn bad_tokens_end_what_kdl_reads() {
        let rejected = |src| match scan(src).stop {
            Some(Stop::Rejected { at, end, .. }) => (at, end),
            other => panic!("{src:?}: {other:?}"),
        };
        assert_eq!(rejected("a ))))"), (2, 3));
        assert_eq!(rejected("a x=1.5.5 y"), (4, 9));
        assert_eq!(rejected("a #truex"), (7, 8));
        assert_eq!(rejected("a=1"), (1, 2));
        assert_eq!(rejected("}}}}"), (0, 1));
        let long = format!("a x={}", "1".repeat(5000));
        assert_eq!(rejected(&long), (4, 4 + MAX_BAD_TOKEN));
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
        "1.",
        ".5",
        "1e",
        "0x",
        "1a",
        "0b2",
        "1.5.5",
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
        "\"\\u{41}\"",
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
        "\"\"\"x",
        "\"\"\"\n a\"\"\"b\n\"\"\"",
        "#\"\"\"\nr\n\"\"\"#",
        "#\"\"\"\n  r\n \"\"\"#",
        "\"\"\"\n  a \\\n b\n  \"\"\"",
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

    fn flat_document() -> impl Strategy<Value = String> {
        let name = || {
            prop::sample::select(
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
        };
        let value = prop::sample::select(
            &[
                "1",
                "-1.5e-3",
                "0x1F",
                "#true",
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
        );
        let space = prop::sample::select(
            &[
                " ",
                "\t",
                " /* c */ ",
                " \\\n ",
                " \\ // c\n ",
                "  ",
                "/* * */",
            ][..],
        );
        let end = prop::sample::select(&["\n", ";", "\r\n", " // c\n", "\u{85}", "\n\n", "; "][..]);
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
            prop::collection::vec(entry, 0..4),
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

    /// What kdl read, without formatting: node names, types and entries.
    fn outline(doc: &KdlDocument) -> String {
        format!(
            "{:?}",
            doc.nodes()
                .iter()
                .map(|n| {
                    let entries: Vec<_> = n
                        .entries()
                        .iter()
                        .map(|e| {
                            (
                                e.name().map(|k| k.value().to_owned()),
                                e.ty().is_some(),
                                e.value(),
                            )
                        })
                        .collect();
                    (
                        n.name().value(),
                        n.ty().is_some(),
                        entries,
                        n.children().is_some(),
                    )
                })
                .collect::<Vec<_>>()
        )
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

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(4096))]

        /// Whatever the scan passes on, kdl reads without error, and the same as the source.
        #[test]
        fn scan_never_accepts_what_kdl_rejects(src in soup()) {
            let scan = scan(&src);
            let end = match &scan.stop {
                None => scan.text.len(),
                Some(Stop::Children { cut, .. }) => *cut,
                Some(_) => return Ok(()),
            };
            let text = scan.text.get(..end).unwrap_or_default();
            let doc = KdlDocument::parse_v2(text);
            prop_assert!(doc.is_ok(), "scan passed {:?}: {:?}", text, doc);
            // The source too: blanking must not hide text kdl rejects.
            let original = KdlDocument::parse_v2(src.get(..end).unwrap_or_default());
            prop_assert!(original.is_ok(), "scan passed {:?}: {:?}", src, original);
            if let (Ok(doc), Ok(original)) = (doc, original) {
                prop_assert_eq!(outline(&doc), outline(&original));
            }
        }

        /// Whatever flat KDL kdl reads, the scan accepts, apart from slashdash chains and
        /// keywords glued to what follows: kdl does not check the end of a keyword, so it reads
        /// `/-a #true"s"` as a dropped node `a #true`, then a node `"s"`.
        #[test]
        fn scan_accepts_what_kdl_accepts(src in soup()) {
            if KdlDocument::parse_v2(&src).is_ok() && !has_glued_keyword(&src) {
                match scan(&src).stop {
                    None | Some(Stop::Children { .. }) => {}
                    Some(Stop::Error(FormatError::Syntax { message, .. }))
                        if message.contains("`/-` cannot follow") => {}
                    other => prop_assert!(false, "kdl reads {:?}, the scan stopped: {:?}", src, other),
                }
            }
        }
    }
}
