//! `:sym`: a symbol by its Typst name, picked on the command line with completions.
//!
//! The app gives the engine the symbols it can draw ([`Engine::set_symbols`]), each a name as
//! Typst math writes it (`alpha`, `arrow.r.long`) and its value, the character or characters it
//! stands for (`α`, `⟶`). lawh-core never reads Typst's own list: the app passes it in, and the
//! tests pass small lists of their own.
//!
//! `:sy[m] query` picks the symbol named exactly `query` (case counts: `Alpha` is not `alpha`),
//! else the best match for it. Matching is fuzzy, by `nucleo-matcher` (Helix's matcher), with
//! fzf's syntax: each word of the query must match the name; `^word` matches its start, `word$`
//! its end, `'word` a run of characters as typed, and `!word` leaves out the names it matches. A
//! word with a capital letter matches case as typed; other words ignore case. The best match is
//! the exact name, then the highest score (matches nearer the start of a name score higher),
//! ties by name as a dictionary orders them: ignoring case, then a small letter before its
//! capital (`alpha`, `Alpha`, `parallel`, `pi`, `Pi`), since a query in small letters matches
//! both. An empty query, or one past [`MAX_SYM_QUERY_CHARS`], picks nothing and the message says
//! why, as does a query that matches no name.
//!
//! What a pick does depends on where the command line was opened:
//!
//! - In a math block (block-Normal mode, or Insert mode through `<C-k>`): the name goes in at
//!   the cursor, with a space before it where the text before ends in a name, and a space after
//!   it where the text after starts with a letter or digit, so Typst reads it as a name of its
//!   own (`xalpha`, `x2alpha` and `alpha2` are other names; `2alpha` is 2 times alpha).
//! - In a text block: the value goes in at the cursor.
//! - On the board: a new math block of the name, at the `size` option's size, as `:ma name`
//!   makes it: at the cursor's grid point, or below the current object by the place rule.
//!
//! In block-Normal mode the text goes in before the character under the cursor, as `i` would
//! type it, and the cursor ends on its last character, as after `i...<Esc>`. That is one undo
//! step, and `.` does not repeat it: `:` commands never go through `.`. On the board, `@:`
//! picks again, as it runs any `:` line again.
//!
//! `<C-k>` in Insert mode opens the command line with `sym ` typed. `<CR>` picks and goes back
//! to Insert mode with the caret after what went in; `<Esc>`, or `<BS>` on an empty line, goes
//! back with nothing put in. Only `:sym` runs from there; another command is refused with a
//! message. The pick is part of the Insert session, so of its one undo step, and is recorded as
//! the characters it put in, spaces included: `.`, and a count on the key that started Insert
//! mode, type those characters again, not the picker's keys.
//!
//! While the command line holds a `:sym` line (`sym ` or `sy ` and a query, after any `:` and
//! spaces, as the command line reads it), the engine keeps completions ([`Engine::completions`]):
//! the best matches, at most [`SYM_ROWS`], and the row selected, none until `<Tab>` or another
//! completion key moves. Matching runs again only when the query changes. `<Tab>` and `<C-n>`
//! select the next row, `<S-Tab>` and `<C-p>` the previous one, and put its name on the line in
//! place of the query; past either end the query comes back as typed, as with Vim's wildmenu.
//! Typing again matches the line as it is then. The keys go through `cmap` mappings first.
//!
//! Matching counts as work (see [`MAX_KEY_WORK`](super::MAX_KEY_WORK)): [`SYM_WORK`] for each
//! name and each word of the query, and for a word of two characters or more,
//! [`SYM_CHAR_WORK`] for each character of each name and [`SYM_CELL_WORK`] for each cell
//! nucleo's matcher may fill. When a key runs out, it shows no rows.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::sync::Arc;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32String};

use super::command::CommandError;
use super::text_motion::{after_insert, content};
use super::undo::Edit;
use super::work::{COPY_WORK, SCAN_WORK, SYM_CELL_WORK, SYM_CHAR_WORK, SYM_WORK};
use super::{Engine, Measure, Mode};
use crate::board::Shape;
use crate::key::{Key, KeyCode, Mods};

