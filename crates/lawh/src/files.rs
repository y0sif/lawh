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

/// Most symbolic links [`write_atomic`] follows from the path it is given to the file it
/// writes, as Linux follows at most 40 (`MAXSYMLINKS`) when it opens a path: links that chain
/// further loop, and the write fails.
const MAX_LINK_HOPS: u32 = 40;

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

/// The writes of one batch of the engine's effects: those one key or timeout asked for.
///
/// The board does not change while the app does a batch's effects (the engine changes it only
/// while it takes a key, a timeout or a click), so every write of a batch writes the same bytes:
/// the board is turned into text once, at the first write, and each file is written once. A
/// later write to a file the batch has written already gets that write's result, without
/// writing again; the app still reports it to the engine. A key asks for at most
/// [`MAX_WRITES_PER_KEY`](lawh_core::engine::MAX_WRITES_PER_KEY) writes, so the files are few.
#[derive(Debug, Default)]
pub struct WriteBatch {
    /// The board as text, once a write has needed it.
    contents: Option<String>,
    /// Each file written, and how the write went.
    written: Vec<(PathBuf, Result<(), String>)>,
}

impl WriteBatch {
    /// Writes the board's text (`contents` makes it, the first time) to `target` with `write`,
    /// unless this batch has written `target` already: then how that write went.
    pub fn write(
        &mut self,
        target: PathBuf,
        contents: impl FnOnce() -> String,
        write: impl FnOnce(&Path, &[u8]) -> io::Result<()>,
    ) -> Result<(), String> {
        if let Some((_, result)) = self.written.iter().find(|(file, _)| *file == target) {
            return result.clone();
        }
        let contents = self.contents.get_or_insert_with(contents);
        let result = write(&target, contents.as_bytes()).map_err(|err| err.to_string());
        self.written.push((target, result.clone()));
        result
    }
}

/// Writes `contents` to `path` so that the file is either the old one or the new one, never
/// half written: into a new temporary file in the same directory, flushed to disk, then renamed
/// over `path`, and on Unix the directory flushed too, so the rename survives a power cut. On an
/// error the temporary file is removed and `path` is untouched.
///
/// A symbolic link is written through ([`follow_links`]): the file it points at is replaced,
/// or created when the link dangles, and the link stays. On Unix the new file has the old one's
/// permissions from the moment it is created, so the board is never more readable than it was;
/// a new file gets the default ones.
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    let target = follow_links(path)?;
    let name = target.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "the path does not name a file")
    })?;
    let dir = match target.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    // `target` is no link, so this is the file being replaced, if there is one.
    let old = fs::metadata(&target).ok().map(|meta| meta.permissions());
    let (temp, mut file) = create_temp(dir, name, old.as_ref())?;
    let synced = file.write_all(contents).and_then(|()| file.sync_all());
    // Closed before the rename: Windows can't rename or remove an open file.
    drop(file);
    let written = synced.and_then(|()| fs::rename(&temp, &target));
    match &written {
        Ok(()) => sync_dir(dir),
        Err(_) => {
            if let Err(err) = fs::remove_file(&temp) {
                tracing::warn!(path = %temp.display(), %err, "could not remove a temporary file");
            }
        }
    }
    written
}

/// The file a write to `path` replaces: `path` with each symbolic link at its end replaced by
/// what it points at (a relative target is taken from the link's directory), until a path that
/// is no link, or names nothing yet (a new file, or what a dangling link points at). Links in
/// the directories on the way are left to the system. Fails past [`MAX_LINK_HOPS`] links.
fn follow_links(path: &Path) -> io::Result<PathBuf> {
    let mut path = path.to_owned();
    // One look more than the hops: the path after the last hop may be no link.
    for _ in 0..=MAX_LINK_HOPS {
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let to = fs::read_link(&path)?;
                // Joining an absolute target gives the target itself.
                path = match path.parent() {
                    Some(dir) => dir.join(to),
                    None => to,
                };
            }
            Ok(_) => return Ok(path),
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(path),
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::other(format!(
        "{}: more than {MAX_LINK_HOPS} symbolic links in a row",
        path.display()
    )))
}

