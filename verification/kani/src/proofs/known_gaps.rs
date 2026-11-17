//! Opt-in Kani proofs for invariants the current URL policy violates.
//!
//! These encode the URL policy as specified by docs/design/SPEC.md §7 and
//! `verification/SPEC.md` (INV-MED-02). They are behind the `known-gaps`
//! feature so the baseline `cargo kani` run stays green; run them with:
//!
//! ```text
//! scripts/verify/kani.sh --gaps
//! ```
//!
//! A failing harness here is a *finding against src/logic*, not a harness bug.
//! The accepted values and the fix shape are recorded in
//! `verification/FINDINGS.md`. When the Lead tightens `is_safe_https_url`,
//! delete the matching proof and move its literal class into
//! `url_policy::MUST_REJECT`.
//!
//! **Status 2026-11-17: `KNOWN_GAPS` is empty.** Finding F-1 (legacy numeric
//! IPv4 spellings such as `127.1`, `2130706433`, `0x7f.0.0.1`) was fixed in
//! `src/logic`, and those literals now live in `MUST_REJECT`. The harness below
//! is therefore vacuous today; keep it wired so a future gap is one line away.

use eterea_logic::is_safe_https_url;

use crate::url_policy::KNOWN_GAPS;

/// INV-MED-02: every recorded `KNOWN_GAPS` literal is rejected by the current
/// policy. Vacuous while the corpus is empty; expected to FAIL if a new gap is
/// added, which is the point.
#[kani::proof]
#[kani::unwind(16)]
fn url_rejects_recorded_gaps() {
    for url in KNOWN_GAPS {
        assert!(
            !is_safe_https_url(url),
            "INV-MED-02 gap: accepted a non-canonical loopback spelling"
        );
    }
}
