# Formal verification in CI

Date: 2026-11-17. Status: **wired and locally green**, with explicit limits.
Owner: verification. This page is the evidence contract for the `verification`
job: it states exactly what CI runs, which commands produced the quoted results,
and what is deliberately *not* verified.

> **No whole-app verification claim.** CI proves a Verus **reference model** of
> the URL policy, model-checks the real pure functions with **bounded** Kani
> inputs, fuzzes two parser surfaces for a short window, and runs **Miri** on
> `verification/miri` only. The Rune desktop UI is runtime-checked by a separate
> headless capture/tree matrix, not proved. SQLite storage, DNS/TLS, fonts and
> the OS clipboard remain trusted; see `verification/COVERAGE.md`.

## Pinned toolchain

All tools install workspace-locally into `.tools/`, sha256-verified, with no
sudo and no global rustup/cargo state. Pins live in
[`scripts/verify/versions.env`](../../scripts/verify/versions.env).

| Tool | Pin | Installed at |
| --- | --- | --- |
| Verus | `0.2026.11.02.8bf68ee` (+ rustup toolchain `1.99.1`) | `.tools/verus/`, `.tools/rustup/` |
| Kani | `0.68.0` (+ bundle toolchain `nightly-2026-08-21`) | `.tools/kani/`, linked `.tools/kani/toolchain` |
| Miri | `nightly-2026-11-16` + `miri`/`rust-src` components | `.tools/rustup/toolchains/` |
| cargo-fuzz | `0.13.2` (uses the pinned Miri nightly) | `.tools/cargo-fuzz/bin/cargo-fuzz` |
| rustup bootstrap | `1.29.3` | `.tools/cargo-home-rustup/bin/rustup` |

Local install evidence (`.tools/logs/install-status.txt`, command
`bash scripts/verify/install.sh verus kani miri fuzz`):

```text
verus  OK        verus 0.2026.11.02.8bf68ee
kani   OK        Kani Rust Verifier 0.68.0 (cargo plugin) (toolchain nightly-2026-08-21-x86_64-unknown-linux-gnu)
miri   OK        miri rustc 1.102.0-nightly (5f4effd57 2026-11-13) (nightly-2026-11-16)
```

Kani 0.68.0 quirks are handled by the install script: the release tarball ships
`bin/kani-driver` but not `bin/cargo-kani` (symlinked as the cargo-plugin entry
point), and the bundle deliberately omits the Rust toolchain (installed via
rustup and symlinked as `.tools/kani/toolchain`, the non-interactive equivalent
of `cargo kani setup`). Verus 2026 releases are rustup-managed; the script
installs rustup and the required toolchain under `.tools/`.

## What CI runs

### 1. `rust-workspace` job — plain-Rust contract tests (every PR/push)

After `cargo test --workspace`, two steps run the standalone verification crates
on the devenv toolchain, each with a hard 120 s timeout so a hang fails fast
instead of stalling CI:

```bash
timeout 120 bash -c 'cd verification/kani && cargo test'
timeout 120 bash -c 'cd verification/miri && cargo test'
```

Observed on the reference host: `10 passed; 1 ignored` (kani crate; the ignored
test is the documented known-gap mechanism) and `5 passed` (miri crate), each in
under a second. These steps need no prover binaries, so they work on forks and
without network downloads.

The `verification` job also installs cargo-fuzz and runs a 5 s-per-target fuzz
smoke step:

```bash
FUZZ_TIME=5 timeout 420 bash scripts/verify/fuzz.sh
```

Targets are `url_policy` (URL/MIME policy) and `import_text` (JSON, archive JS
and CSV parser entry points). The fuzz gate found F-5, a panic on malformed
archive JS; the fix and regression test are in `src/backend` and
`verification/FINDINGS.md`. Fuzz crashes are copied to `fuzz/artifacts/` and fail
the step.

### 2. `verification` job — pinned provers (every PR/push; full mode on dispatch)

Steps, in order:

1. **Restore pinned verification tools** — `actions/cache@v5` for
   `.tools/{downloads,verus,kani,rustup,cargo-home,cargo-home-rustup,cargo-fuzz}` with key
   `verification-tools-<runner.os>-<hash(scripts/verify/versions.env)>`
   (restore-key prefix fallback). The key changes when any pin changes.