/// Most symbols `:sym` picks from (4,096); [`Engine::set_symbols`] keeps the first this many
/// it can use and drops the rest.
///
/// Typst's `codex` 0.3.0 names 1,189 symbols for math (1,142 in `sym`, 47 in `gender` and
/// `control`), of which the bundled fonts draw 732 and miss 457 (lawh-math's
/// `counts_are_codex_0_3_0_and_the_bundled_fonts`); this leaves room for a later Typst and more
/// fonts. Each typed key of a `:sym` line that changes its query matches every name once for
/// each word of the query (see `SYM_WORK` in `work.rs`). With this many names about as long as
/// Typst's (11 characters on average, 28 at most; the test's average 14), every key of the
/// costliest queries of [`MAX_SYM_QUERY_CHARS`] fits in a third of
/// [`MAX_KEY_WORK`](super::MAX_KEY_WORK), 4.3 ms at most in debug
/// (`a_max_length_query_fits_the_work_budget` in `tests/engine_cost.rs`). With names of 64
/// characters some of those queries cost more than the budget, and are refused whole.
pub const MAX_SYMBOLS: usize = 4096;

/// Most characters in a `:sym` query, spaces at its ends not counted (64). A longer query
/// matches nothing and the message says so, while typing and on `<CR>`.
///
/// The longest Typst name is 28 characters. Matching costs each word of the query a pass over
/// every name: a query of 1,000 characters and 500 words took 7.3 ms in release over the 1,142
/// names of Typst's `sym` (of the 1,189 lawh lists), while 64 characters hold at most 32
/// words. A name longer than this could not be put on the line by `<Tab>` and matched again,
/// so [`Engine::set_symbols`] drops such names.
pub const MAX_SYM_QUERY_CHARS: usize = 64;

/// Most bytes of a `:sym` query that are read before it is refused: four a character, the most
/// UTF-8 takes, so a longer query holds more than [`MAX_SYM_QUERY_CHARS`] characters whatever
/// they are. Derived, not measured.
///
/// It is refused without copying the line or counting its characters: before, a mapping that
/// typed and took back a character after a query of 5,050 four-byte characters (a 20 KB line,
/// within [`MAX_COMMAND_LINE_CHARS`](super::MAX_COMMAND_LINE_CHARS)) copied and counted the
/// line on every key, 30,000 keys in 110 ms in release and 171 ms in debug, none of it counted
/// as work; now 6.5 ms and 52 ms, the keys running out first (`key_costs` in
/// `tests/engine_cost.rs`, 2026-10-12). A query of at most this many bytes is copied and counted
/// as work ([`COPY_WORK`] a byte).
const MAX_SYM_QUERY_BYTES: usize = 4 * MAX_SYM_QUERY_CHARS;

/// Rows of completions a `:sym` line shows (8), the best match first.
///
/// Eight rows of the status line's height stay above the status line in any window lawh is
/// usable in, and a query a few characters long already narrows Typst's names to a handful.
pub const SYM_ROWS: usize = 8;

/// The key that opens a `:sym` line from Insert mode.
pub(super) const SYM_KEY: Key = Key::ctrl('k');

/// A symbol `:sym` can pick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    /// Its name in Typst math: `alpha`, `arrow.r.long`. Put into math blocks, and on the
    /// command line by `<Tab>`.
    pub name: String,
    /// What it stands for: one character or a few (`α`, `⟶`). Put into text blocks.
    pub value: String,
}

/// The symbols `:sym` picks from, and what matching them costs.
#[derive(Debug, Clone)]
pub(super) struct Symbols {
    /// In order of name, no name twice.
    list: Arc<[Symbol]>,
    /// The names as the matcher reads them, in the same order: made once, not on every key.
    names: Vec<Utf32String>,
    /// How many names have each length in characters, from 0 to [`MAX_SYM_QUERY_CHARS`].
    lengths: [usize; MAX_SYM_QUERY_CHARS + 1],
    /// Characters in all the names.
    chars: usize,
}

