# Coverage

Status of the verification layers on 2026-11-17. Open findings: none. F-1 (URL
policy) and F-5 (archive-JS parser panic) are fixed and locked by regression
tests; see [`FINDINGS.md`](FINDINGS.md). The v2 reader-state/history/export
surface is covered by the data-v2 service and store tests; the headless UI
harness was refreshed for the new routes, selection, history, export and
carousel, keys sheet, search, mouse back and directory filter, and now covers
89 deterministic cases. Findings from the 2026-11-17 critical audit are tracked
in [`../../docs/audit/2026-11-17-critical-audit.md`](../../docs/audit/2026-11-17-critical-audit.md).

> **No whole-app verification claim.** The Verus layer proves a reference model
> of the URL policy, not `src/logic` as written. Kani checks bounded pure
> functions. Miri checks `verification/miri`. Fuzzing observes two parser
> surfaces for a short CI window. The Rune desktop UI is runtime-checked by a
> headless capture/tree harness, not proved. SQLite, Rune/wgpu, ffmpeg,
> DNS/TLS/HTTP, font shaping and the OS clipboard are trusted.

## Tool status (workspace-local under `.tools/`)

| Tool / pin | Check | Observed |
| --- | --- | --- |
| Verus `0.2026.11.02.8bf68ee` | `scripts/verify/verus.sh` | `verification results:: 10 verified, 0 errors` |
| Kani `0.68.0` | `scripts/verify/kani.sh` fast gate | 5 harnesses successfully verified, 0 failures; 7 expensive URL/MIME harnesses exceed a 240 s CBMC budget and are advisory only |
| Miri `nightly-2026-11-16` | `scripts/verify/miri.sh` | 5 passed, 0 failed, no UB reported |
| cargo-fuzz `0.13.2` | `scripts/verify/fuzz.sh` | 2 targets: `url_policy`, `import_text`; local 10 s run per target, CI 5 s |
| plain stable Rust | `cd verification/kani && cargo test` | 10 passed, 1 ignored |
| Rune headless harness | `bash scripts/ui-harness.sh` | 89/89 captures pass image and tree goldens, copy lint 0/0, no clipping warnings |

Pins, URLs and sha256 are in [`../scripts/verify/versions.env`](../scripts/verify/versions.env).

## What runs where

| Layer | Command | Scope |
| --- | --- | --- |
| Verus | `scripts/verify/verus.sh` | URL-policy reference model, 10 lemmas |
| Kani fast gate | `scripts/verify/kani.sh --harness ...` | 5 bounded harnesses: keymap and URL fragment rule |
| Kani advisory | `scripts/verify/ci.sh --full` on workflow_dispatch | 7 expensive URL/MIME harnesses, never a pass gate |
| Miri | `scripts/verify/miri.sh` | standalone pure-crate tests |
| Fuzz | `scripts/verify/fuzz.sh` | URL/MIME policy and JSON/CSV/archive import parsers |
| UI harness | `bash scripts/ui-harness.sh` | 89 deterministic captures: 54 scenes, 16 main routes, 4 dataset routes, 14 script captures (keys sheet, search, mouse back, directory filter, export, selection, carousel) |
| Workspace tests | `cargo test --workspace` | storage, store actor, parsers, media workers, desktop model |
| Data v2 tests | `cargo test -p eterea-core -p eterea-app -p eterea-logic` | schema v2, batch import, tags/read/bulk, history/undo, export round-trips, 10k benchmarks |
| Media v2 tests | `cargo test -p eterea-media` | image worker/cache, video cache, tools discovery, audio lifecycle, real local ffmpeg fixture |

CI: `.github/workflows/ci.yml` runs the workspace tests, the standalone crate
tests, the Verus/Kani/Miri gate and a 5 s-per-target fuzz smoke step in the
`verification` job. The UI harness needs software Vulkan and is a local gate.

## Coverage by area

### Import and storage (`INV-IMP-*`, `INV-QRY-*`)

| Area | Evidence | Status |
| --- | --- | --- |
| Malformed archive JS returns an error, never a panic (INV-IMP-01) | Fuzz `import_text`; regression test `malformed_archive_wrapper_is_an_error_not_a_panic` | runtime + fuzz, F-5 fixed |
| CSV/JSON/X archive parsing and dedup | `cargo test -p eterea-core -p eterea-app`; service guardrail tests | runtime-validated |
| Preview never writes | store/service tests, import routes exercised by the UI matrix | runtime-validated |
| Quote honesty and media metadata | backend tests; quote unavailable path in UI | runtime-validated |
| Storage, migrations and SQL | SQLite tests plus migration tests | runtime-validated; SQLite trusted |

### Reader state, history, export and carousel (`INV-DATA-*`, `INV-REL-*`)

| Area | Evidence | Status |
| --- | --- | --- |
| Schema v2 migration and data preservation | `tests/migrations_tests`, `tests/data_v2` | runtime-validated; SQLite trusted |
| Read state, tags, bulk selection and FTS freshness | data-v2 service tests; store-actor tests | runtime-validated |
| History retention, exact inverse, newest-only undo, conflict reporting | data-v2 history tests | runtime-validated |
| CSV/JSON export scope, escaping and round-trip | data-v2 export tests plus release benchmarks | runtime-validated |
| 10k import/export budget | `cargo test -p eterea-core --release --test v2_benchmarks -- --ignored` | measured: 10k parse+write 56 ms; CSV/JSON export ~81 ms |
| Video cache/audio/decoder absence | `cargo test -p eterea-media`; no network fixture | runtime-validated; ffmpeg trusted |
| URL entity/link suppression, quote honesty | importer/link tests + UI captures | runtime-validated |
| Release runtime bundle | `scripts/smoke-linux-bundle.sh` | pending final green bundle |

