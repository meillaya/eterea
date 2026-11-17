# scripts/verify — pinned verification toolchain

Every tool used by verification installs under **`<repo>/.tools/`** and is
driven from these scripts. Nothing is installed system-wide, `sudo` is never
used, and no global `~/.cargo` / `~/.rustup` state is read or written.

| Tool | Pin | Installed at | Status 2026-11-17 |
| --- | --- | --- | --- |
| Verus | `0.2026.11.02.8bf68ee` | `.tools/verus/` | installed, `verus --version` OK |
| Verus toolchain | `1.99.1` (rustup) | `.tools/rustup/toolchains/` | installed |
| Kani | `0.68.0` | `.tools/kani/` | installed; `cargo-kani --version` OK |
| Kani toolchain | `nightly-2026-08-21` (rustup) | `.tools/rustup/toolchains/` | installed, linked as `.tools/kani/toolchain` |
| Miri | `nightly-2026-11-16` + miri component | `.tools/rustup/toolchains/` | installed, `miri --version` OK |
| rustup | `1.29.3` | `.tools/cargo-home-rustup/bin/rustup` | installed |
| cargo-fuzz | `0.13.2` | `.tools/cargo-fuzz/bin/cargo-fuzz` | installed |

Exact pins, URLs and sha256 digests are in [`versions.env`](versions.env); the
last install result is in `.tools/logs/install-status.txt` (workspace-local, not
committed).

## Files

| File | Purpose |
| --- | --- |
| `versions.env` | single source of pins: versions, URLs, sha256, timeouts |
| `install.sh` | installs/reinstalls tools into `.tools/`; per-step logs under `.tools/logs/` |
| `verus.sh` | checks `verification/verus/*.rs` with pinned Verus |
| `kani.sh` | `cargo kani` on `verification/kani`; `--gaps` adds known-gap proofs |
| `miri.sh` | `cargo miri test` on `verification/miri` |
| `fuzz.sh` | cargo-fuzz run for `url_policy` and `import_text`; 10 s each by default, seeds in `fuzz/corpus/` |
| `all.sh` | runs every installed layer, writes `.tools/logs/all-status.txt`; `--strict` turns SKIP into failure; `--fast` limits Kani to the quickly-verified harness set |
| `ci.sh` | CI gate: runs the pinned layers with explicit per-layer `timeout` budgets and writes `.tools/logs/ci-status.txt` + `ci-*.log`; `--full` adds the advisory (non-gating) expensive Kani run |

## Usage

```bash
# One-time install. Network is needed (~450 MB total); each artifact has a
# download timeout (default 300 s, override with DOWNLOAD_TIMEOUT=120 ...).
scripts/verify/install.sh
scripts/verify/install.sh verus kani      # subset
scripts/verify/install.sh --force miri    # redownload/reinstall

# Run the layers
scripts/verify/all.sh                     # SKIP layers whose tool is absent
scripts/verify/all.sh --strict            # missing tool = failure (for CI)
scripts/verify/all.sh --gaps              # include known-gap Kani proofs
scripts/verify/all.sh --skip-kani --skip-miri --skip-verus
scripts/verify/kani.sh --harness url_accepts_only_https
scripts/verify/verus.sh verification/verus/url_policy.rs
scripts/verify/miri.sh
FUZZ_TIME=10 scripts/verify/fuzz.sh

# Plain-stable harness crate tests (no tool install needed)
cd verification/kani && cargo test
```

`all.sh` writes only these workspace-local files:
`.tools/logs/{harness,kani,miri,verus}.log`, `.tools/logs/all-status.txt`,
and the Kani/Miri cargo caches under `.tools/target/`.

## CI wiring (`.github/workflows/ci.yml`, 2026-11-17)

Two places call this directory from CI:

1. **`rust-workspace` job (every PR/push, no prover binaries).** Two extra steps
   after `cargo test --workspace` run the standalone crates' contract tests on
   the devenv toolchain, each wrapped in a 120 s `timeout`:
   ```bash
   timeout 120 bash -c 'cd verification/kani && cargo test'
   timeout 120 bash -c 'cd verification/miri && cargo test'
   ```
   Observed: `10 passed; 1 ignored` (kani) and `5 passed` (miri); the ignored
   test is the known-gap mechanism and is documented in FINDINGS.md.