impl Symbols {
    /// `list`, which must be in order of name ([`name_order`]) with no name twice and no name
    /// longer than [`MAX_SYM_QUERY_CHARS`].
    fn new(list: Arc<[Symbol]>) -> Self {
        let mut lengths = [0; MAX_SYM_QUERY_CHARS + 1];
        let mut chars = 0;
        for symbol in list.iter() {
            let n = symbol.name.chars().count();
            chars += n;
            if let Some(count) = lengths.get_mut(n) {
                *count += 1;
            }
        }
        let names = list
            .iter()
            .map(|s| Utf32String::from(s.name.as_str()))
            .collect();
        Self {
            list,
            names,
            lengths,
            chars,
        }
    }

    /// The work of matching every name against one word of `n` characters (see [`SYM_WORK`]).
    /// nucleo finds a word of one character in a name with no more than a look at each place
    /// it is in. A longer word costs a pass over each name, and the cells the matcher fills for
    /// each name it fits in: `n` rows of `m - n + 1` for a name of `m` characters.
    fn word_work(&self, n: usize) -> usize {
        let names = SYM_WORK.saturating_mul(self.list.len());
        if n <= 1 {
            return names;
        }
        let cells: usize = self
            .lengths
            .iter()
            .enumerate()
            .skip(n)
            .map(|(m, count)| count.saturating_mul(m + 1 - n))
            .fold(0, usize::saturating_add);
        names
            .saturating_add(SYM_CHAR_WORK.saturating_mul(self.chars))
            .saturating_add(SYM_CELL_WORK.saturating_mul(n).saturating_mul(cells))
    }
}

impl Default for Symbols {
    fn default() -> Self {
        Self::new(Arc::from(Vec::new()))
    }
}

/// The completions of the `:sym` line on the command line.
#[derive(Debug, Clone)]
pub(super) struct Completions {
    /// The line as typed, which the rows were matched for.
    typed: String,
    /// Bytes of `typed` before its query: the `:` and spaces before the name, the name, and the
    /// spaces after it.
    prefix: usize,
    /// The query: the text after the name, without spaces at its ends.
    query: String,
    /// The best matches, best first, at most [`SYM_ROWS`].
    rows: Vec<Symbol>,
    /// The row whose name is on the line, put there by `<Tab>` or another completion key.
    selected: Option<usize>,
    /// The line as the completions left it: `typed`, or its prefix and a row's name.
    shown: String,
}

impl Completions {
    /// No rows, for a query of more than [`MAX_SYM_QUERY_BYTES`]: the line is not kept, so the
    /// next key on it looks at it again (and refuses it again, as cheaply).
    fn refused(prefix: usize) -> Self {
        Self {
            typed: String::new(),
            prefix,
            query: String::new(),
            rows: Vec::new(),
            selected: None,
            shown: String::new(),
        }
    }
}

impl Engine {
    /// Sets the symbols `:sym` picks from, and returns how many it kept.
    ///
    /// It keeps the symbols `:sym` can use, the first [`MAX_SYMBOLS`] of them: a name of 1 to
    /// [`MAX_SYM_QUERY_CHARS`] characters with no space or control character in it, a value of
    /// at least one character with no control character in it, and of several symbols with one
    /// name, the first. Values go into text blocks as if typed, which control characters would
    /// not be. A list that is all usable, in order of name and with no name twice, is kept as
    /// it is; any other is copied, in order of name. The order is a dictionary's: ignoring
    /// case, then a small letter before its capital (`alpha`, `Alpha`, `beta`), comparing
    /// ASCII letters only.
    ///
    /// The app calls this once, before typing starts. Completions showing are dropped until the
    /// next key changes the line.
    pub fn set_symbols(&mut self, symbols: Arc<[Symbol]>) -> usize {
        self.completions = None;
        let ready = symbols.len() <= MAX_SYMBOLS
            && symbols.iter().all(usable)
            && symbols.windows(2).all(|w| match w {
                [a, b] => name_order(&a.name, &b.name) == Ordering::Less,
                _ => true,
            });
        let list = if ready {
            symbols
        } else {
            let mut seen = HashSet::new();
            let mut kept: Vec<Symbol> = symbols
                .iter()
                .filter(|s| usable(s) && seen.insert(s.name.as_str()))
                .take(MAX_SYMBOLS)
                .cloned()
                .collect();
            kept.sort_by(|a, b| name_order(&a.name, &b.name));
            Arc::from(kept)
        };
        self.symbols = Symbols::new(list);
        self.symbols.list.len()
    }

