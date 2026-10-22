//! Shared isolation checks for opt-in performance tests. Test-only.
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) const PERF_ROOT: &str = r"D:\Ai\illuTag-refactor-work\perf-million-hires";

#[cfg(windows)]
fn assert_no_reparse(path: &Path) {
    use std::os::windows::fs::MetadataExt;
    let mut current = Some(path);
    while let Some(candidate) = current {
        if candidate.exists() {
            let metadata = fs::symlink_metadata(candidate).unwrap();
            assert_eq!(
                metadata.file_attributes() & 0x400,
                0,
                "perf path must not contain a reparse point: {}",
                candidate.display()
            );
        }
        current = candidate.parent();
    }
}

#[cfg(not(windows))]
fn assert_no_reparse(_path: &Path) {}

pub(crate) fn run_root() -> PathBuf {
    let raw = env::var_os("ILLUTAG_PERF_RUN_DIR")
        .map(PathBuf::from)
        .expect("ILLUTAG_PERF_RUN_DIR must point at an isolated perf run directory");
    assert!(raw.exists(), "perf run directory must already exist");
    let root = fs::canonicalize(&raw).unwrap();
    let display = root.to_string_lossy();
    let normalized = Path::new(display.strip_prefix(r"\\?\").unwrap_or(&display));
    let allowed = Path::new(PERF_ROOT);
    let relative = normalized
        .strip_prefix(allowed)
        .expect("perf data must stay under the isolated perf root");
    assert!(
        !relative.as_os_str().is_empty(),
        "perf data must be a child of {}, not the root itself",
        allowed.display()
    );
    assert!(
        !normalized
            .to_string_lossy()
            .to_lowercase()
            .contains("danbooru"),
        "refusing to use a real library path: {}",
        normalized.display()
    );
    assert_no_reparse(&root);
    assert!(
        root.join(".perf-isolated").is_file(),
        "runner must create the ownership marker before the test starts"
    );
    fs::write(root.join("test.pid"), std::process::id().to_string()).unwrap();
    root
}

/// Cold subdirectory: must not exist, and its existing ancestors must not be
/// reparse points. The runner already checks these paths; direct opt-in test
/// execution must enforce the same rule before the first write.
pub(crate) fn cold_dir(root: &Path, label: &str) -> PathBuf {
    assert!(
        !label.contains(std::path::MAIN_SEPARATOR) && !label.contains('/'),
        "perf subdirectory label must be a single component"
    );
    let path = root.join(label);
    assert!(
        !path.exists(),
        "cold perf subdirectory already exists: {}",
        path.display()
    );
    assert_no_reparse(root);
    assert_no_reparse(path.parent().unwrap_or(root));
    fs::create_dir_all(&path).unwrap();
    assert_no_reparse(&path);
    path
}