2. **`verification` job (every PR/push; `workflow_dispatch` adds the advisory
   full Kani run).** Steps:
   - `actions/cache@v5` restores `.tools/{downloads,verus,kani,rustup,cargo-home,cargo-home-rustup}`
     with key `verification-tools-<os>-<hash of scripts/verify/versions.env>`;
     the key changes when any pin changes. First run is a cache miss (~450 MB
     of sha256-verified downloads + rustup toolchains); later runs restore the
     workspace-local toolchain.
   - `timeout 1200 bash scripts/verify/install.sh verus kani miri fuzz` installs
     or re-verifies the pins, including cargo-fuzz; it never installs
     system-wide and never needs sudo.
   - `timeout 2400 bash scripts/verify/ci.sh` (with `--full` on
     `workflow_dispatch`) runs the layers with explicit budgets:
     | layer | command | CI budget | observed reference host |
     | --- | --- | --- | --- |
     | verus | `scripts/verify/verus.sh` | 300 s | < 5 s, 10 verified |
     | kani (fast) | `scripts/verify/kani.sh` + 5 harness names | 900 s | ~30 s warm, 5 verified |
     | miri | `scripts/verify/miri.sh` | 900 s | ~20 s warm, 5 passed |
     | fuzz | `scripts/verify/fuzz.sh` | 420 s | 5 s/target in CI; no crash |
     | kani-full | `scripts/verify/kani.sh` (all) | 900 s | **ADVISORY only**, may time out |
   - If a pin in `versions.env` changes, the key changes; a stale toolchain
     restored by the `restore-keys` prefix fails loudly at the script's version
     check (it never silently uses the wrong Verus/Kani/Miri). Clear the cache
     or run `scripts/verify/install.sh --force <tool>` after a pin bump.
   - Evidence: `.tools/logs/ci-status.txt` + `ci-*.log` are copied to
     `verification-evidence/`, uploaded as the `verification-evidence-<run_id>`
     artifact (14-day retention), and the layer table is appended to the job's
     `$GITHUB_STEP_SUMMARY`.
   - The job hard stop is 80 min (20 min install + 55 min of layer budgets as the
     worst case); the fast gate is never allowed to hang.
   `draft-release.yml` intentionally does not run verification: it builds and
   publishes a binary artifact, and slower prover verification belongs in the
   CI gate, not the release workflow.

3. **Do not** add `verification/kani` or `verification/miri` to the root
   workspace members: their `[workspace]` tables are deliberate, and
   `.tools/` is already in the root `.gitignore`.

### Reference-host observations and explicit limits

* The Kani **fast gate** is the only Kani result CI asserts: 5 harnesses
  verified locally (`action_for_*` ×4 including the symbolic 1-byte key proof,
  plus the symbolic URL fragment rule). The other **7 URL/MIME harnesses exceed
  a 240 s CBMC budget** on the reference host (std string/char/IP/case-folding
  code is symbolically executed); they are run only by `--full` and recorded as
  ADVISORY, never as verified. Their input corpora are covered by
  `cargo test` and Miri.
* **Miri CI scope is `verification/miri` only** (the pure crates). The desktop
  crate depends on Rune/wgpu/ffmpeg and is deliberately out of Miri scope on
  this host and in CI.
* **Verus proves a reference model** in `verification/verus/url_policy.rs`, not
  `src/logic` as written. Do not describe a green Verus layer as an app proof.
* The workflow YAML is checked locally with
  `python3 scripts/validate-github-workflows.py` (PASS) and
  `actionlint .github/workflows/*.yml` (PASS); neither is part of CI today.

## Notes / troubleshooting

* **Verus is rustup-managed since 2026**: the `verus` wrapper needs `rustup` on
  PATH and toolchain `1.99.1` installed. `install.sh` installs both under
  `.tools`; `verus.sh` exports `RUSTUP_HOME`, `CARGO_HOME`,
  `RUSTUP_TOOLCHAIN` and PATH for you.
* **Kani 0.68.0 tarball does not ship `cargo-kani`**: `install.sh` symlinks
  `.tools/kani/bin/cargo-kani -> kani-driver` (the driver dispatches on argv[0]).
  It also installs the toolchain the bundle expects (`nightly-2026-08-21`) and
  symlinks `.tools/kani/toolchain` to it, which is what `cargo kani setup`
  would otherwise do interactively.
* **Kani verification is slow on concrete corpora**: CBMC cost scales with
  the number of concrete strings × std library work. Baseline proofs use small
  representative corpora; the full corpora are checked by `cargo test` and
  Miri. Use `--harness` for fast feedback and raise the job timeout for the
  full suite.
* **Network is slow/unavailable**: each step has `DOWNLOAD_TIMEOUT` (default
  300 s) and `BUILD_TIMEOUT` (default 300 s); a failed step records the exact
  failing command in `.tools/logs/install-<tool>.log` and the script continues
  with the others. Rerun later; verified downloads are cached in
  `.tools/downloads` by sha256.
* **Miri needs the host target only**; the pinned nightly is minimal + `miri` +
  `rust-src` (cargo-miri builds its sysroot from source).
