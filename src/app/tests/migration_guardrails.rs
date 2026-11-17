//! Guardrails keeping this app crate on the current workspace layout.
//!
//! The Dioxus shell is gone; the active desktop runtime is the Rune shell in
//! `src/desktop`, with `eterea-app` as the shared service and store boundary.
//! These tests read the workspace manifests so a crate move or a legacy shell
//! reintroduction fails here instead of in CI discovery.

use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("app crate should have repo parent")
        .parent()
        .expect("src directory should have repo parent")
        .to_path_buf()
}

fn read(relative: &str) -> String {
    fs::read_to_string(repo_root().join(relative)).expect("file should be readable")
}

#[test]
fn workspace_contains_the_rune_workspace_members() {
    let cargo_toml = read("Cargo.toml");
    for member in [
        "src/backend",
        "src/app",
        "src/logic",
        "src/ui-model",
        "src/media",
        "src/desktop",
    ] {
        assert!(
            cargo_toml.contains(&format!("\"{member}\"")),
            "workspace must include {member}"
        );
    }
    assert!(
        !cargo_toml.contains("\"src/dioxus-app\""),
        "the removed Dioxus shell must not be a workspace member"
    );
    assert!(!cargo_toml.contains("\"src-tauri\""));
}

#[test]
fn run_path_is_the_rune_desktop_binary() {
    let desktop_manifest = read("src/desktop/Cargo.toml");
    assert!(desktop_manifest.contains("name = \"eterea-desktop\""));
    assert!(desktop_manifest.contains("name = \"eterea\""));
    assert!(
        repo_root().join("src/desktop/src/main.rs").exists(),
        "the desktop entry point must exist"
    );
}

#[test]
fn legacy_shell_directories_are_removed() {
    let root = repo_root();
    assert!(!root.join("src-tauri").exists());
    assert!(!root.join("src/frontend").exists());
}

#[test]
fn readme_has_no_legacy_run_path() {
    let readme = read("README.md");
    assert!(!readme.contains("cargo tauri"));
    assert!(!readme.contains("bun run"));
}