    /// The completions of the `:sym` line being typed: the best matching symbols, best first
    /// and at most [`SYM_ROWS`], and the one selected (its name is on the line), if any. The
    /// rows are empty for an empty query, one past [`MAX_SYM_QUERY_CHARS`], or one that matches
    /// nothing. `None` unless the command line holds a `:sym` line.
    pub fn completions(&self) -> Option<(&[Symbol], Option<usize>)> {
        if self.mode != Mode::CommandLine {
            return None;
        }
        let completions = self.completions.as_ref()?;
        Some((completions.rows.as_slice(), completions.selected))
    }

    /// `<C-k>` in Insert mode: the command line with `sym ` typed, its keys going back to the
    /// Insert session (see the module docs).
    pub(super) fn open_sym_line(&mut self) {
        self.command_line.clear();
        self.command_line.set_text("sym ".to_owned());
        self.mode = Mode::CommandLine;
        self.update_completions();
    }

    /// After a key that may have changed the command line: keeps the completions of a `:sym`
    /// line, matching again when its query changed, and drops them on any other line.
    pub(super) fn update_completions(&mut self) {
        let line = self.command_line.text();
        if self.completions.as_ref().is_some_and(|c| c.shown == line) {
            return;
        }
        let Some((prefix, query)) = sym_line(line) else {
            self.completions = None;
            return;
        };
        if query.len() > MAX_SYM_QUERY_BYTES {
            // Past the cap whatever its characters: refused before the line is copied or the
            // query's characters counted, so a key on a long line costs next to nothing.
            self.completions = Some(Completions::refused(prefix));
            self.error(CommandError::SymQueryTooLong.to_string());
            return;
        }
        // Copying the line twice (as typed and as shown) and the query, and counting the
        // query's characters.
        let (copied, query_len) = (
            line.len().saturating_mul(2).saturating_add(query.len()),
            query.len(),
        );
        if !self.charge(COPY_WORK.saturating_mul(copied)) {
            self.completions = None;
            return;
        }
        let line = self.command_line.text().to_owned();
        let query = line
            .get(prefix..prefix + query_len)
            .unwrap_or_default()
            .to_owned();
        if let Some(completions) = &mut self.completions
            && completions.query == query
        {
            // Only the line around the query changed: the rows stay.
            completions.typed.clone_from(&line);
            completions.shown = line;
            completions.prefix = prefix;
            completions.selected = None;
            return;
        }
        let rows = if query.chars().count() > MAX_SYM_QUERY_CHARS {
            self.error(CommandError::SymQueryTooLong.to_string());
            Vec::new()
        } else if let Some(rows) = self.match_symbols(&query) {
            rows
        } else {
            // Out of work: no rows, and the next key matches again.
            self.completions = None;
            return;
        };
        self.completions = Some(Completions {
            typed: line.clone(),
            prefix,
            query,
            rows,
            selected: None,
            shown: line,
        });
    }

    /// `<Tab>`, `<C-n>`, `<S-Tab>` and `<C-p>` on a `:sym` line: selects the next or previous
    /// row and puts its name on the line, or the query as typed past either end. True when the
    /// key was one of them on a `:sym` line; other keys, and these on other lines, go on to the
    /// command line's editing (where they do nothing).
    pub(super) fn completion_key(&mut self, key: Key) -> bool {
        let shift_tab = Key::new(
            KeyCode::Tab,
            Mods {
                shift: true,
                ..Mods::NONE
            },
        );
        let forward = if key == Key::named(KeyCode::Tab) || key == Key::ctrl('n') {
            true
        } else if key == shift_tab || key == Key::ctrl('p') {
            false
        } else {
            return false;
        };
        let Some(completions) = &self.completions else {
            return false;
        };
        let Some(last) = completions.rows.len().checked_sub(1) else {
            return true;
        };
        let next = match (completions.selected, forward) {
            (None, true) => Some(0),
            (None, false) => Some(last),
            (Some(i), true) => (i < last).then_some(i + 1),
            (Some(i), false) => i.checked_sub(1),
        };
        let line = match next.and_then(|i| completions.rows.get(i)) {
            Some(row) => {
                let prefix = completions.typed.get(..completions.prefix);
                format!("{}{}", prefix.unwrap_or_default(), row.name)
            }
            None => completions.typed.clone(),
        };
        // Copying the line onto the command line.
        if self.charge(COPY_WORK.saturating_mul(line.len()))
            && self.command_line.set_text(line.clone())
            && let Some(completions) = &mut self.completions
        {
            completions.selected = next;
            completions.shown = line;
        }
        true
    }

