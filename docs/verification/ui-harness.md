# Headless Rune UI verification harness

Runtime checks for the `eterea` desktop shell, driven through Rune's headless
agent tooling. The harness runs the real binary, captures a PNG and an
accessibility tree per case, checks layout/copy/geometry, and compares image
hashes and canonical tree signatures against a small committed manifest.

This is **runtime UI validation, not a proof and not whole-app verification**.
It covers what a headless, scripted run can observe: rendering succeeds, the
accessibility tree has content, layout stays inside the window, copy passes the
repo lint, interactions change the surface deterministically, and visuals and
structure did not drift from the committed goldens.

## Files

| Path | Purpose |
| --- | --- |
| [`verification/ui/harness.py`](../../verification/ui/harness.py) | The harness (Python 3, stdlib only). |
| [`verification/ui/matrix.json`](../../verification/ui/matrix.json) | Matrix, expectations, limits, host driver defaults. |
| [`verification/ui/manifest.json`](../../verification/ui/manifest.json) | Golden SHA-256 image hashes, tree signatures and input hashes (`--update` writes it). |
| [`verification/ui/fixtures/x-api-v2-demo.json`](../../verification/ui/fixtures/x-api-v2-demo.json) | 8-post X API v2 fixture backing the main temp archive (posts, media, one quote, one external link). |
| [`verification/ui/fixtures/eterea-export-carousel.json`](../../verification/ui/fixtures/eterea-export-carousel.json) | eterea export JSON (format `eterea-bookmarks`, v2): one bookmark with twenty image attachments, no network URLs. |
| [`verification/ui/scripts/`](../../verification/ui/scripts) | Rune `--script` input files for the interaction captures. |
| [`scripts/ui-harness.sh`](../../scripts/ui-harness.sh) | Build wrapper for this checkout. |

Captures, logs and the aggregated copy-lint page are scratch output under
`.work/ui-harness/` (git-ignored); only the manifest, fixtures and scripts are
committed.

## Commands

```bash
# build + capture the whole matrix and rewrite the golden manifest
bash scripts/ui-harness.sh --update

# build + capture the whole matrix and verify the goldens (the normal check)
bash scripts/ui-harness.sh

# verify captures already on disk; runs no GPU/app binary
bash scripts/ui-harness.sh --no-build --verify-only

# subsets (useful while iterating)
bash scripts/ui-harness.sh --no-build --filter 'script/'
bash scripts/ui-harness.sh --no-build --scene hidden
bash scripts/ui-harness.sh --no-build --route history

# make clipping findings fail the run
bash scripts/ui-harness.sh --no-build --strict

# force or skip the cargo build
bash scripts/ui-harness.sh --build --update
bash scripts/ui-harness.sh --no-build
```

Environment overrides for the wrapper:

- `CARGO_HOME` / `CARGO_TARGET_DIR` — default to `.work/cargo-home` and
  `.work/rune-spike/target` in this checkout.
- `MESA_LAVAPIPE` — Nix store path of Mesa with lavapipe. The wrapper exports
  `VK_ICD_FILENAMES=<MESA_LAVAPIPE>/share/vulkan/icd.d/lvp_icd.x86_64.json` and
  `LD_LIBRARY_PATH=<MESA_LAVAPIPE>/lib` when they are not already set.
- `RUST_TOOLCHAIN_BIN` — a directory prepended to `PATH` for `cargo`/`rustc`
  (useful when the default `cargo` is a rustup shim with no toolchain), and
  `CARGO` to name the cargo binary.
- `--build` / `--no-build` — force or skip `cargo build -p eterea-desktop -p
  eterea-core`.

The wrapper builds by default so the harness tests current code. If cargo is
unusable in the current shell it falls back to the existing binary, but it
refuses a binary older than `src/` (a stale binary would verify the wrong code)
and tells you to fix the toolchain or pass `--build`.

Exit codes: `0` pass, `1` verification failure, `2` usage or environment error.

