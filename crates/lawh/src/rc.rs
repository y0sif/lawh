//! lawhrc: settings and key maps the engine reads at start (see `Engine::source`).

use std::fs::File;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

use etcetera::{AppStrategy as _, AppStrategyArgs};
use lawh_core::engine::MAX_RC_BYTES;
use lawh_core::{Engine, RcWarning};

/// The lawhrc's file name in lawh's config directory.
const FILE_NAME: &str = "lawhrc";

/// Where lawh reads its lawhrc: `lawhrc` in lawh's config directory, `~/.config/lawh/lawhrc` on
/// Linux and macOS (XDG, `XDG_CONFIG_HOME` honoured) and `%APPDATA%\y0sif\lawh\config\lawhrc`
/// on Windows. `None` when the system has no home directory for the user.
pub fn path() -> Option<PathBuf> {
    let strategy = etcetera::choose_app_strategy(AppStrategyArgs {
        top_level_domain: "com".into(),
        author: "y0sif".into(),
        app_name: "lawh".into(),
    });
    match strategy {
        Ok(strategy) => Some(strategy.in_config_dir(FILE_NAME)),
        Err(err) => {
            tracing::warn!(%err, "no config directory, so no lawhrc is read");
            None
        }
    }
}

/// Reads the lawhrc at `path` into `engine`, before the first key. A missing file is no
/// lawhrc. Each warning (a line that did not load, or a file that could not be read) is
/// logged; the result is the status line message for them, if there are any.
pub fn source(engine: &mut Engine, path: &Path) -> Option<String> {
    let warnings = match read(path) {
        Ok(None) => return None,
        Ok(Some(text)) => {
            tracing::info!(path = %path.display(), "reading lawhrc");
            engine.source(&text)
        }
        Err(message) => vec![RcWarning { line: 0, message }],
    };
    for warning in &warnings {
        tracing::warn!(
            path = %path.display(),
            line = warning.line,
            "lawhrc: {}",
            warning.message
        );
    }
    summary(&warnings)
}

/// The text of the lawhrc at `path`: `Ok(None)` when there is no such file, `Err` with why it
/// was not read. A file larger than [`MAX_RC_BYTES`] is refused from its size, without being
/// read; reading stops one byte past the limit in case the size was wrong.
fn read(path: &Path) -> Result<Option<String>, String> {
    let cannot = |err: io::Error| format!("could not read {}: {err}", path.display());
    let file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(cannot(err)),
    };
    let too_large = || format!("the file is larger than {MAX_RC_BYTES} bytes, so it was not read");
    let limit = u64::try_from(MAX_RC_BYTES).unwrap_or(u64::MAX);
    if file.metadata().map_err(cannot)?.len() > limit {
        return Err(too_large());
    }
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(cannot)?;
    if bytes.len() > MAX_RC_BYTES {
        return Err(too_large());
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| "the file is not UTF-8 text, so it was not read".to_owned())
}

/// The status line for lawhrc warnings: the first one, and how many more the log has.
/// `lawhrc line 5: <message> (2 more warnings in the log)`, or `lawhrc: <message>` for the
/// file as a whole.
fn summary(warnings: &[RcWarning]) -> Option<String> {
    let first = warnings.first()?;
    let place = match first.line {
        0 => "lawhrc".to_owned(),
        line => format!("lawhrc line {line}"),
    };
    let more = match warnings.len() - 1 {
        0 => String::new(),
        1 => " (1 more warning in the log)".to_owned(),
        n => format!(" ({n} more warnings in the log)"),
    };
    Some(format!("{place}: {}{more}", first.message))
}

#[cfg(test)]
mod tests {
    use lawh_core::Board;

    use super::*;
    use crate::test_dir::TestDir;

    fn engine() -> Engine {
        Engine::new(Board::new())
    }

    #[test]
    fn the_path_is_lawhrc_in_the_config_directory() {
        if let Some(path) = path() {
            assert!(path.ends_with(Path::new("lawh").join("lawhrc")) || cfg!(windows));
            assert!(path.ends_with("lawhrc"), "{}", path.display());
        }
    }

    #[test]
    fn a_missing_lawhrc_is_silent() {
        let dir = TestDir::new("rc-missing");
        let mut e = engine();
        assert_eq!(source(&mut e, &dir.path("lawhrc")), None);
        assert_eq!(e.settings(), engine().settings());
    }

    #[test]
    fn a_good_lawhrc_changes_the_engine_quietly() {
        let dir = TestDir::new("rc-good");
        let rc = dir.write(
            "lawhrc",
            "\" comment\nset timeoutlen=500\nnnoremap U <C-r>\n",
        );
        let mut e = engine();
        assert_eq!(source(&mut e, &rc), None);
        assert_eq!(e.settings().timeoutlen, 500);
    }

    #[test]
    fn the_first_warning_is_shown_with_a_count_of_the_rest() {
        let dir = TestDir::new("rc-warnings");
        let mut e = engine();
        let one = dir.write("one", "set timeoutlen=500\nset nope\n");
        let shown = source(&mut e, &one).unwrap();
        assert!(shown.starts_with("lawhrc line 2: "), "{shown}");
        assert!(!shown.contains("more"), "{shown}");
        assert_eq!(e.settings().timeoutlen, 500, "the good line still loads");

        let three = dir.write("three", "set nope\nquit\n\nset nope\n");
        let shown = source(&mut engine(), &three).unwrap();
        assert!(shown.starts_with("lawhrc line 1: "), "{shown}");
        assert!(shown.ends_with(" (2 more warnings in the log)"), "{shown}");
    }

    #[test]
    fn a_huge_lawhrc_is_refused_without_being_read() {
        let dir = TestDir::new("rc-huge");
        let path = dir.path("lawhrc");
        // A gigabyte that takes no disk space: the size is set, nothing is written.
        File::create(&path).unwrap().set_len(1 << 30).unwrap();
        let started = std::time::Instant::now();
        assert_eq!(
            source(&mut engine(), &path).as_deref(),
            Some("lawhrc: the file is larger than 1048576 bytes, so it was not read")
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(5));

        // Sparse as well: the system temp directory may have a small quota.
        let at_limit = dir.path("at-limit");
        File::create(&at_limit)
            .unwrap()
            .set_len(u64::try_from(MAX_RC_BYTES).unwrap())
            .unwrap();
        assert_eq!(read(&at_limit).unwrap().unwrap().len(), MAX_RC_BYTES);
    }

    #[test]
    fn a_lawhrc_that_cannot_be_read_says_why() {
        let dir = TestDir::new("rc-unreadable");
        let binary = dir.write("binary", [b's', b'e', 0xFF, b't']);
        assert_eq!(
            source(&mut engine(), &binary).as_deref(),
            Some("lawhrc: the file is not UTF-8 text, so it was not read")
        );
        let shown = source(&mut engine(), dir.root()).unwrap();
        assert!(
            shown.starts_with(&format!(
                "lawhrc: could not read {}: ",
                dir.root().display()
            )),
            "{shown}"
        );
    }
}