    /// `:sym` picks for `query` (see the module docs): the symbol named `query`, else the best
    /// match, into the block being edited, or as a new math block on the board (placed as `:ma`
    /// places one, which measures the board when the cursor is on an object).
    pub(super) fn pick_symbol(&mut self, query: &str, measure: &mut dyn Measure) {
        let Some(symbol) = self.find_symbol(query) else {
            return;
        };
        if self.block.is_some() {
            return self.put_in_block(&symbol);
        }
        let shape = Shape::Math {
            at: self.cursor_point(),
            src: symbol.name,
            size: self.settings.size(),
        };
        self.add_block(shape, measure);
    }

    /// The symbol named `query`, else the best match for it: from the completions when they
    /// were matched for it, else matched now. `None` with a message when nothing matches, or
    /// without one when the key ran out of work.
    fn find_symbol(&mut self, query: &str) -> Option<Symbol> {
        let query = query.trim();
        let list = &self.symbols.list;
        if let Ok(i) = list.binary_search_by(|s| name_order(&s.name, query)) {
            return list.get(i).cloned();
        }
        let rows = match self.completions.take() {
            Some(completions) if completions.query == query => completions.rows,
            _ => self.match_symbols(query)?,
        };
        let best = rows.into_iter().next();
        if best.is_none() {
            if self.symbols.list.is_empty() {
                self.error("no symbols to pick from");
            } else {
                self.error(format!("no symbol matches \"{query}\""));
            }
        }
        best
    }

    /// The best matches for `query`, best first, at most [`SYM_ROWS`]: none for an empty query
    /// or one past [`MAX_SYM_QUERY_CHARS`]. Matching each word of the query against every name
    /// counts as work (see [`SYM_WORK`]); `None` when the key runs out of work.
    fn match_symbols(&mut self, query: &str) -> Option<Vec<Symbol>> {
        if query.chars().count() > MAX_SYM_QUERY_CHARS {
            return Some(Vec::new());
        }
        let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
        // No words, or only syntax (`^`, `!`): nothing to match.
        if pattern.atoms.is_empty() {
            return Some(Vec::new());
        }
        let work = pattern
            .atoms
            .iter()
            .map(|atom| self.symbols.word_work(atom.needle_text().len()))
            .fold(0, usize::saturating_add);
        if !self.charge(work) {
            return None;
        }
        let Symbols { list, names, .. } = &self.symbols;
        let matcher = self.matcher.get_or_insert_with(new_matcher);
        // The best so far, best first: (the exact name, the score, the position in order of
        // name). Later names come after earlier ones of the same rank, so ties go by name.
        let mut best: Vec<(bool, u32, usize)> = Vec::with_capacity(SYM_ROWS + 1);
        for (i, (symbol, name)) in list.iter().zip(names).enumerate() {
            let Some(score) = pattern.score(name.slice(..), matcher) else {
                continue;
            };
            let rank = (symbol.name == query, score);
            if best.len() == SYM_ROWS && best.last().is_some_and(|&(e, s, _)| (e, s) >= rank) {
                continue;
            }
            let at = best.partition_point(|&(e, s, _)| (e, s) >= rank);
            best.insert(at, (rank.0, rank.1, i));
            best.truncate(SYM_ROWS);
        }
        Some(
            best.iter()
                .filter_map(|&(_, _, i)| list.get(i).cloned())
                .collect(),
        )
    }

