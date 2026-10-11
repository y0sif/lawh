//! Key mappings: `:map` and its family, and the typeahead they expand in.
//!
//! As in Vim, keys from [`Engine::feed`] go into a typeahead before the grammar sees them. Each
//! of Vim's map modes has its own mappings: `n` (Normal mode, on the board and in a text block),
//! `x` (Visual mode), `o` (an operator waiting for its motion), `i` (Insert mode) and `c` (the
//! command line). A key is looked up in the mode the engine is in when its turn comes, so
//! `inoremap jk <Esc>` works right after `i`.
//!
//! When the first keys of the typeahead are a mapping's keys, they are replaced by what they
//! map to. When they are only the start of a longer mapping, they wait for the next key, or for
//! [`Engine::timeout`] once `timeoutlen` has passed (see [`Effect::ArmTimer`]); a mapping made
//! with `<nowait>` does not wait for longer ones. On a timeout the keys take the longest mapping
//! they make, or run as typed. A key that starts no mapping runs as typed, and the keys after it
//! are looked up again.
//!
//! Keys from `:noremap` and its kind are not mapped again. Keys from `:map` are, except the
//! first one when what a mapping maps to starts with its own keys (Vim's rule, so `:nmap l lw`
//! moves right, then onto the next object). `<Leader>` in a mapping is the `leader` setting at
//! the time the mapping is made. As in Vim, a `0` that goes on a count and the key after `@`
//! are not mapped, and `.` replays the keys a change ran after mapping, without mapping them
//! again.

use std::collections::VecDeque;

use super::command::Command;
use super::{Effect, Engine, Measure, Mode};
use crate::key::{Key, KeyCode, format_keys};

/// Most mappings in each mode (1,000).
///
/// A lawhrc holds tens. Every key typed is looked up in its mode's mappings one by one, so the
/// cap keeps that cheap. It also bounds what mappings hold: 1,000 mappings of two
/// [`MAX_MAP_KEYS`] keys (12 bytes a key) are about 6 MB a mode.
pub const MAX_MAPS: usize = 1000;

/// Most keys on each side of a mapping: the keys mapped, and what they map to (256).
///
/// Vim takes at most 50 bytes of keys to map; a long `:te` or `:ma` line fits in what they map
/// to. With [`MAX_MAP_DEPTH`], the cap bounds the keys one typed key can set off.
pub const MAX_MAP_KEYS: usize = 256;

/// Most mappings that one typed key (or a timeout) may expand, one from another (1,000, Vim's
/// default `maxmapdepth`). Past it the typeahead is dropped and the message says "recursive
/// mapping".
///
/// `:nmap a b` with `:nmap b a` never ends otherwise. Vim counts expansions until a key reaches
/// a command; lawh counts them until the next typed key, because a mapping can loop forever
/// while its keys do reach commands: `:map x yx` yanks with `yy` and maps `x` again without end.
/// Vim stops that with Ctrl-C, which the engine does not have. So one typed key runs at most
/// 1,000 expansions of at most [`MAX_MAP_KEYS`] keys each.
pub const MAX_MAP_DEPTH: usize = 1000;

/// Most keys the typeahead holds (10,000); past it, it is dropped with a message.
///
/// Typed keys wait there only while they start a longer mapping, so they are at most
/// [`MAX_MAP_KEYS`]. A mapping that feeds itself more keys than it takes (`:map a` with a long
/// run of `a`s, in Visual mode, where `a` does nothing) adds up to 255 keys an expansion; this
/// stops it long before [`MAX_MAP_DEPTH`], within 40 expansions at worst.
pub const MAX_PENDING_KEYS: usize = 10_000;

/// One of Vim's map modes: where a mapping applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MapMode {
    /// `n`: Normal mode, on the board and in a text block (block-Normal mode).
    Normal,
    /// `x`: Visual mode (`v` in `:vmap` means the same in lawh).
    Visual,
    /// `o`: an operator (`d`, `y`) waiting for its motion.
    OperatorPending,
    /// `i`: Insert mode.
    Insert,
    /// `c`: the `:` command line.
    CommandLine,
}

impl MapMode {
    /// The letter that starts the mode's commands: `n` in `:nmap`, `x`, `o`, `i`, `c`.
    pub const fn letter(self) -> char {
        match self {
            Self::Normal => 'n',
            Self::Visual => 'x',
            Self::OperatorPending => 'o',
            Self::Insert => 'i',
            Self::CommandLine => 'c',
        }
    }

