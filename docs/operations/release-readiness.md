# Release readiness

Status: **draft release candidate**, not a public release.

The archive path, the Rune shell, the release workflow and the verification gate
run locally. The desktop is rendered headlessly for the UI matrix; a live
graphical session and a GitHub-hosted release dispatch are still unproven.

## Local gates

```bash
devenv shell -- cargo test --workspace
devenv shell -- cargo build -p eterea-desktop --release
bash scripts/verify/ci.sh
python3 scripts/validate-github-workflows.py
python3 scripts/check-doc-links.py README.md 'docs/**/*.md'
```

| Gate | Evidence | Limit |
|:---|:---|:---|
| Workspace tests | `cargo test --workspace` passes on the reference host. | Host toolchain has no rustfmt or clippy; devenv supplies them. |
| Verification | `scripts/verify/ci.sh` passes Verus, the five gated Kani harnesses and Miri; `scripts/verify/fuzz.sh` passes URL and import targets. | Seven expensive URL/MIME Kani harnesses are advisory only. See `verification/COVERAGE.md`. |
| UI matrix | Headless captures and accessibility-tree checks in `verification/ui/`; 82/82 pass with no clipping warnings. | Software Vulkan (lavapipe); interaction flows are script-captured, not exhaustively driven. |
| Draft release workflow | YAML validates; builds and packages `eterea` with bundled non-GPU runtime libs, icons, desktop entry and checksums; direct ELF and launcher pass fresh-Xvfb `--snap/--tree` and process smokes. | No GitHub-hosted run has been observed. The artifact is unsigned. |

## Known gaps

- HLS/DASH playback and live X sync are not shipped.
- Video audio and frames need `ffmpeg`/`ffplay` on `PATH`; ffmpeg is trusted.
- The release tarball is unsigned and has no package-manager installer.
- A live desktop screenshot on the target display has not been captured in this
  workspace; the packaged binary is verified under Xvfb/lavapipe instead.
- The bundle relies on the host glibc (2.39+) and GPU stack, like an AppImage.

## Release checklist

1. Run the local gates above on the tag commit.
2. Run `bash scripts/ui-harness.sh` after generating the binary.
3. Run `bash scripts/package-linux-release.sh` and
   `scripts/smoke-linux-bundle.sh dist/eterea-* --snap --tree --icd <lavapipe.json>`.
4. Dispatch the tag workflow and compare the uploaded artifact checksum with
   `SHA256SUMS.txt`.
5. Open the app on a graphical session, import a fixture, and walk the manual
   checks in `docs/development.md`.
6. Replace this page's status line only when those steps pass.