### URL, MIME and media (`INV-MED-*`)

| Area | Evidence | Status |
| --- | --- | --- |
| URL policy literals | Kani corpora, Verus model, fuzz `url_policy` | model-checked + fuzz |
| URL policy helper properties | symbolic Kani bounded 9–12 bytes | bounded model-checked |
| Image MIME allowlist | Kani `image_mime_accepts_allowlist` / `image_mime_rejects_other_types` | model-checked |
| HTTPS fetch, byte cap, cache, cancellation, image MIME, decoder failure | `cargo test -p eterea-media` (unit + public-API tests; no live network) | runtime-validated |
| Embed payload parsing for opt-in `repair media` | `cargo test -p eterea-media --test syndication_payload` on captured payload fixtures; mapping/enrichment covered in `eterea-app` tests | runtime-validated; the live endpoint is not part of CI |
| Redirect/DNS residual risk | documented in media crate and `SPEC.md` | trusted/limit |

### UI model and keyboard (`INV-UI-*`)

| Area | Evidence | Status |
| --- | --- | --- |
| Single-key map | Kani `action_for_*` harnesses, Miri, cargo test | model-checked |
| Sequence/mode layer | `g`/`G` are unmapped; no sequence layer is implemented | not implemented |
| Desktop key handling | shell code and manual checks; UI harness does not drive input | runtime/manual only |
| Key sheet drift | hand-maintained array in the shell; no generated table | known maintenance risk |
| Settings persistence | `settings.json` read/write exercised manually | runtime-validated |

### Geometry and rendering (`INV-GEO-*`, `INV-RND-*`)

The headless UI matrix ([`ui-harness.md`](../docs/verification/ui-harness.md))
captures every route and scene at 1440x900, 1280x820 and 960x640, light and dark.
It checks:

* the binary exits 0 and writes a valid PNG of the expected size;
* the accessibility tree has a window-sized root, the expected route/scene text,
  and no negative or unclipped out-of-window nodes;
* copy lint on all captured strings;
* image hash and canonical tree signature against committed goldens.

Final result on the reference host: **89/89 cases pass, 89/89 image and tree
goldens, copy lint 0/0, 0 clipping warnings**. This is runtime validation of
render states and geometry bounds, not proof, pixel-level QA or exhaustive
interaction testing. Motion, contrast ratios, hover, focus traversal, clipboard,
dialogs and video/audio behaviour are not fully covered by the matrix.

### Trusted components

| Component | Why trusted | Consequence |
| --- | --- | --- |
| SQLite (`rusqlite`, bundled) | storage engine | schema/data invariants depend on SQLite |
| Rune / wgpu / Vulkan driver | UI runtime; lavapipe in headless | layout, glyphs and pixels are trusted; the matrix checks bounds/text only |
| ffmpeg | video decode, local file only | decode correctness and audio are trusted/not wired |
| DNS/TLS/HTTP (`ureq`, rustls) | network stack | literal-host policy is checked; DNS rebinding/redirects remain residual |
| Font shaping/fallback (parley, system fonts) | text stack | CJK/Latin rendering trusted |
| OS clipboard and window manager | platform | copy/open and window management trusted |
| Rust std IP/URL parsing and allocator | standard library | policy rests on these parsers |

## Fuzz

Targets are in [`../fuzz`](../fuzz):

* `url_policy` feeds arbitrary UTF-8 to `is_safe_https_url` and
  `is_image_mime`; accepted URLs must be HTTPS and fragment-free.
* `import_text` feeds arbitrary UTF-8 to the JSON, archive-JS and CSV parser
  entry points; malformed input must return an error, not panic.

Committed seeds live in `fuzz/corpus/`. The CI step runs 5 s per target; local
10 s runs on 2026-11-17 produced:

```text
url_policy  1,013,942 runs in 6 s   PASS
import_text     46,582 runs in 6 s   PASS  (after F-5 fix)
```

F-5 was found by this target: `window.YTD.tweets.part0 = K];[\n` made
`extract_payload` slice with `start > end` and panic. Fixed with an
`InvalidFormat` guard and a regression test; the crash input was removed after
the fix.

## Limits

1. Not whole-app: no proof/model-check of SQLite, Rune rendering, wgpu, ffmpeg,
   DNS/TLS/HTTP, fonts, clipboard or the desktop interaction state machine.
2. Kani is bounded; the seven expensive URL/MIME harnesses are advisory.
3. Miri covers `verification/miri` only; the desktop crate is excluded.
4. Fuzzing is a short smoke window, not a proof of parser security.
5. The UI matrix verifies render states and geometry bounds, not interaction
   behaviour or pixel contrast.
6. No GitHub-hosted CI run has been observed yet; the numbers are from the
   reference host.

## How to update

1. Add or change the invariant in `SPEC.md`.
2. Add the harness and a seed corpus if fuzzing.
3. Run the exact layer command and record the observed result here.
4. Never call a bounded/runtime check "proved"; name the bound and scope.
5. CI evidence must come from a real artifact or the exact local command.
