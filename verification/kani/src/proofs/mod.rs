//! `#[kani::proof]` harnesses. This module is only compiled by `cargo kani`
//! (which sets `--cfg kani`); plain `cargo build`/`cargo test` never sees it.
//!
//! Rules for new harnesses:
//! * symbolic inputs are byte arrays, never `String`/`str::from_utf8` (UTF-8
//!   validation would dominate the verification cost and prove nothing about
//!   the policy under test);
//! * every `kani::assume` has a comment saying which input class it excludes;
//! * keep `#[kani::unwind(N)]` just above the largest concrete loop/recursion
//!   bound; a harness that times out is worse than a smaller bounded one.
//! * a harness proving a *specified* property the code currently violates goes
//!   in `known_gaps` behind the `known-gaps` feature, not in the baseline.

mod keymap;
mod mime;
mod url;

#[cfg(feature = "known-gaps")]
mod known_gaps;

/// Upper bound on the symbolic URL length used by the baseline URL harnesses.
///
/// Nine bytes is the shortest class that can still contain a complete
/// `https://` scheme plus one more byte: with a longer budget Kani would spend
/// its time in `std::net::IpAddr::from_str` for symbolic host strings, while
/// the property proved here (accepted implies an HTTPS scheme) is about the
/// scheme split. Longer URL properties are covered by the concrete corpora,
/// which cost minutes rather than hours under CBMC.
const URL_LEN: usize = 9;

/// Build a `&str` from a bounded symbolic byte array.
///
/// Callers must `kani::assume` every byte is ASCII (`b < 0x80`) first, which
/// makes the bytes valid UTF-8. Using `from_utf8_unchecked` keeps Kani's
/// reasoning on the policy rather than on std's UTF-8 validator.
fn ascii_str(bytes: &[u8]) -> &str {
    debug_assert!(bytes.iter().all(|b| *b < 0x80));
    // SAFETY: ASCII is valid UTF-8; callers assume `b < 0x80` for every byte.
    unsafe { core::str::from_utf8_unchecked(bytes) }
}
