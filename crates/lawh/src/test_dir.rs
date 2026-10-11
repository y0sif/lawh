//! A directory of files for one test, in the system temp directory, removed when dropped.

use std::fs;
use std::path::{Path, PathBuf};

pub struct TestDir(PathBuf);

impl TestDir {
    /// An empty directory; `name` must differ between tests, which run at the same time.
    pub fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("lawh-app-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub fn root(&self) -> &Path {
        &self.0
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    /// Writes a file in the directory and returns its path.
    pub fn write(&self, name: &str, contents: impl AsRef<[u8]>) -> PathBuf {
        let path = self.path(name);
        fs::write(&path, contents).unwrap();
        path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
