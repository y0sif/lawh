//! Reading a lawhrc: `:set` and the mapping commands, one per line.

use std::collections::HashMap;

use super::Engine;
use super::command::{Command, parse_command};
use super::keymap::MapMode;
use crate::key::{Key, format_keys};

/// Largest lawhrc read, in bytes (1 MiB); a larger one is not read at all, with one warning.
///
/// A lawhrc of settings and mappings takes a few kilobytes. The cap keeps a wrong file given as
/// the rc (a log, a binary) cheap to turn down.
pub const MAX_RC_BYTES: usize = 1024 * 1024;

/// Most warnings [`Engine::source`] lists one by one (100); one more warning counts the rest.
///
/// A wrong file can have a problem on every line, thousands in all, and nobody reads past the
/// first few.
pub const MAX_RC_WARNINGS: usize = 100;

/// A line of a lawhrc that did not load as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RcWarning {
    /// The line, counting from 1; 0 for the file as a whole.
    pub line: usize,
    pub message: String,
}

/// The line that last mapped some keys in a mode, to warn when a later line maps them again.
type Mapped = HashMap<(MapMode, Vec<Key>), usize>;

impl Engine {
    /// Reads a lawhrc: one `:` command a line, the `:` optional. Blank lines and lines starting
    /// with `"` are comments. Only `:set` and the mapping commands (`:map`, `:unmap`,
    /// `:mapclear` and their families) are allowed. Nothing else happens: no effects, no
    /// message, no `@:`.
    ///
    /// Returns a warning for each line that did not load as written: a command that fails or is
    /// not allowed (the line is skipped), and keys mapped again in a mode (the later line
    /// wins). Every other line loads, so one bad mapping does not lose the rest. A file larger
    /// than [`MAX_RC_BYTES`] is not read.
    pub fn source(&mut self, rc: &str) -> Vec<RcWarning> {
        if rc.len() > MAX_RC_BYTES {
            return vec![RcWarning {
                line: 0,
                message: format!(
                    "the file is larger than {MAX_RC_BYTES} bytes, so it was not read"
                ),
            }];
        }
        // A byte order mark, as some Windows editors write.
        let rc = rc.strip_prefix('\u{feff}').unwrap_or(rc);
        let mut warnings = Vec::new();
        let mut more: Option<(usize, usize)> = None;
        let mut mapped = Mapped::new();
        for (i, line) in rc.lines().enumerate() {
            let n = i + 1;
            let Err(message) = self.source_line(line, n, &mut mapped) else {
                continue;
            };
            if warnings.len() < MAX_RC_WARNINGS {
                warnings.push(RcWarning { line: n, message });
            } else {
                more.get_or_insert((n, 0)).1 += 1;
            }
        }
        if let Some((line, count)) = more {
            warnings.push(RcWarning {
                line,
                message: format!("{count} lines from here on have problems too"),
            });
        }
        warnings
    }

    /// Runs line `n` of a lawhrc: why it did not load as written, if so.
    fn source_line(&mut self, line: &str, n: usize, mapped: &mut Mapped) -> Result<(), String> {
        let text = line.trim_start_matches(|c: char| c == ':' || c.is_whitespace());
        if text.is_empty() || text.starts_with('"') {
            return Ok(());
        }
        let command = parse_command(text).map_err(|e| e.to_string())?;
        match &command {
            Command::Map { modes, lhs, .. } => {
                let lhs = self.resolve(lhs);
                let earlier = modes.iter().find_map(|mode| {
                    let line = mapped.get(&(*mode, lhs.clone()))?;
                    Some((*mode, *line))
                });
                self.run_setup(&command)?;
                for mode in *modes {
                    mapped.insert((*mode, lhs.clone()), n);
                }
                match earlier {
                    Some((mode, line)) => Err(format!(
                        "`{}` was mapped in {} mode on line {line} already; this line replaces it",
                        format_keys(&lhs),
                        mode.name()
                    )),
                    None => Ok(()),
                }
            }
            Command::Unmap { modes, lhs } => {
                let lhs = self.resolve(lhs);
                self.run_setup(&command)?;
                for mode in *modes {
                    mapped.remove(&(*mode, lhs.clone()));
                }
                Ok(())
            }
            Command::MapClear { modes } => {
                self.run_setup(&command)?;
                mapped.retain(|(mode, _), _| !modes.contains(mode));
                Ok(())
            }
            Command::Set(_) => self.run_setup(&command).map(|_| ()),
            Command::ShowMaps { lhs, .. } if lhs.is_empty() => {
                Err("missing the keys to map and what they map to".to_owned())
            }
            Command::ShowMaps { lhs, .. } => Err(format!(
                "missing what `{}` maps to",
                format_keys(&self.resolve(lhs))
            )),
            _ => {
                let name = text.split_whitespace().next().unwrap_or_default();
                Err(format!(
                    "`:{name}` is not allowed in lawhrc, only `:set` and mapping commands are"
                ))
            }
        }
    }
}
