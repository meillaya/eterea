# Build and render performance

These are measurements from the development host, not release certification.
The reference host is x86_64 Linux with a rustup-free Rust 1.99.1 toolchain.

## Build profile

The workspace ships one release profile with `lto = "fat"`, one codegen unit,
`strip = true`, `panic = "abort"` and no incremental compilation. That profile
trades compile time for runtime speed and a smaller installed binary.

## mbx build space

The workspace carries `.mbx.toml` for [mbx](https://mr-boxington.jdx.dev).
Measured with the mbx cache under the workspace (`.work/cache/mbx`), not in the
home directory:

| Measurement | Result |
|:---|:---|
| Cold debug managed build, `mbx build -p eterea-desktop` | 1 min 26 s |
| Cold release managed build, `mbx build -p eterea-desktop --release` | 4 min 40 s |
| Release build in `devenv shell -- cargo build -p eterea-desktop --release` | 4 min 14 s cold; 2 min 10 s after source-only changes |
| Release binary, stripped | 28 MiB |
| Managed target after debug and release | 4.6 GiB |
| Shared action store | 4.5 GiB in 5,961 objects |
| Total mbx cache directory | 9.2 GiB |
| Plain Cargo target directory on this host | 21 GiB (all dev/test artifacts, not one build) |
| `mbx clean` of the debug target, then warm rebuild | 4.56 s wall |
| Warm rebuild cache hits | 448, 0 misses |
| Estimated compiler time avoided | 10 min 10 s |

`mbx clean` removes the managed target but keeps the shared store. The next
build restores matching outputs from the store, so the target is rebuilt from
cache rather than from scratch. Machine-specific cache budgets stay in the
user's global mbx configuration; the repository only carries workspace policy.

## Headless render

Rune's headless compositor with Mesa lavapipe reports its own frame timings.
These are software-rendered, so they are useful for regressions, not for
predicting a discrete GPU:

| Capture | Result |
|:---|:---|
| Feed at 1280x820, images loaded | GPU init about 260 ms; frame CPU p50 about 10 ms |
| Video at 960x540, ffmpeg decode | frame CPU p50 about 19 ms |
| Empty/onboarding at 1280x820 | layout and frame CPU under 10 ms |

The first frame includes process and GPU initialization. Steady-state frame
costs are dominated by software rasterization and image decode.

## Application benchmarks

The service layer has explicit, gated benchmarks rather than implicit timing
claims:

| Benchmark | Command | Budget |
|:---|:---|:---|
| 1,200-tag graph build (import + restricted co-occurrence query + force layout + normalization) | `cargo test -p eterea-app --release --lib release_large_graph_benchmark -- --ignored --nocapture` | < 1,000 ms release |
| Feed and filtered-feed pages on a file-backed archive | `scripts/perf-baseline.sh` (see its `--help`; stress mode via `--stress <count>`) | Reported by the baseline, no hard gate |
| Backend query baselines (search, calendar, graph) | `cargo test -p eterea-core --release --test v2_benchmarks -- --ignored` | Reported with release gate labels in the test names |

Measured on the reference host in a release build (rustc release profile):
**1,200 nodes / 1,200 edges in 520 ms** for the whole import-free build pass
(restricted co-occurrence query, deterministic force layout, normalization). The
graph benchmark prints node/edge counts and elapsed time; the ignored test also
asserts the 1,000 ms budget so a regression fails CI when the release-like
gate is run. Debug builds are slower and are intentionally not gated.

## Reproduce

```bash
# release application benchmark (release profile)
cargo test -p eterea-app --release --lib release_large_graph_benchmark -- --ignored --nocapture

# mbx, with its store kept inside the workspace
XDG_CACHE_HOME=$PWD/.work/cache XDG_CONFIG_HOME=$PWD/.work/mbx-config \
  mbx build -p eterea-desktop
XDG_CACHE_HOME=$PWD/.work/cache XDG_CONFIG_HOME=$PWD/.work/mbx-config mbx stats

# headless render
ETEREA_DEMO=1 ETEREA_LOAD_MEDIA=1 target/debug/eterea \
  --snap /tmp/feed.png --theme light --size 1280x820 --at 2
```


## v2 data and startup

Measured on the reference host after the schema v2/data-v2 work.

| Measurement | Result |
|:---|:---|
| 10k CSV parse + write + 10k export benchmark | 8 ms parse, 315 ms insert, 48 ms CSV export, 46 ms JSON export; parse+write 56 ms in the benchmark harness |
| 10k release CSV export | 5,556,365 bytes in 81 ms |
| 10k release JSON export | 7,526,277 bytes in 82 ms |
| First frame after background startup (headless, 1280x820, real archive) | about 16 ms |
| Debug app initialization moved off first frame | store open/feed in a worker; setup no SQLite or image wait |

The 10k budget is end-to-end parse plus one SQLite write transaction. The
benchmark reports its actual times; repeated local runs stayed well under one
second.

## Media and video (v2)

The image cache and video cache are bounded and content-addressed. Repeated
plays reuse the local video file; ffmpeg/ffplay never receive a remote URL. The
media crate reports 105 passing tests and one ignored network smoke test.

Controls for the current frame costs are documented in the source and in
`docs/verification/ui-harness.md`; headless numbers remain software-rendered.
