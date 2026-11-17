//! Miri runtime checks for eterea's pure crates.
//!
//! Miri is not a prover: it executes tests under an interpreter that detects
//! undefined behaviour, invalid pointer use and most aliasing violations. For
//! an `#![forbid(unsafe_code)]` crate the value is in (a) catching UB in the
//! dependency code the pure functions call (std string/IP parsing), and (b)
//! proving the exercised paths are deterministic and leak-free.
//!
//! Run with `scripts/verify/miri.sh` (pinned `nightly-2026-11-16` + miri).
//! Keep tests small: Miri is roughly two orders of magnitude slower than a
//! native `cargo test`, and this crate is meant to stay in the seconds range.

#![forbid(unsafe_code)]

/// Test-only library target so `cargo miri test` has a stable package to build.
pub fn smoke() -> bool {
    true
}
