# Kani harness crate

Standalone crate that model-checks eterea's pure functions with **Kani
0.68.0**. It has its own `[workspace]`, so the repository root
`cargo build/test --workspace` never builds it, and it depends only on
`eterea-logic` and `eterea-ui-model` by path.

```
verification/kani/
├── Cargo.toml          # own [workspace]; path dependencies; known-gaps feature
├── README.md           # this file
├── src/
│   ├── lib.rs          # crate docs and extension-point steps
│   ├── url_policy.rs   # INV-MED-01..04 corpora + contract helper
│   ├── mime.rs         # INV-MED-09 image MIME allowlist
│   ├── keymap.rs       # INV-UI-01..03 action_for table + contract helper
│   └── proofs/         # #[cfg(kani)] harnesses (only compiled by cargo kani)
│       ├── mod.rs
│       ├── url.rs
│       ├── mime.rs
│       ├── keymap.rs
│       └── known_gaps.rs   # opt-in; expected to FAIL until src/logic is fixed
└── tests/properties.rs # plain `cargo test` checks over the same corpora
```

## Commands

Everything is installed and driven by `scripts/verify/`; the direct commands
are listed too because they are what CI runs.

```bash
# 0. Install the pinned toolchain (network; ~180 MB; writes only under .tools/)
scripts/verify/install.sh kani

# 1. Baseline model checking (all harnesses except known gaps)
scripts/verify/kani.sh

# 2. Single harness (fast feedback while editing a proof)
scripts/verify/kani.sh --harness url_rejects_known_bad_literals

# 3. Known gaps against src/logic — expected to FAIL, see verification/FINDINGS.md
scripts/verify/kani.sh --gaps

# 4. Plain stable-Rust checks (no Kani needed; this is what normal CI runs)
cd verification/kani
CARGO_HOME=../../.tools/cargo-home CARGO_TARGET_DIR=../../.tools/target/harness cargo test

# 5. Miri UB-detection layer reuses these corpora
scripts/verify/miri.sh
```

`scripts/verify/kani.sh` exports `PATH=.tools/kani/bin:$PATH`,
`CARGO_HOME=.tools/cargo-home` and `CARGO_TARGET_DIR=.tools/target/kani`, so a
Kani run never writes to `$HOME`.

### Toolchain notes (discovered while pinning 0.68.0)

* The 0.68.0 release tarball ships `bin/kani-driver`, **not** `bin/cargo-kani`.
  `kani-driver` dispatches on its invocation name, so `install.sh` creates
  `.tools/kani/bin/cargo-kani -> kani-driver`. Without that symlink the cargo
  plugin cannot be invoked.
* The release bundle deliberately omits the Rust toolchain. The official
  interactive flow (`cargo install kani-verifier` + `cargo kani setup`) puts a
  nightly under `<bundle>/toolchain`; `install.sh` does the non-interactive
  equivalent: `rustup toolchain install nightly-2026-08-21 --profile minimal`
  under `.tools/rustup`, then symlinks `.tools/kani/toolchain` to it. The pin
  must match the bundle's `rust-toolchain-version` file and the install fails
  loudly if it does not.
* First `cargo kani` run compiles `eterea-logic`/`eterea-ui-model` with
  `kani-compiler`; later runs reuse the target dir under `.tools/target/kani`.

## What is proved, and how bounded

Kani checks the **actual Rust functions** in `eterea-logic` and
`eterea-ui-model`; it is not checking a model. Each harness states its bound:

| Harness | Invariant | Input | Result 2026-11-17 |
| --- | --- | --- | --- |
| `action_for_table_matches_spec` | INV-UI-01 | 15 concrete keys | verified, 1.1 s |
| `action_for_unmapped_keys_are_none` | INV-UI-01 | ~34 concrete keys | verified |
| `action_for_future_keys_are_not_global` | INV-UI-02 | `g`, `G` | verified |
| `action_for_single_byte_keys` | INV-UI-01 | symbolic 1 ASCII byte | verified |
| `url_rejects_empty_userinfo` | INV-MED-03 | `https://@` + symbolic 3-byte continuation | not completed: CBMC > 240 s |
| `url_rejects_fragments` | INV-MED-04 | `https://#` + symbolic 3-byte continuation | verified |
| `url_rejects_known_bad_literals` | INV-MED-01..04 | small `KANI_MUST_REJECT` subset | not completed: CBMC > 240 s |
| `url_accepts_known_good_literals` | INV-MED-01..04 | small `KANI_MUST_ACCEPT` subset | not completed: CBMC > 240 s |
| `url_accepts_only_https` | INV-MED-01 | symbolic 9-byte ASCII URL | not completed: CBMC > 240 s |
| `image_mime_accepts_allowlist` | INV-MED-09 | concrete allowlist corpus | not completed: CBMC > 240 s |
| `image_mime_rejects_other_types` | INV-MED-09 | concrete reject corpus | not completed: CBMC > 240 s |
| `url_rejects_recorded_gaps` (`--gaps`) | INV-MED-02 | `KNOWN_GAPS` (empty) | vacuous pass |

"Not completed" means the harness compiled and CBMC started but exceeded a
240 s budget on this machine; it is **not** a pass and not a failure. Those
properties are covered at runtime by `cargo test` and Miri and (for the URL
model) by Verus. CI should run the full suite with a 45+ min job or use
`scripts/verify/all.sh --fast`, which runs the verified set above.

Why the symbolic URL harnesses are scoped to 9–12 bytes: Kani verifies the real
`is_safe_https_url`, including `std::net::IpAddr::from_str`. Symbolically
executing the IP parser over a long symbolic host makes CBMC explode; the
bounded class still proves the property it claims (scheme, userinfo, fragment)
against every input of that length. Longer/real inputs are covered by the
concrete corpora in `cargo test` and Miri, not by Kani.

Why there are two URL corpora: CBMC cost grows with concrete corpus size, and
each URL executes std character iteration and IP parsing. `KANI_MUST_*` is the
representative subset used by proofs; `MUST_*` is the full regression corpus
used by `cargo test`/Miri. Neither is "the spec" — `verification/SPEC.md` is.

`known-gaps` proofs encode the spec, not current behaviour. **As of the
2026-11-17 freeze `KNOWN_GAPS` is empty** (finding F-1 was fixed; those literals
now live in `MUST_REJECT`), so `--gaps` is vacuous and passes by construction.
The feature stays so the next finding is a one-line list entry plus a recorded
entry in `verification/FINDINGS.md`.

## Adding a harness

1. Pick the `INV-*` id from `verification/SPEC.md` (add the invariant first if
   it is new).
2. Add a corpus/helper in `src/url_policy.rs`, `src/mime.rs`, `src/keymap.rs`
   or a new module, with the invariant id in a comment.
3. Add a concrete assertion to `tests/properties.rs`.
4. Add a `#[kani::proof]` to `src/proofs/` using bounded byte inputs.
5. Run `cargo test`, then `scripts/verify/kani.sh --harness <name>`, then
   `scripts/verify/all.sh`.
6. Update `verification/COVERAGE.md`.

## Limits

* Kani proves properties of a *bounded input class*; a passing run is not a
  proof for unbounded URLs. `verification/SPEC.md` and `COVERAGE.md` say which
  bound each harness uses.
* Kani cannot prove liveness, timing, rendering, SQLite behaviour or the
  Rune/wgpu stack; those are runtime-validated or trusted, see `COVERAGE.md`.
* A Kani pass on the pure crates is **not** a whole-app verification claim.