    /// The mode's name in messages.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Visual => "Visual",
            Self::OperatorPending => "operator-pending",
            Self::Insert => "Insert",
            Self::CommandLine => "command-line",
        }
    }
}

/// The modes of `:map`, `:noremap`, `:unmap` and `:mapclear`: Normal, Visual and
/// operator-pending.
pub(super) const NVO: &[MapMode] = &[MapMode::Normal, MapMode::Visual, MapMode::OperatorPending];

/// The modes of `:map!`, `:noremap!`, `:unmap!` and `:mapclear!`: Insert and command-line.
pub(super) const IC: &[MapMode] = &[MapMode::Insert, MapMode::CommandLine];

/// A key of a mapping as typed in a `:map` command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapKey {
    Key(Key),
    /// `<Leader>`: the `leader` setting when the mapping is made.
    Leader,
}

/// A mapping: `lhs` typed in its mode runs `rhs`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Mapping {
    lhs: Vec<Key>,
    rhs: Vec<Key>,
    /// Made by `:noremap` and its kind: `rhs` is not mapped again.
    noremap: bool,
    /// `<nowait>`: used at once, without waiting to see whether a longer mapping is typed.
    nowait: bool,
}

impl Mapping {
    /// The command that makes this mapping in `mode`: `nnoremap <nowait> j h`.
    fn describe(&self, mode: MapMode) -> String {
        let kind = if self.noremap { "noremap" } else { "map" };
        let nowait = if self.nowait { "<nowait> " } else { "" };
        // Spaces inside show as typed, as in Vim's listing; at the ends a `:map` line would
        // drop them, so they show as `<Space>`.
        let last = self.rhs.len().saturating_sub(1);
        let rhs: String = self
            .rhs
            .iter()
            .enumerate()
            .map(|(i, key)| match key.code {
                KeyCode::Char(' ') if key.mods.is_empty() && i != 0 && i != last => " ".to_owned(),
                _ => key.to_string(),
            })
            .collect();
        let rhs = if rhs.is_empty() {
            "<Nop>".to_owned()
        } else {
            rhs
        };
        format!(
            "{}{kind} {nowait}{} {rhs}",
            mode.letter(),
            format_keys(&self.lhs)
        )
    }
}

/// A key in the typeahead.
#[derive(Debug, Clone, Copy)]
struct Queued {
    key: Key,
    /// The key may be mapped: it was typed, or came from `:map`.
    remap: bool,
}

/// What the typeahead's first keys make in a mode's mappings.
enum Lookup {
    /// No mapping: the first key runs as typed.
    Literal,
    /// They start a longer mapping: wait for another key or the timeout.
    Wait,
    /// Replace the first `len` keys with `keys`.
    Expand { len: usize, keys: Vec<Queued> },
}

/// The mappings of every mode, and the typeahead.
#[derive(Debug, Clone, Default)]
pub(super) struct Keymap {
    normal: Vec<Mapping>,
    visual: Vec<Mapping>,
    operator: Vec<Mapping>,
    insert: Vec<Mapping>,
    command_line: Vec<Mapping>,
    /// Keys not yet run: typed keys waiting for a longer mapping, and keys from mappings.
    typeahead: VecDeque<Queued>,
    /// Mappings expanded since the last typed key or timeout (see [`MAX_MAP_DEPTH`]).
    depth: usize,
}

impl Keymap {
    fn mappings(&self, mode: MapMode) -> &[Mapping] {
        match mode {
            MapMode::Normal => &self.normal,
            MapMode::Visual => &self.visual,
            MapMode::OperatorPending => &self.operator,
            MapMode::Insert => &self.insert,
            MapMode::CommandLine => &self.command_line,
        }
    }

    fn mappings_mut(&mut self, mode: MapMode) -> &mut Vec<Mapping> {
        match mode {
            MapMode::Normal => &mut self.normal,
            MapMode::Visual => &mut self.visual,
            MapMode::OperatorPending => &mut self.operator,
            MapMode::Insert => &mut self.insert,
            MapMode::CommandLine => &mut self.command_line,
        }
    }

    /// Keys waiting for a longer mapping, in Vim notation.
    pub(super) fn waiting(&self) -> String {
        self.typeahead.iter().map(|q| q.key.to_string()).collect()
    }

    /// Drops every key in the typeahead.
    pub(super) fn flush(&mut self) {
        self.typeahead.clear();
    }

