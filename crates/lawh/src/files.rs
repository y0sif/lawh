//! Board files: what `lawh [FILE]` opens, and writing a board for `:w`.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, anyhow};
use lawh_core::format::MAX_FILE_BYTES;
use lawh_core::{Board, FormatError};

/// Names [`write_atomic`] tries for its temporary file before giving up: one per try, in case
/// an earlier write left one behind (a crash) or another lawh writes the same file.
const TEMP_NAME_TRIES: u32 = 100;

/// The board lawh edits, and where it came from.
#[derive(Debug)]
pub struct Opened {
    pub board: Board,
    /// The file `:w` writes; `None` for an unnamed board, which `:w name.lawh` names.
    pub path: Option<PathBuf>,
    /// `path` does not exist yet: the board starts empty and `:w` creates the file.
    pub new_file: bool,
}

/// What `lawh [FILE]` opens: the board in `file`; an empty board named `file` when there is no
/// such file (as Vim opens a new file); an empty unnamed board without `file`.
///
/// Errors (a file that can't be read, a directory, a board that doesn't parse) read
/// `<path>: <io error>` or `<path>:<line>:<col>: <message>`.
pub fn plan_open(file: Option<&Path>) -> anyhow::Result<Opened> {
    let Some(path) = file else {
        return Ok(Opened {
            board: Board::new(),
            path: None,
            new_file: false,
        });
    };
    let board = match File::open(path) {
        Ok(file) => read_board(path, file)?,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Ok(Opened {
                board: Board::new(),
                path: Some(path.to_owned()),
                new_file: true,
            });
        }
        Err(err) => return Err(anyhow!(err).context(path.display().to_string())),
    };
    Ok(Opened {
        board,
        path: Some(path.to_owned()),
        new_file: false,
    })
}

/// Reads and parses the board in `file`, opened from `path`.
///
/// The file's size is checked before anything is read, so a huge file is refused without being
/// loaded into memory. Reading stops one byte past the limit, in case the file grows meanwhile
/// or reports no size (a pipe).
fn read_board(path: &Path, file: File) -> anyhow::Result<Board> {
    let in_path = || path.display().to_string();
    let len = file.metadata().with_context(in_path)?.len();
    let limit = u64::try_from(MAX_FILE_BYTES).unwrap_or(u64::MAX);
    if len > limit {
        let err = FormatError::FileTooLarge {
            bytes: usize::try_from(len).unwrap_or(usize::MAX),
        };
        return Err(anyhow!("{}:{err}", path.display()));
    }
    let mut src = String::new();
    file.take(limit.saturating_add(1))
        .read_to_string(&mut src)
        .with_context(in_path)?;
    // FormatError's message already starts with `line:column:`.
    lawh_core::from_lawh(&src).map_err(|err| anyhow!("{}:{err}", path.display()))
}

/// The name the engine knows the board's file by: the path as given. A path that isn't UTF-8
/// gets a lossy name; [`write_target`] maps that name back to the real path.
pub fn engine_name(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The file a `:w` of `path` (as the engine names it) writes: the opened file when `path` is
/// its engine name (`opened` pairs the two), else `path` itself. A relative path is taken from
/// `launch_dir`, the directory lawh was started in, when there is one.
pub fn write_target(
    path: &str,
    opened: Option<(&str, &Path)>,
    launch_dir: Option<&Path>,
) -> PathBuf {
    let path = match opened {
        Some((name, real)) if name == path => real.to_owned(),
        _ => PathBuf::from(path),
    };
    match launch_dir {
        Some(dir) => dir.join(path),
        None => path,
    }
}

/// Writes `contents` to `path` so that the file is either the old one or the new one, never
/// half written: into a new temporary file in the same directory, flushed to disk, then renamed
/// over `path`. On an error the temporary file is removed and `path` is untouched.
///
/// A symbolic link is written through: the file it points at is replaced and the link stays.
/// On Unix the new file keeps the old one's permissions.
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let name = target.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "the path does not name a file")
    })?;
    let dir = match target.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    let (temp, mut file) = create_temp(dir, name)?;
    let synced = file.write_all(contents).and_then(|()| {
        keep_permissions(&target, &file);
        file.sync_all()
    });
    // Closed before the rename: Windows can't rename or remove an open file.
    drop(file);
    let written = synced.and_then(|()| fs::rename(&temp, &target));
    if written.is_err()
        && let Err(err) = fs::remove_file(&temp)
    {
        tracing::warn!(path = %temp.display(), %err, "could not remove a temporary file");
    }
    written
}

/// A new file in `dir` named after `name`, hidden on Unix: `.area.lawh.<pid>-<n>.tmp`.
fn create_temp(dir: &Path, name: &std::ffi::OsStr) -> io::Result<(PathBuf, File)> {
    let pid = std::process::id();
    let mut last = None;
    for n in 0..TEMP_NAME_TRIES {
        let mut temp = OsString::from(".");
        temp.push(name);
        temp.push(format!(".{pid}-{n}.tmp"));
        let temp = dir.join(temp);
        match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(file) => return Ok((temp, file)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => last = Some(err),
            Err(err) => return Err(err),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("no name left for a temporary file")))
}