/// A new file in `dir` named after `name`, hidden on Unix: `.area.lawh.<pid>-<n>.tmp`. On Unix
/// it is created with `old`'s permissions when given (those of the file it will replace).
fn create_temp(
    dir: &Path,
    name: &std::ffi::OsStr,
    old: Option<&fs::Permissions>,
) -> io::Result<(PathBuf, File)> {
    let pid = std::process::id();
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    with_permissions(&mut options, old);
    let mut last = None;
    for n in 0..TEMP_NAME_TRIES {
        let mut temp = OsString::from(".");
        temp.push(name);
        temp.push(format!(".{pid}-{n}.tmp"));
        let temp = dir.join(temp);
        match options.open(&temp) {
            Ok(file) => {
                keep_permissions(&temp, &file, old);
                return Ok((temp, file));
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => last = Some(err),
            Err(err) => return Err(err),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("no name left for a temporary file")))
}

/// Creates files with the permission bits of `old`, if given; the umask may still take some
/// away, which [`keep_permissions`] puts back. Unix only: on Windows the only permission is
/// read-only, and a read-only file can't be replaced anyway.
#[cfg(unix)]
fn with_permissions(options: &mut OpenOptions, old: Option<&fs::Permissions>) {
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
    if let Some(old) = old {
        options.mode(old.mode() & 0o777);
    }
}

#[cfg(not(unix))]
fn with_permissions(_options: &mut OpenOptions, _old: Option<&fs::Permissions>) {}

/// Gives `file`, just created at `path` and still empty, exactly the permissions `old`, the
/// umask's change undone. A failure is logged, not fatal: the board is still written, with
/// permissions no wider than `old`'s.
#[cfg(unix)]
fn keep_permissions(path: &Path, file: &File, old: Option<&fs::Permissions>) {
    if let Some(old) = old
        && let Err(err) = file.set_permissions(old.clone())
    {
        tracing::warn!(path = %path.display(), %err, "could not keep the file's permissions");
    }
}

#[cfg(not(unix))]
fn keep_permissions(_path: &Path, _file: &File, _old: Option<&fs::Permissions>) {}

/// Flushes `dir` to disk after a rename in it, so the new name survives a power cut. Unix only
/// (Windows can't open a directory as a file). A failure is logged, not fatal: the file is
/// written and renamed already.
#[cfg(unix)]
fn sync_dir(dir: &Path) {
    if let Err(err) = File::open(dir).and_then(|dir| dir.sync_all()) {
        tracing::warn!(path = %dir.display(), %err, "could not flush the directory to disk");
    }
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) {}

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

    #[test]
    fn a_batch_makes_the_text_once_and_writes_each_file_once() {
        let mut batch = WriteBatch::default();
        let mut made = 0;
        let mut written = Vec::new();
        let mut results = Vec::new();
        for name in [
            "a.lawh", "b.lawh", "a.lawh", "bad.lawh", "b.lawh", "bad.lawh",
        ] {
            let result = batch.write(
                PathBuf::from(name),
                || {
                    made += 1;
                    "board".to_owned()
                },
                |target, bytes| {
                    written.push((target.to_owned(), bytes.to_vec()));
                    if target == Path::new("bad.lawh") {
                        Err(io::Error::other("disk full"))
                    } else {
                        Ok(())
                    }
                },
            );
            results.push(result);
        }
        assert_eq!(made, 1);
        let board = b"board".to_vec();
        assert_eq!(
            written,
            [
                (PathBuf::from("a.lawh"), board.clone()),
                (PathBuf::from("b.lawh"), board.clone()),
                (PathBuf::from("bad.lawh"), board),
            ]
        );
        // Every write has its result, a failed one too, in order.
        let failed = Err("disk full".to_owned());
        assert_eq!(
            results,
            [Ok(()), Ok(()), Ok(()), failed.clone(), Ok(()), failed]
        );
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
    fn the_temporary_file_has_the_old_permissions_before_anything_is_written() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = TestDir::new("temp-mode");
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o7777;
        // 0o666 is wider than the umask lets a new file be: it is put back.
        for old in [0o600, 0o640, 0o444, 0o666] {
            let name = format!("m{old:o}.lawh");
            let perms = fs::Permissions::from_mode(old);
            let (temp, file) = create_temp(dir.root(), name.as_ref(), Some(&perms)).unwrap();
            assert_eq!(file.metadata().unwrap().len(), 0);
            assert_eq!(mode(&temp), old, "{old:o}");
        }
        // A new file gets what any new file gets.
        let (temp, _file) = create_temp(dir.root(), "new.lawh".as_ref(), None).unwrap();
        let plain = dir.write("plain", "");
        assert_eq!(mode(&temp), mode(&plain));

        // A whole write: a private board stays private.
        let private = dir.write("private.lawh", "old");
        fs::set_permissions(&private, fs::Permissions::from_mode(0o600)).unwrap();
        write_atomic(&private, b"new").unwrap();
        assert_eq!(mode(&private), 0o600);
        // A failed directory flush is only logged.
        sync_dir(&dir.path("no-such-dir"));
    }

    #[cfg(unix)]
    #[test]
    fn a_dangling_link_is_written_through_to_a_new_file() {
        let dir = TestDir::new("dangling");
        let link = dir.path("link.lawh");
        std::os::unix::fs::symlink("missing.lawh", &link).unwrap();
        write_atomic(&link, b"new").unwrap();
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(fs::read_to_string(dir.path("missing.lawh")).unwrap(), "new");
        assert_eq!(listing(dir.root()), ["link.lawh", "missing.lawh"]);
    }

    #[cfg(unix)]
    #[test]
    fn links_are_followed_from_their_own_directory_and_in_chains() {
        let dir = TestDir::new("chain");
        fs::create_dir(dir.path("sub")).unwrap();
        let real = dir.write("real.lawh", "old");
        std::os::unix::fs::symlink("../real.lawh", dir.path("sub").join("link")).unwrap();
        let chain = dir.path("chain.lawh");
        std::os::unix::fs::symlink("sub/link", &chain).unwrap();
        write_atomic(&chain, b"new").unwrap();
        assert_eq!(fs::read_to_string(&real).unwrap(), "new");
        assert_eq!(listing(dir.root()), ["chain.lawh", "real.lawh", "sub"]);
        assert_eq!(listing(&dir.path("sub")), ["link"]);

        // As many links in a row as the system follows, and one more.
        for (links, ok) in [(MAX_LINK_HOPS, true), (MAX_LINK_HOPS + 1, false)] {
            let dir = TestDir::new(&format!("hops-{links}"));
            dir.write("l0", "old");
            for n in 1..=links {
                std::os::unix::fs::symlink(format!("l{}", n - 1), dir.path(&format!("l{n}")))
                    .unwrap();
            }
            let written = write_atomic(&dir.path(&format!("l{links}")), b"new");
            assert_eq!(written.is_ok(), ok, "{links} links: {written:?}");
            let want = if ok { "new" } else { "old" };
            assert_eq!(fs::read_to_string(dir.path("l0")).unwrap(), want);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_loop_of_links_fails_and_writes_nothing() {
        let dir = TestDir::new("loop");
        std::os::unix::fs::symlink("b", dir.path("a")).unwrap();
        std::os::unix::fs::symlink("a", dir.path("b")).unwrap();
        let err = write_atomic(&dir.path("a"), b"new").unwrap_err();
        assert!(err.to_string().contains("symbolic links"), "{err}");
        assert_eq!(listing(dir.root()), ["a", "b"]);
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