    /// Looks up the typeahead, which is not empty, in `mode`'s mappings. When `timed_out`, no
    /// more keys are coming for a longer mapping.
    fn lookup(&self, mode: MapMode, timed_out: bool) -> Lookup {
        let typeahead = &self.typeahead;
        let mut full: Option<&Mapping> = None;
        let mut longer = false;
        for mapping in self.mappings(mode) {
            let lhs = &mapping.lhs;
            if lhs.is_empty() || !lhs.iter().zip(typeahead).all(|(k, q)| *k == q.key) {
                continue;
            }
            if lhs.len() > typeahead.len() {
                longer = true;
            } else if full.is_none_or(|f| lhs.len() > f.lhs.len()) {
                full = Some(mapping);
            }
        }
        if longer && !timed_out && !full.is_some_and(|m| m.nowait) {
            return Lookup::Wait;
        }
        let Some(mapping) = full else {
            return Lookup::Literal;
        };
        // Vim's rule: when the keys map to themselves and more, the first is not mapped again.
        let first_as_typed = mapping.rhs.starts_with(&mapping.lhs);
        let keys = mapping
            .rhs
            .iter()
            .enumerate()
            .map(|(i, &key)| Queued {
                key,
                remap: !(mapping.noremap || (first_as_typed && i == 0)),
            })
            .collect();
        Lookup::Expand {
            len: mapping.lhs.len(),
            keys,
        }
    }

    /// Replaces the first `len` keys of the typeahead with `keys`, within [`MAX_MAP_DEPTH`] and
    /// [`MAX_PENDING_KEYS`]: past either, the message to show.
    fn expand(&mut self, len: usize, keys: Vec<Queued>) -> Result<(), String> {
        if self.depth >= MAX_MAP_DEPTH {
            return Err("recursive mapping".to_owned());
        }
        let len = len.min(self.typeahead.len());
        if self.typeahead.len() - len + keys.len() > MAX_PENDING_KEYS {
            return Err(format!(
                "mappings made more than {MAX_PENDING_KEYS} keys at once; they were dropped"
            ));
        }
        self.depth += 1;
        self.typeahead.drain(..len);
        for queued in keys.into_iter().rev() {
            self.typeahead.push_front(queued);
        }
        Ok(())
    }

    /// Adds `mapping` to each of `modes`, replacing one of the same keys. All or nothing: no
    /// mode takes it when one is full.
    fn define(&mut self, modes: &[MapMode], mapping: &Mapping) -> Result<(), String> {
        let full = modes.iter().find(|mode| {
            let mappings = self.mappings(**mode);
            mappings.len() >= MAX_MAPS && mappings.iter().all(|m| m.lhs != mapping.lhs)
        });
        if let Some(mode) = full {
            return Err(format!(
                "{} mode holds at most {MAX_MAPS} mappings",
                mode.name()
            ));
        }
        for mode in modes {
            let mappings = self.mappings_mut(*mode);
            match mappings.iter_mut().find(|m| m.lhs == mapping.lhs) {
                Some(old) => old.clone_from(mapping),
                None => mappings.push(mapping.clone()),
            }
        }
        Ok(())
    }

    /// Removes the mappings of `lhs` from `modes`: false when there were none.
    fn remove(&mut self, modes: &[MapMode], lhs: &[Key]) -> bool {
        let mut removed = false;
        for mode in modes {
            let mappings = self.mappings_mut(*mode);
            let before = mappings.len();
            mappings.retain(|m| m.lhs != lhs);
            removed |= mappings.len() != before;
        }
        removed
    }

    /// The mappings of `lhs` in `modes`, as the commands that make them; with no keys, how many
    /// mappings `modes` hold.
    fn show(&self, modes: &[MapMode], lhs: &[Key]) -> String {
        if lhs.is_empty() {
            let n: usize = modes.iter().map(|mode| self.mappings(*mode).len()).sum();
            let noun = if n == 1 { "mapping" } else { "mappings" };
            return format!("{n} {noun}; `:map {{keys}}` shows the mappings of some keys");
        }
        let found: Vec<String> = modes
            .iter()
            .filter_map(|mode| {
                let mapping = self.mappings(*mode).iter().find(|m| m.lhs == lhs)?;
                Some(mapping.describe(*mode))
            })
            .collect();
        if found.is_empty() {
            format!("no mapping for `{}`", format_keys(lhs))
        } else {
            found.join(", ")
        }
    }
}

