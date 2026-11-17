# verification/

Verification assets for eterea. They are **not** part of the application build:
each harness crate has its own `[workspace]` and depends on the pure crates by
path, so `cargo build --workspace` at the repo root ignores everything here.

## Read in this order

| File | What it answers |
| --- | --- |
| [`SPEC.md`](SPEC.md) | What must be true: numbered invariants (IMP, QRY, MED, UI, GEO, RND) with sources and the proved/model-checked/runtime-validated/trusted vocabulary. |
| [`COVERAGE.md`](COVERAGE.md) | What is actually checked today: invariant → harness → status, plus the exact tool pins and the list of trusted components. **Start here for a status report.** |
| [`FINDINGS.md`](FINDINGS.md) | Bugs/gaps found while verifying, with reproducers, against the crate that owns them (`src/logic` is owned by the Lead). |
| [`kani/README.md`](kani/README.md) | Kani 0.68.0 harness crate: layout, bounded-input rationale, exact commands, extensibility. |
| [`verus/README.md`](verus/README.md) | Verus reference model and the migration plan: exactly what would move into Verus and what stays Kani/Miri/runtime. |
| [`miri/`](miri) | Standalone Miri test crate; reuses the Kani corpora for UB detection. |
| [`../fuzz/`](../fuzz) | cargo-fuzz targets for the URL/MIME policy and the JSON/CSV/archive importers; see [`../fuzz/README.md`](../fuzz/README.md). |
| [`../docs/verification/ui-harness.md`](../docs/verification/ui-harness.md) | Headless render/accessibility-tree matrix for the Rune shell. |

Tool scripts are in [`../scripts/verify/`](../scripts/verify/README.md)
(`versions.env`, `install.sh`, `verus.sh`, `kani.sh`, `miri.sh`, `all.sh`).

## One-command status

```bash
scripts/verify/all.sh --strict --fast   # proof/model-check/UB gate
scripts/verify/fuzz.sh                  # 10 s per fuzz target
scripts/verify/ci.sh                    # CI entry point with explicit budgets
bash scripts/ui-harness.sh              # 82-case headless UI matrix
```

CI: `.github/workflows/ci.yml` has a `verification` job that caches the pinned
`.tools/` toolchain, runs Verus, the 5-harness Kani fast gate and Miri with
explicit budgets, and uploads `verification-evidence-<run_id>`; a manual
`workflow_dispatch` adds the advisory full Kani run. The plain-Rust contract
tests also run in the `rust-workspace` job. Exact commands, budgets, observed
results and limits: [`../docs/verification/formal-verification.md`](../docs/verification/formal-verification.md).

## Scope statement

This work does **not** claim eterea is formally verified.

* The Verus layer proves lemmas about a **reference model** of the URL policy,
  not about `src/logic` as written.
* The Kani layer model-checks the **actual pure functions** over **bounded**
  inputs, plus concrete corpora.
* Miri and the plain harness tests **runtime-validate** exercised paths.
* `COVERAGE.md` lists what is merely **trusted**: SQLite, Rune, wgpu, ffmpeg
  (when integrated), DNS resolution, std's URL/IP parsing, font shaping and the
  OS clipboard.
* The Rune UI harness is **runtime validation**, not proof: it renders states,
  reads the accessibility tree and compares goldens. It does not drive
  interactions, network fetches, dialogs, video or audio.
* Fuzzing is a short smoke window over two parser surfaces, not a security proof.

The honest sentence to repeat: *"the pure crates have bounded model checking,
concrete regression tests and UB checks; the Verus proofs are of a reference
model; the UI is runtime-checked, not verified."*