2. **Install pinned tools** — `timeout 1200 bash scripts/verify/install.sh verus kani miri fuzz`
   (hard step timeout 20 min). Cache hits only re-verify versions.
3. **Run layers with explicit budgets** — `timeout 2400 bash scripts/verify/ci.sh`
   (`--full` only on `workflow_dispatch`), followed by
   `FUZZ_TIME=5 timeout 420 bash scripts/verify/fuzz.sh`. Layer budgets are
   asserted and printed by the scripts; the job hard stop is 80 min.
4. **Assemble evidence** (`if: always()`) — copies `.tools/logs/ci-status.txt`,
   `ci-*.log` and `ci-fuzz.log` to `verification-evidence/`, appends a layer
   table to the job summary, and uploads the `verification-evidence-<run_id>`
   artifact (14-day retention).

| Layer | Command | CI budget | Reference-host observation |
| --- | --- | --- | --- |
| verus | `scripts/verify/verus.sh` | 300 s | PASS, 1 s, `10 verified, 0 errors` |
| kani (fast gate) | `scripts/verify/kani.sh` + 5 named harnesses | 900 s | PASS, 29 s, `5 successfully verified harnesses, 0 failures` |
| miri | `scripts/verify/miri.sh` | 900 s | PASS, 13–22 s, `5 passed; 0 failed` |
| fuzz | `scripts/verify/fuzz.sh` | 420 s | PASS, 5 s/target in CI; URL and import targets; no crash |
| kani-full (advisory) | `scripts/verify/kani.sh` (all harnesses) | 900 s | ADVISORY only; may exceed CBMC budgets |

`draft-release.yml` is intentionally unchanged: it builds and publishes the
desktop binary artifact, and prover verification belongs to the CI gate rather
than the release job.

The headless UI harness is not a CI job because GitHub-hosted runners have no
Vulkan device. It is a local gate:

```bash
bash scripts/ui-harness.sh
# 89/89 cases, image 89/89, tree 89/89, copy lint 0/0, no clipping warnings
```

### Kani budgets and limits (explicit)

* The **fast gate** is the only Kani result CI asserts: 5 harnesses verified on
  the reference host — `action_for_table_matches_spec`,
  `action_for_unmapped_keys_are_none`,
  `action_for_future_keys_are_not_global`,
  `action_for_single_byte_keys` (symbolic 1-byte key),
  `url_rejects_fragments` (symbolic 12-byte bounded URL).
* The other **7 URL/MIME harnesses are not gate material**: they compile and
  start but exceed a 240 s CBMC budget each on the reference host because they
  symbolically execute std string/char/IP/case-folding code. They run only under
  `ci.sh --full` (manual `workflow_dispatch`) and are recorded as ADVISORY,
  never as verified. Their concrete corpora are runtime-covered by `cargo test`
  and Miri.
* Kani proves properties of a **bounded input class** (9–12 byte symbolic URL
  prefixes; one symbolic key byte). It does not prove unbounded behaviour.

### Miri scope (explicit)

* CI runs `cargo miri test` on **`verification/miri` only**. That crate depends
  on `eterea-logic` and `eterea-ui-model` by path and on the Kani crate's shared
  corpora; it exercises the URL/keymap/MIME code on host-mounted inputs.
* The **desktop crate is out of scope**: it depends on Rune/wgpu/ffmpeg, needs a
  GPU/window and cannot run under Miri in CI. This is deliberate, not a gap in
  the harness.
* Miri is undefined-behaviour detection, not a prover: a pass means "no UB was
  observed on the exercised paths".

### Verus scope (explicit)

* CI runs `verus --crate-type=lib verification/verus/url_policy.rs` through
  `scripts/verify/verus.sh`: `10 verified, 0 errors` on the reference host.
* Those 10 lemmas are about the **reference model** in that file (scheme,
  authority, host, userinfo, loopback-literal rejection plus the executable
  scheme check), **not** about `src/logic/src/lib.rs` as written. The migration
  plan — what would move into Verus and what stays Kani/Miri/runtime — is
  [`verification/verus/README.md`](../../verification/verus/README.md).

## Reference-host evidence (2026-11-17)

