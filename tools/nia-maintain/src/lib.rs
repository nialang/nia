pub mod audit;
pub mod report;

use std::path::{Path, PathBuf};

pub type MaintainResult<T> = Result<T, String>;

pub fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("nia-maintain must live under the repository tools directory")
}

pub fn parse_usize(value: &str, option: &str) -> MaintainResult<usize> {
    value
        .parse()
        .map_err(|_| format!("{option} requires a non-negative integer, found {value:?}"))
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    pub struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        pub fn new(name: &str) -> Self {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("nia-maintain-{name}-{}-{id}", std::process::id()));
            fs::create_dir_all(&path).expect("create test directory");
            Self { path }
        }

        pub fn path(&self) -> &Path {
            &self.path
        }

        pub fn write(&self, relative: &str, contents: &str) {
            let path = self.path.join(relative);
            fs::create_dir_all(path.parent().expect("test file parent"))
                .expect("create test file parent");
            fs::write(path, contents).expect("write test file");
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