## Matrix

122 captures. Scenes run at all three sizes and both themes; routes, datasets and
scripts run at `1440x900` in both themes. Everything uses `--scale 1`, so PNG
pixels equal the requested point size.

| Group | Cases | How |
| --- | --- | --- |
| Scenes: `detail`, `selected`, `quote`, `video`, `hidden`, `palette`, `keys`, `searching`, `lightbox` | 9 x 3 sizes x 2 themes = **54** | `ETEREA_DEMO=1`, bundled demo deck, no database, no network |
| Routes: `feed`, `authors`, `topics`, `graph`, `calendar`, `import`, `settings`, `detail` | 8 x 1 size x 2 themes = **16** | main temp archive + `ETEREA_ROUTE` |
| Route sets: `data/empty/history` (empty archive) and `data/carousel/feed` (twenty-media archive) | 2 x 2 themes = **4** | secondary temp archives + `ETEREA_ROUTE` |
| Scripts: `export-sheet` (`e`), `selection-bar` (`x`, `shift+j`), `carousel-page-1` (`m`), `carousel-page-2` (`m`, `Right`) | 4 x 2 themes = **8** | main/carousel archives + Rune `--script` |

Sizes: `1440x900`, `1280x820`, `960x640` (the app's default, its `size`, and its
`min_size`). Themes: `light`, `dark`, passed as `--theme`; the tree's
`dark_system` must match.

Archives are built fresh per run and never touch the user's data:
`eterea-cli ingest` writes `fixtures/x-api-v2-demo.json` into
`.work/ui-harness/datasets/main`, the carousel fixture into
`.work/ui-harness/datasets/carousel`, and `eterea-cli stats` creates an empty
schema for `.work/ui-harness/datasets/empty`. Captures read them through
`ETEREA_DB`. Scene captures use `ETEREA_DEMO=1` and bundled
`src/desktop/assets/demo` media.

## What each case checks

- exit code 0, a non-empty PNG with the PNG signature and exact
  `width x height`, and a non-empty tree JSON;
- the tree reports the requested `viewport`, `title: "eterea"`,
  `dark_system` matching `--theme`, and an empty rune `warnings` list;
- the element list is non-empty, the root element rect covers the window, and
  no element has negative size;
- route and dataset captures expose the expected `route-<Name>` element key
  (`route-Feed`, `route-History`, `route-Settings`, ...);
- the accessibility root has non-zero bounds `[0, 0, W, H]`;
- no accessibility node has negative width/height, and no node without a
  `clips` state ancestor extends past the window (clipping/overflow signal);
- expected accessibility text exists: brand, the nav labels
  (`feed authors topics graph calendar import history settings`), and
  scene/route/script-specific copy — viewer actions (`favorited`, `copy link`,
  `open on x`, `edit tags`, `delete`), quote-card text, media alt labels
  (`Carousel frame 5`...), carousel controls (`‹`, `2/5`, `›`), selection bar
  (`2 selected`, `tag`, `delete`, `read`, `unread`, `clear`), export sheet
  (`scope`, `format`, `csv`, `json`, `cancel`), import instructions, settings
  copy (`media`, `load automatically`, `load on demand`), calendar/topic/graph
  content, and route history (`no actions yet`, `export archive`);
- expected **paint** text exists in the element list (visible copy even where
  the a11y tree omits an overlay layer);
- suppressed copy is absent: `feed` must not render raw `https://t.co/` URLs
  (external links show their expanded display URL instead);
- `python3 scripts/copy-lint.py` reports **0 errors** on the aggregate of every
  captured accessibility string and every captured visible text (banned
  phrases, exclamation marks, em dashes, all-caps labels). Copy-lint warnings do
  not fail the run;
- SHA-256 of the PNG and the canonical tree signature match `manifest.json`,
  and the hashes of `matrix.json`, the fixtures and the scripts that produced
  the manifest still match (an edit without `--update` fails loudly).