/// Gives the new file `file` the permissions of `target`, the file it replaces, if there is
/// one. Unix only: on Windows the only permission is read-only, and a read-only file can't be
/// replaced anyway. A failure is logged, not fatal: the board is still written.
#[cfg(unix)]
fn keep_permissions(target: &Path, file: &File) {
    let Ok(old) = fs::metadata(target) else {
        return;
    };
    if let Err(err) = file.set_permissions(old.permissions()) {
        tracing::warn!(path = %target.display(), %err, "could not keep the file's permissions");
    }
}

#[cfg(not(unix))]
fn keep_permissions(_target: &Path, _file: &File) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_dir::TestDir;

    #[test]
    fn no_file_opens_an_unnamed_empty_board() {
        let opened = plan_open(None).unwrap();
        assert!(opened.board.is_empty());
        assert_eq!(opened.path, None);
        assert!(!opened.new_file);
    }

    #[test]
    fn a_missing_file_opens_an_empty_board_named_after_it() {
        let dir = TestDir::new("missing");
        for path in [
            dir.path("new.lawh"),
            dir.path("no-such-dir").join("new.lawh"),
        ] {
            let opened = plan_open(Some(&path)).unwrap();
            assert!(opened.board.is_empty());
            assert_eq!(opened.path.as_deref(), Some(path.as_path()));
            assert!(opened.new_file);
            assert!(!path.exists(), "opening must not create the file");
        }
    }

    #[test]
    fn an_existing_file_opens_its_board() {
        let dir = TestDir::new("existing");
        let path = dir.write("a.lawh", "lawh version=1\ncircle id=c1 x=0 y=0 r=3\n");
        let opened = plan_open(Some(&path)).unwrap();
        assert_eq!(opened.board.len(), 1);
        assert_eq!(opened.path.as_deref(), Some(path.as_path()));
        assert!(!opened.new_file);
    }

    #[test]
    fn a_bad_file_or_a_directory_is_an_error_naming_the_path() {
        let dir = TestDir::new("bad");
        let bad = dir.write("bad.lawh", "lawh version=1\n\nstar id=s1 x=0 y=0\n");
        let err = format!("{:#}", plan_open(Some(&bad)).unwrap_err());
        assert!(
            err.starts_with(&format!(
                "{}:3:1: unknown object kind `star`",
                bad.display()
            )),
            "{err}"
        );
        let err = format!("{:#}", plan_open(Some(dir.root())).unwrap_err());
        assert!(
            err.starts_with(&format!("{}: ", dir.root().display())),
            "{err}"
        );
    }

    #[test]
    fn writes_go_to_the_opened_file_or_from_the_launch_directory() {
        let launch = Path::new("work");
        let real = Path::new("boards").join("a.lawh");
        let opened = Some(("boards/a.lawh", real.as_path()));
        assert_eq!(
            write_target("boards/a.lawh", opened, Some(launch)),
            launch.join(&real)
        );
        assert_eq!(
            write_target("b.lawh", opened, Some(launch)),
            launch.join("b.lawh")
        );
        assert_eq!(write_target("b.lawh", None, None), Path::new("b.lawh"));
        let absolute = std::env::temp_dir().join("c.lawh");
        let absolute_name = absolute.to_str().unwrap();
        assert_eq!(write_target(absolute_name, None, Some(launch)), absolute);
    }

    #[cfg(unix)]
    #[test]
    fn a_path_that_is_not_utf8_is_written_as_given() {
        use std::os::unix::ffi::OsStringExt as _;
        let real = PathBuf::from(OsString::from_vec(vec![b'b', 0xFF, b'.', b'l']));
        let name = engine_name(&real);
        assert_ne!(Path::new(&name), real);
        assert_eq!(write_target(&name, Some((&name, &real)), None), real);
    }

    /// The directory's file names, sorted.
    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_write_creates_or_replaces_the_file_and_leaves_nothing_else() {
        let dir = TestDir::new("write");
        let path = dir.path("a.lawh");
        write_atomic(&path, b"one\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "one\n");
        write_atomic(&path, b"two\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "two\n");
        assert_eq!(listing(dir.root()), ["a.lawh"]);
    }

    #[test]
    fn a_failed_write_leaves_no_temporary_file() {
        let dir = TestDir::new("fail");
        let missing = dir.path("no-such-dir").join("a.lawh");
        assert_eq!(
            write_atomic(&missing, b"x").unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        // Renaming a file over a directory fails after the temporary file is written.
        let sub = dir.path("sub.lawh");
        fs::create_dir(&sub).unwrap();
        write_atomic(&sub, b"x").unwrap_err();
        assert!(sub.is_dir());
        assert_eq!(listing(dir.root()), ["sub.lawh"]);
    }

    #[test]
    fn a_leftover_temporary_file_is_skipped() {
        let dir = TestDir::new("leftover");
        let leftover = dir.write(&format!(".a.lawh.{}-0.tmp", std::process::id()), "old");
        let path = dir.path("a.lawh");
        write_atomic(&path, b"new").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        assert_eq!(fs::read_to_string(&leftover).unwrap(), "old");
    }

    #[cfg(unix)]
    #[test]
    fn a_write_keeps_permissions_and_symbolic_links() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = TestDir::new("unix");
        let real = dir.write("real.lawh", "old");
        fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
        let link = dir.path("link.lawh");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        write_atomic(&link, b"new").unwrap();
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(fs::read_to_string(&real).unwrap(), "new");
        let mode = fs::metadata(&real).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(listing(dir.root()), ["link.lawh", "real.lawh"]);
    }
}