Commands were run from the repository root on the development host
(`x86_64 Linux`, Rust `1.99.1`, tools installed under `.tools/`). Exact output
is quoted; full logs are in `.tools/logs/` locally and in the CI
`verification-evidence-<run_id>` artifact in CI.

### A. `bash scripts/verify/ci.sh` (the CI gate) — PASS

```text
eterea formal verification gate
  budgets: verus=300s kani-fast=900s miri=900s kani-full=900s (advisory)

verus      PASS      budget=300s  elapsed=1s
kani       PASS      budget=900s  elapsed=29s
miri       PASS      budget=900s  elapsed=22s

  kani tail: Complete - 5 successfully verified harnesses, 0 failures, 5 total.
  miri tail: test result: ok. 5 passed; 0 failed; 0 ignored; 0 filtered out
formal verification gate: PASSED (mandatory layers)
exit code: 0
```

### B. `bash scripts/verify/all.sh --strict --fast` — PASS

```text
harness   PASS  cargo test verification/kani
kani      PASS  cargo kani (baseline)
miri      PASS  cargo miri test verification/miri
verus     PASS  verus verification/verus
exit code: 0
```

### C. `cd verification/kani && cargo test` — PASS

```text
running 11 tests
test result: ok. 10 passed; 0 failed; 1 ignored; 0 filtered out
```

The ignored test is the known-gap mechanism; `KNOWN_GAPS` is empty (finding F-1
fixed — `verification/FINDINGS.md`).

### D. Workflow validation — PASS

```text
$ python3 scripts/validate-github-workflows.py
GitHub workflow validation: PASS
validated=ci.yml name=CI
validated=draft-release.yml name=Draft Release

$ python3 scripts/test_validate_github_workflows.py
PASS: real workflows pass
… (25 fixture checks) …
exit code: 0

$ actionlint .github/workflows/ci.yml .github/workflows/draft-release.yml
(no output; exit code 0)
```

`actionlint` was downloaded to `.tools/tmp/actionlint` for this validation only;
it is not a CI dependency.

## Expected CI cost (reference estimate)

| Scenario | Tool restore | Layers | Job estimate |
| --- | --- | --- | --- |
| Cache hit | ~1–2 min artifact download/restore | ~2–4 min (Kani target dir is not cached, so kani-compiler recompiles the path deps) | **~5–8 min** |
| Cache miss | ~450 MB downloads + rustup toolchains, ~5–8 min | ~2–4 min | ~15 min |

Hard cap: `timeout-minutes: 80` on the job, 20 min on install, and per-layer
timeouts in `scripts/verify/ci.sh` (300+900+900 s mandatory = 35 min worst
case; the dispatch path adds a 900 s advisory layer). The fast gate cannot hang
CI.

## Reproduction

```bash
# Local, exactly what CI does for the mandatory layers:
bash scripts/verify/install.sh verus kani miri fuzz
bash scripts/verify/ci.sh
# Manual "full" run (adds the advisory expensive Kani harnesses):
bash scripts/verify/ci.sh --full
# Plain-Rust contract tests (no prover binaries):
(cd verification/kani && cargo test)
(cd verification/miri && cargo test)
# Workflow static validation:
python3 scripts/validate-github-workflows.py
python3 scripts/test_validate_github_workflows.py
```

## Remaining limits (repeat once more)

1. **Not whole-app**: no proof/model-check of SQLite storage, Rune rendering,
   wgpu, ffmpeg, DNS/TLS/HTTP, font shaping, clipboard or the desktop UI.
2. **Bounded Kani**: 5 harnesses gated; 7 expensive URL/MIME harnesses are
   advisory only and exceed a 240 s CBMC budget on the reference host.
3. **Miri scope**: `verification/miri` only; desktop/Rune/GPU/ffmpeg excluded.
4. **Verus is a reference model**, not `src/logic` as written; the migration plan
   is in `verification/verus/README.md`.
5. **Fuzz is a smoke window**, not a security proof. It covers two parser
   surfaces for a few seconds per CI run.
6. **UI harness is runtime validation**, not proof: 89/89 render/tree/golden
   cases pass, but it does not drive interactions, network, dialogs, video or
   audio. It needs software Vulkan, so it is a local gate.
7. **No CI evidence from GitHub has been observed yet** — the numbers above are
   the local reference-host run of the exact CI commands. The first real CI run
   should be compared with this page; drift is a review item.