    /// Puts `symbol` into the block being edited at its cursor: the name into math (spaced from
    /// a name around it), the value into text. In an Insert session (from `<C-k>`) it is typed
    /// into the session; in block-Normal mode it is one undo step, the cursor on its last
    /// character. A refusal by the board (a cap on text) leaves the block as it was, with a
    /// message.
    fn put_in_block(&mut self, symbol: &Symbol) {
        let Some(block) = &self.block else {
            return;
        };
        let (id, byte) = (block.id.clone(), block.byte);
        let Some(before) = self.board.get(&id).map(|o| o.shape.clone()) else {
            return;
        };
        let text = content(&before).unwrap_or_default();
        let insert = if let Shape::Math { .. } = before {
            let (insert, scanned) = spaced(&symbol.name, text, byte);
            // Looking back through the name before the cursor.
            if !self.charge(SCAN_WORK.saturating_mul(scanned)) {
                return;
            }
            insert
        } else {
            symbol.value.clone()
        };
        let cursor = self.cursor.clone();
        if !self.splice(byte, byte, &insert) {
            return;
        }
        if self.session.is_some() {
            // Typed into the session: `.` and a count type the same characters.
            for c in insert.chars() {
                self.repeat.push(Key::char(c));
                self.keep_session_key(Key::char(c));
            }
            return;
        }
        let end = self.block.as_ref().map_or(byte, |b| b.byte);
        let on_last = self
            .block_text()
            .map_or(end, |text| after_insert(text, end));
        if let Some(block) = &mut self.block {
            block.byte = on_last;
        }
        let Some(after) = self.board.get(&id).map(|o| o.shape.clone()) else {
            return;
        };
        let edit = Edit::Replace { id, before, after };
        self.commit_at(vec![edit], cursor, Some(byte), Some(on_last));
    }
}

/// The order of names: ignoring the case of ASCII letters, then a small letter before its
/// capital, so `alpha`, `Alpha`, `beta`. Equal only for equal names.
fn name_order(a: &str, b: &str) -> Ordering {
    // Swapping the case of ASCII letters puts small letters before capitals; it changes no
    // other byte, so only equal names compare equal.
    let swapped = |c: u8| {
        if c.is_ascii_alphabetic() { c ^ 0x20 } else { c }
    };
    a.bytes()
        .map(|c| c.to_ascii_lowercase())
        .cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
        .then_with(|| a.bytes().map(swapped).cmp(b.bytes().map(swapped)))
}

/// `:sym` can pick `symbol` (see [`Engine::set_symbols`]).
fn usable(symbol: &Symbol) -> bool {
    let name = &symbol.name;
    let name_ok = !name.is_empty()
        && name.chars().count() <= MAX_SYM_QUERY_CHARS
        && !name.chars().any(|c| c.is_whitespace() || c.is_control());
    let value_ok = !symbol.value.is_empty() && !symbol.value.chars().any(char::is_control);
    name_ok && value_ok
}

/// The matcher `:sym` matches with: nucleo's defaults, and a bonus for matches nearer the start
/// of a name, since names are typed from their start.
fn new_matcher() -> Matcher {
    let mut config = Config::DEFAULT;
    config.prefer_prefix = true;
    Matcher::new(config)
}

/// The command's name at the start of a `:` line, as the command line reads it: after any `:`
/// and spaces, the letters up to the first character that is not one.
pub(super) fn command_name(line: &str) -> &str {
    let line = line.trim_start_matches(|c: char| c == ':' || c.is_whitespace());
    let len = line
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(line.len());
    line.get(..len).unwrap_or_default()
}

/// `line` is a `:sym` command: its name is `sy` or `sym`.
pub(super) fn is_sym_command(line: &str) -> bool {
    matches!(command_name(line), "sy" | "sym")
}

/// The query of a `:sym` line: `sy` or `sym` and a space after any `:` and spaces, then the
/// query. The bytes before the query, and the query without spaces at its ends; `None` for
/// any other line.
fn sym_line(line: &str) -> Option<(usize, &str)> {
    let name = command_name(line);
    if !matches!(name, "sy" | "sym") {
        return None;
    }
    // The name starts where the `:` and spaces before it end.
    let start = line.len()
        - line
            .trim_start_matches(|c: char| c == ':' || c.is_whitespace())
            .len();
    let after = line.get(start + name.len()..)?;
    let mut chars = after.chars();
    chars.next().filter(|c| c.is_whitespace())?;
    let query = chars.as_str().trim_start();
    let prefix = line.len() - query.len();
    Some((prefix, query.trim_end()))
}

