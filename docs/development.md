# Development

## Environment

Use the devenv shell. It pins Rust 1.99.1 (Rune's pinned revision needs the
stabilized language features), supplies the native libraries Rune needs, and
`ffmpeg` for video playback:

```bash
devenv shell
```

With direnv, entering the checkout activates the same shell through `.envrc`.
The host in this workspace also has a workspace-local Rust cache at
`.work/cargo-home` and a target directory at `.work/rune-spike/target`; they are
not part of the repository.

## Commands

```bash
# Run the desktop app
devenv shell -- cargo run -p eterea-desktop

# Check the workspace
devenv shell -- cargo check --workspace

# Tests
devenv shell -- cargo test --workspace

# Release build
devenv shell -- cargo build -p eterea-desktop --release

# Verification gate (pinned tools in .tools/)
bash scripts/verify/all.sh --strict --fast
FUZZ_TIME=10 bash scripts/verify/fuzz.sh
```

`verification/kani` and `verification/miri` are standalone crates with their own
lockfiles and are intentionally not workspace members. Run them through the
scripts above or `scripts/verify/ci.sh`.

Formatting and clippy require rustfmt and clippy in the active toolchain. This
host's rustup-free toolchain does not include them; devenv's toolchain does.

## Live checks

Run the app on a graphical session and check:

- the window opens decorated at 1280x820 and resizes to the 960x640 minimum
- an empty data directory shows the onboarding state and a real file picker
- CSV, JSON and X archive imports preview before writing and skip duplicates
- search filters while typing; Enter opens the first result
- `j`/`k`, `Enter`, `Esc`, `/`, `:`, `?`, `f`, `c`, `o`, `m` and `Space` work
- author and tag filters return to the feed and clear with `Esc`
- the graph and calendar routes use the archive data
- light, dark and text-size choices survive a restart
- remote images stay hidden until `m`, and images open in the lightbox
- video downloads through the app and plays with the system `ffmpeg`

## Test-only switches

These environment variables exist for headless captures and tests; they are not
user settings:

| Variable | Effect |
|:---|:---|
| `ETEREA_DEMO=1` | Use the local eight-post deck and bundle assets; no database or network. |
| `ETEREA_ROUTE=feed\|authors\|topics\|graph\|calendar\|import\|settings\|detail` | Start on a route. |
| `ETEREA_SELECTED=N` | Select a feed index after the first page loads. |
| `ETEREA_LOAD_MEDIA=1` | Load every post's media after the feed arrives. |
| `ETEREA_PLAY=1` | Start the selected post's video after the feed arrives. |
| `ETEREA_IMPORT=PATH` | Preview a file without opening the picker. |
| `ETEREA_DB=PATH` | Open a specific SQLite database instead of the platform default. |
| `ETEREA_CONFIG_DIR=PATH` | Keep `settings.json` in a test directory. |

Headless rendering needs a Vulkan adapter. Without a GPU, Mesa lavapipe works:

```bash
export VK_ICD_FILENAMES=/nix/store/1vkzwrp4ny20iljfnv9valq0bfn8czlv-mesa-26.2.3/share/vulkan/icd.d/lvp_icd.x86_64.json
export LD_LIBRARY_PATH=/nix/store/1vkzwrp4ny20iljfnv9valq0bfn8czlv-mesa-26.2.3/lib
```

## UI captures

The Rune compositor can render without a window:

```bash
.work/rune-spike/target/debug/eterea \
  --snap docs/assets/previews/feed-light.png \
  --theme light --size 1280x820 --at 2
```

Scenes are listed with `--list-scenes`. The headless UI harness runs the full
scene and route matrix; see
[`verification/ui-harness.md`](verification/ui-harness.md).

## Build storage

The workspace carries an `.mbx.toml` policy for
[mbx](https://mr-boxington.jdx.dev). With mbx installed:

```bash
mbx build -p eterea-desktop --release
mbx stats
```

Do not commit machine-specific mbx paths or cache budgets. The repository policy
contains only workspace-wide build settings.