impl Engine {
    /// Puts a typed key at the end of the typeahead and runs what it can.
    pub(super) fn type_key(&mut self, key: Key, measure: &mut dyn Measure) -> Vec<Effect> {
        self.keymap.depth = 0;
        self.keymap.typeahead.push_back(Queued { key, remap: true });
        self.run_typeahead(false, measure)
    }

    /// The time [`Effect::ArmTimer`] asked for has passed with no other key: keys waiting for a
    /// longer mapping run now, as the longest mapping they make, or as typed. Does nothing when
    /// no keys wait.
    ///
    /// The app drops an armed timer when a key comes, before calling [`Engine::feed`]: a late
    /// timeout would cut short keys that started waiting since.
    pub fn timeout(&mut self, measure: &mut dyn Measure) -> Vec<Effect> {
        if self.keymap.typeahead.is_empty() {
            return Vec::new();
        }
        self.keymap.depth = 0;
        self.run_typeahead(true, measure)
    }

    /// Runs the typeahead until it is empty or waits for a longer mapping. When `timed_out`,
    /// keys do not wait.
    fn run_typeahead(&mut self, timed_out: bool, measure: &mut dyn Measure) -> Vec<Effect> {
        let mut effects = Vec::new();
        while let Some(first) = self.keymap.typeahead.front().copied() {
            let lookup = if first.remap && !self.unmapped(first.key) {
                self.keymap.lookup(self.map_mode(), timed_out)
            } else {
                Lookup::Literal
            };
            match lookup {
                Lookup::Literal => {
                    self.keymap.typeahead.pop_front();
                    effects.extend(self.dispatch(first.key, measure));
                }
                Lookup::Wait => {
                    effects.push(Effect::ArmTimer {
                        ms: self.settings.timeoutlen,
                    });
                    break;
                }
                Lookup::Expand { len, keys } => {
                    if let Err(message) = self.keymap.expand(len, keys) {
                        self.keymap.flush();
                        self.error(message);
                        break;
                    }
                }
            }
        }
        effects
    }

    /// The map mode the next key is looked up in.
    fn map_mode(&self) -> MapMode {
        match self.mode {
            Mode::Normal if self.pending.has_operator() => MapMode::OperatorPending,
            Mode::Normal | Mode::BlockNormal => MapMode::Normal,
            Mode::Visual => MapMode::Visual,
            Mode::Insert => MapMode::Insert,
            Mode::CommandLine => MapMode::CommandLine,
        }
    }

    /// Keys Vim does not map where they come: a `0` that goes on a count (`10j`), and the key
    /// after `@`, which names what to repeat.
    fn unmapped(&self, key: Key) -> bool {
        let normal = matches!(self.mode, Mode::Normal | Mode::Visual | Mode::BlockNormal);
        let count_zero =
            key == Key::char('0') && self.pending.count.is_some() && self.pending.keys.is_empty();
        let after_at = self.mode == Mode::Normal && self.pending.keys == [Key::char('@')];
        normal && (count_zero || after_at)
    }

    /// `keys` with `<Leader>` replaced by the `leader` setting.
    pub(super) fn resolve(&self, keys: &[MapKey]) -> Vec<Key> {
        keys.iter()
            .map(|k| match k {
                MapKey::Key(key) => *key,
                MapKey::Leader => self.settings.leader,
            })
            .collect()
    }

    /// Runs a mapping command or `:set`: a message to show if it has one, or why it failed.
    /// Other commands do nothing here.
    pub(super) fn run_setup(&mut self, command: &Command) -> Result<Option<String>, String> {
        match command {
            Command::Map {
                modes,
                noremap,
                nowait,
                lhs,
                rhs,
            } => {
                let mapping = Mapping {
                    lhs: self.resolve(lhs),
                    rhs: self.resolve(rhs),
                    noremap: *noremap,
                    nowait: *nowait,
                };
                self.keymap.define(modes, &mapping).map(|()| None)
            }
            Command::ShowMaps { modes, lhs } => {
                Ok(Some(self.keymap.show(modes, &self.resolve(lhs))))
            }
            Command::Unmap { modes, lhs } => {
                let lhs = self.resolve(lhs);
                if self.keymap.remove(modes, &lhs) {
                    Ok(None)
                } else {
                    Err(format!("no mapping for `{}`", format_keys(&lhs)))
                }
            }
            Command::MapClear { modes } => {
                for mode in *modes {
                    self.keymap.mappings_mut(*mode).clear();
                }
                Ok(None)
            }
            Command::Set(args) => Ok(self.set(args)),
            _ => Ok(None),
        }
    }
}