/// `name` to put into math at `byte` of `text`, with a space before it where the text before
/// ends in a name (a letter, or digits after one) and after it where the text after starts with
/// a letter or digit: Typst reads letters and digits that touch as one name. Also the bytes it
/// looked back through.
fn spaced(name: &str, text: &str, byte: usize) -> (String, usize) {
    let before = text.get(..byte).unwrap_or_default();
    let after = text.get(byte..).unwrap_or_default();
    let run = before.trim_end_matches(char::is_alphanumeric);
    let word = before.get(run.len()..).unwrap_or_default();
    let space_before = word.chars().any(char::is_alphabetic);
    let space_after = after.chars().next().is_some_and(char::is_alphanumeric);
    let mut insert = String::with_capacity(name.len() + 2);
    if space_before {
        insert.push(' ');
    }
    insert.push_str(name);
    if space_after {
        insert.push(' ');
    }
    (insert, word.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sym_lines_take_sy_or_sym_and_a_space() {
        assert_eq!(sym_line("sym al"), Some((4, "al")));
        assert_eq!(sym_line("sy al"), Some((3, "al")));
        assert_eq!(sym_line(": sym   arrow r  "), Some((8, "arrow r")));
        assert_eq!(sym_line("sym "), Some((4, "")));
        assert_eq!(sym_line("sym"), None);
        assert_eq!(sym_line("syms a"), None);
        assert_eq!(sym_line("s a"), None);
        assert_eq!(sym_line("sym! a"), None);
        assert_eq!(sym_line("si 2"), None);
        assert!(is_sym_command("sym! a") && is_sym_command(":sy") && !is_sym_command("set"));
    }

    #[test]
    fn names_are_spaced_from_names_around_them() {
        let at = |text: &str, byte| spaced("alpha", text, byte).0;
        assert_eq!(at("", 0), "alpha");
        assert_eq!(at("x", 1), " alpha");
        assert_eq!(at("2", 1), "alpha");
        assert_eq!(at("x2", 2), " alpha");
        assert_eq!(at("(x)", 3), "alpha");
        assert_eq!(at("x", 0), "alpha ");
        assert_eq!(at("2", 0), "alpha ");
        assert_eq!(at("x y", 1), " alpha");
        assert_eq!(at("x y", 2), "alpha ");
        assert_eq!(at("é", 2), " alpha");
        assert_eq!(at("x\ny", 2), "alpha ");
        assert_eq!(at("x\n", 2), "alpha");
        assert_eq!(spaced("alpha", "a+bc", 4), (" alpha".to_owned(), 2));
    }

    #[test]
    fn names_go_in_dictionary_order() {
        let mut names = [
            "Pi", "pi", "beta", "Alpha", "alpha", "arrow.r", "arrow", "AA", "aA", "Aa", "aa",
            "a.b", "ab", "é", "z",
        ];
        names.sort_by(|a, b| name_order(a, b));
        let expected = [
            "a.b", "aa", "aA", "Aa", "AA", "ab", "alpha", "Alpha", "arrow", "arrow.r", "beta",
            "pi", "Pi", "z", "é",
        ];
        assert_eq!(names, expected);
        for a in expected {
            for b in expected {
                assert_eq!(name_order(a, b) == Ordering::Equal, a == b, "{a} {b}");
            }
        }
    }

    #[test]
    fn symbols_need_a_name_and_a_value_to_type() {
        let s = |name: &str, value: &str| Symbol {
            name: name.to_owned(),
            value: value.to_owned(),
        };
        assert!(usable(&s("alpha", "α")));
        assert!(usable(&s("space.en", "\u{2002}")));
        assert!(usable(&s(&"a".repeat(MAX_SYM_QUERY_CHARS), "a")));
        assert!(!usable(&s(&"a".repeat(MAX_SYM_QUERY_CHARS + 1), "a")));
        assert!(!usable(&s("", "α")));
        assert!(!usable(&s("al pha", "α")));
        assert!(!usable(&s("alpha", "")));
        assert!(!usable(&s("alpha", "a\nb")));
        assert!(!usable(&s("al\u{7}", "a")));
    }
}
