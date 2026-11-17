//! Test-only scratch directories for cache and manager tests.
//!
//! Scratch paths are deterministic, unique per process and label, and always
//! stay inside the workspace: under `$CARGO_TARGET_DIR/media-test-scratch` when
//! that variable is present, otherwise under `<workspace>/.work`. The
//! [`ScratchDir`] guard removes its directory on drop, including on panic.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Unique temporary directory inside the workspace.
pub(crate) struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    pub(crate) fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = scratch_root().join(format!("{label}-{}-{sequence}", std::process::id()));
        std::fs::create_dir_all(&path).expect("create scratch directory");
        Self { path }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

static PROCESS_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Serialize tests that spawn external processes.
///
/// The suite runs in parallel; spawning many shell/fake-decoder processes at
/// once can transiently fail under a busy machine (EAGAIN). Production spawns
/// one decoder and one audio process, so serializing the process-based tests
/// keeps them deterministic without weakening what they cover.
pub(crate) fn process_test_lock() -> std::sync::MutexGuard<'static, ()> {
    PROCESS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Write an executable shell script under `dir` and return its path.
///
/// Only available on Unix; the whole media test suite runs on the Linux
/// desktop target. Scripts ignore their arguments by convention so the same
/// fake can stand in for ffmpeg or ffplay.
#[cfg(unix)]
pub(crate) fn write_script(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = dir.join(name);
    std::fs::write(&path, body).expect("write test script");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("make test script executable");
    path
}

fn scratch_root() -> PathBuf {
    if let Some(dir) = std::env::var_os("CARGO_TARGET_DIR") {
        return PathBuf::from(dir).join("media-test-scratch");
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.work/media-test-scratch")
}