The tree signature (`tree_signature` in `harness.py`) hashes the accessibility
tree plus `viewport`, `title`, `dark_system` and `warnings` with sorted JSON
keys. It deliberately excludes wall-clock fields (`t`, `anim_t`), so it tracks
structure, names, bounds and states without racing the clock.

## Interactions captured by `--script`

Rune replays a JSON array of timestamped input events. The committed scripts
press keys at fixed virtual times, and the case's `--at` is set after the last
event so the resulting surface is captured:

| Script | Events | Captured surface |
| --- | --- | --- |
| `export-sheet.json` | `e` at 1.2 s | export sheet (scope all/filter, format csv/json, cancel/export) |
| `selection-bar.json` | `x` at 1.2 s, `shift+j` at 1.5 s | selection bar with `2 selected` and tag/delete/read/unread/clear |
| `carousel-page-1.json` | `m` at 1.2 s | twenty-media post, first carousel page, `‹ 1/5 ›` |
| `carousel-page-2.json` | `m` at 1.2 s, `Right` at 1.6 s | second carousel page, `‹ 2/5 ›`, frames 5-8 |

## Updating the goldens

Run `--update` after an intentional UI change and review the diff of
`verification/ui/manifest.json`. `--update` refuses to combine with a subset
filter and refuses to write a manifest from a failing run. If the manifest's
`inputs` hashes (matrix, fixtures, scripts) or a case's hashes differ,
verification reports the case and says to re-run `--update`.

`--verify-only` re-checks captures already under `.work/ui-harness/captures/`
without building or running the app. It fails if a capture file is missing or
corrupt, if a hash/signature does not match, or if an input file changed.

## Determinism notes

- Scene captures are deterministic: demo deck, bundled local media, fixed
  virtual time, and Mesa lavapipe.
- Database and script captures run with `ETEREA_HIDE_MEDIA=1` and an isolated
  `ETEREA_CONFIG_DIR` whose `settings.json` sets avatars and auto-media off.
  Posts reference remote URLs; without this, whether an avatar or image lands
  before the capture is a wall-clock race. With it, route/script images and trees
  are deterministic and no network is used. The trade-off: database captures
  show `load media` placeholders instead of remote images, while demo scenes do
  exercise the default auto-media image path with bundled assets.
- Route history is captured **empty** (`no actions yet`) from a fresh archive.
  A populated history renders each event's local wall-clock time
  (`17 Nov 18:03`), so it is **not golden**; it is manual evidence only:
  `.work/ui-harness/datasets/main` + `ETEREA_ROUTE=history` shows the import
  event from the fixture ingest.
- The host preferences of the user are never read: every capture gets isolated
  `ETEREA_CONFIG_DIR`/`XDG_CONFIG_HOME`/`XDG_DATA_HOME`/`XDG_CACHE_HOME`
  directories, and all `ETEREA_*` switches from the caller's environment are
  dropped before building each case's environment.
- Captures are still driver-dependent: font shaping, GPU rasterisation and the
  Mesa/wgpu versions can change pixels. Image goldens are meant for this
  checkout on a pinned environment; after a driver/Mesa/wgpu/Rune bump run
  `--update` and review the diff.

## Known findings and limits

- **Clipping warnings are currently zero.** The detail viewer is now a scroll
  viewport (56 pt top / 200 pt bottom padding) so media-heavy posts stay
  reachable at small heights. The media-heavy scenes expose a clipped
  `scroll-view` (`scene/hidden/960x640`: viewport `[0, 56, 960, 584]`,
  scroll range 228.4 pt; `scene/hidden/1280x820`: range 48.4 pt;
  `scene/video/960x640`: range 2.3 pt), so the overflow sites the harness
  flagged before (favourite/copy-link/open-on-x/edit-tags/delete row below the
  window) are now inside a scrolling ancestor with a `clips` state and are no
  longer reported. The old 960 px date-rail clipping is gone too: the 740 pt
  detail content keeps `detail`/`quote`/`video`/`hidden` rails inside the
  window. The full matrix run reports **0 clipping warnings**; `--strict` would
  fail on any new one. These captures show the top of the viewer, not the
  bottom after scrolling (the tree carries the scroll range and
  `scroll-up`/`scroll-down`/`set-scroll-offset` actions; scrolling itself is
  not scripted).
