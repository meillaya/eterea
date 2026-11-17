//! Standalone Kani 0.68.0 harness crate for eterea.
//!
//! # Why this crate is not in the main workspace
//!
//! It has its own `[workspace]` in `Cargo.toml`, so root `cargo build`/
//! `cargo test` ignore it, and it only depends on `eterea-logic` and
//! `eterea-ui-model` by path. Model checking the Rune/desktop tree would be
//! slow and meaningless; the pure crates are the unit of verification.
//!
//! # Two audiences, one source of truth
//!
//! * **Plain Rust** (`cargo test`): the property helpers in [`url_policy`],
//!   [`mime`] and [`keymap`] are checked against the current pure functions
//!   over concrete corpora. This is what runs in CI today.
//! * **Kani** (`cargo kani`): the `#[kani::proof]` harnesses under `proofs`
//!   are gated with `#[cfg(kani)]`; `cargo kani` sets that cfg, plain cargo
//!   does not. `scripts/verify/kani.sh` drives it.
//!
//! The helpers and the corpora are shared by both layers so a harness cannot
//! drift from the tests that explain it. `verification/miri` reuses the same
//! corpora for its UB-detection run.
//!
//! # Adding a harness (extension point)
//!
//! 1. Add a property helper or corpus to [`url_policy`], [`mime`], [`keymap`]
//!    (or a new module) with a comment naming the `INV-*` id from
//!    `verification/SPEC.md`.
//! 2. Add a concrete check to `tests/properties.rs` so the property is
//!    exercised by plain `cargo test` on stable Rust.
//! 3. Add a bounded `#[kani::proof]` to `proofs/url.rs`, `proofs/mime.rs` or
//!    `proofs/keymap.rs`. Keep symbolic inputs small (bytes, not `String`) and
//!    use `kani::assume`.
//! 4. Run `scripts/verify/all.sh` and update `verification/COVERAGE.md`.
//!
//! When a harness proves an expected property that the current code violates,
//! put it behind the `known-gaps` feature and record the finding instead of
//! hiding it: see `proofs/known_gaps.rs` and `verification/FINDINGS.md`.
//!
//! This crate tracks `src/logic` and `src/ui-model` as they are. When those
//! crates change, re-run `scripts/verify/all.sh`; corpora that no longer match
//! fail on purpose so the contract change is reviewed, not discovered later.

#![deny(rustdoc::broken_intra_doc_links)]

pub mod keymap;
pub mod mime;
pub mod url_policy;

/// `#[kani::proof]` harnesses. Only compiled by `cargo kani`.
#[cfg(kani)]
mod proofs;