- **`ETEREA_HIDE_MEDIA` does not affect demo scenes.** Demo setup marks every
  post's media loaded and preloads bundled images, so `scene hidden` with
  `ETEREA_HIDE_MEDIA=1` still shows images; the hidden-media `load media`
  surface is captured instead by the database routes and the carousel dataset
  (which run with `ETEREA_HIDE_MEDIA=1`). The matrix still sets the env for the
  `hidden` scene as requested; report any behavior change here.
- **No free-form interaction coverage.** The scripts cover export, selection and
  carousel paging only. No import flow, favorites/read toggles, tag editing,
  delete confirmation, drag/drop, search typing, or video playback
  (`ETEREA_PLAY`) is captured; `ETEREA_IMPORT` is not exercised.
- **No network.** All archives are local; remote avatars/media are disabled for
  database captures. Media alt labels and carousel pages are exercised without
  fetching image bytes.
- **Accessibility tree + paint list only.** Checks read the accesskit tree and
  the element/paint list Rune emits; this is not a certified screen-reader
  audit, and effects that only exist in pixels are covered only by the image
  goldens.
- **Clipping is geometric, not pixel-level.** A node is "overflowing" when its
  accessibility bounds leave the viewport with no `clips` ancestor. Content
  clipped inside a scrolled container is expected and not flagged; partial
  occlusion by overlays is not detected.
- **Headless Vulkan required.** There is no GPU in CI; a software Vulkan driver
  is needed. On this host the wrapper uses Mesa lavapipe from the Nix store. A
  machine without a Vulkan adapter fails with Rune's GPU error and the case exit
  code, not a silent pass.
- **Cost.** Each capture is a full offscreen app launch (debug build, GPU init,
  virtual frames): roughly 17-18 s for a demo scene and 2-5 s for a
  database/script route; the full 98-case matrix took ~85-105 s on this host
  with the default 8 workers (plus the cargo build). The debug `eterea` binary
  is ~650 MB.
- **copy-lint scope.** It lints the aggregate of accessibility and paint text
  captured from the trees. It cannot see copy that is never rendered in the
  matrix, and its Rust-source mode is not used here.

## Failure-path demos

Clear, nonzero failures:

```text
$ bash scripts/ui-harness.sh --no-build --scene does-not-exist
harness error: unknown scene 'does-not-exist': matrix scenes are detail, selected, ...
EXIT: 2

$ rm .work/ui-harness/captures/script/export-sheet/1440x900/light.png
$ bash scripts/ui-harness.sh --no-build --verify-only --filter 'script/export-sheet/1440x900/light'
...
  FAIL script/export-sheet/1440x900/light: capture: no PNG was written
FAILED: 1 error(s)
EXIT: 1

$ printf '\n' >> verification/ui/scripts/carousel-page-2.json
$ bash scripts/ui-harness.sh --no-build --verify-only --filter 'script/carousel-page-2'
  FAIL inputs changed since the golden manifest was written (scripts/carousel-page-2.json: content changed); review and run --update
FAILED: 1 error(s)
EXIT: 1
```

A tampered capture (garbage bytes, wrong dimensions), a missing tree, a changed
matrix/fixture/script without `--update`, a missing `route-<Name>` element, or a
hash/signature drift all fail the same way with the case id and the
expected/actual values.

## CI note

A CI job needs: the pinned Rust toolchain, the workspace's cargo home/target
cache, a Mesa build with lavapipe (or a GPU), and `--no-build` after the build
job produced the debug binaries. Run `bash scripts/ui-harness.sh --update` only
to refresh goldens in a reviewed change, never in the pass/fail job.
