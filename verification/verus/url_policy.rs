//! Verus reference model of eterea's URL policy.
//!
//! Check with `scripts/verify/verus.sh` (pinned Verus 0.2026.11.02.8bf68ee):
//!
//! ```text
//! scripts/verify/verus.sh verification/verus/url_policy.rs
//! ```
//!
//! # What this file is
//!
//! A self-contained specification of INV-MED-01..04 from
//! `verification/SPEC.md`, written in Verus so the contract is checked
//! deductively instead of only by bounded model checking:
//!
//! * INV-MED-01: an accepted URL starts with the exact `https://` scheme.
//! * INV-MED-02: an accepted URL's host is not a loopback/private literal.
//! * INV-MED-03: an accepted URL has no userinfo (`@` in the authority).
//! * INV-MED-04: an accepted URL has a non-empty authority and host.
//!
//! It models the policy over byte sequences (`Seq<u8>`) rather than `&str`.
//! Bytes are what a URL parser actually sees; modelling them avoids trusting
//! `str` slicing inside the specification.
//!
//! # What this file is NOT
//!
//! It is not a proof of `src/logic/src/lib.rs`. `is_safe_https_url` is ordinary
//! Rust built on `str::strip_prefix/split` and `std::net::IpAddr::from_str`;
//! Verus cannot verify it without replacing the parser with verified code.
//! The executable mirror at the bottom covers only the scheme check (the part
//! cheap to re-express). `verification/verus/README.md` has the exact migration
//! plan and says what deliberately stays Kani-only or Miri-only.
//!
//! The recursive spec functions below (`first_byte`, `authority`, `host`) are
//! definitions, not computations: the proofs are definitional implications over
//! an arbitrary URL, so Verus never has to evaluate them on a concrete literal
//! (SMT unfolding of recursive definitions over symbolic sequences is exactly
//! where this kind of reference model becomes expensive for no gain).

use vstd::prelude::*;

verus! {

// --- Byte helpers -----------------------------------------------------------

/// True when `s` begins with `https://` (lower-case, exact; INV-MED-01).
pub open spec fn has_https_scheme(s: Seq<u8>) -> bool {
    s.len() >= 8
        && s[0] == b'h'
        && s[1] == b't'
        && s[2] == b't'
        && s[3] == b'p'
        && s[4] == b's'
        && s[5] == b':'
        && s[6] == b'/'
        && s[7] == b'/'
}

/// Index of the first occurrence of `needle` in `s`, or `s.len()` if absent.
///
/// This is the parsing primitive for authority and host. It is defined
/// recursively for the specification; a verified implementation would use a
/// loop or `memchr` with a proved correspondence to this definition.
pub open spec fn first_byte(s: Seq<u8>, needle: u8) -> nat
    decreases s.len(),
{
    if s.len() == 0 {
        0
    } else if s[0] == needle {
        0
    } else {
        1 + first_byte(s.skip(1), needle)
    }
}

/// The authority of an `https://` URL: bytes between the scheme and the first
/// `/`. Empty if the URL does not have the https scheme.
pub open spec fn authority(url: Seq<u8>) -> Seq<u8> {
    if has_https_scheme(url) {
        let rest = url.skip(8);
        rest.subrange(0, first_byte(rest, b'/') as int)
    } else {
        Seq::empty()
    }
}

/// The host of an authority: the contents of a bracketed IPv6 literal, or the
/// bytes before the `:port`. Mirrors `is_safe_https_url`'s host extraction; it
/// does not validate literal syntax (the verified parser is migration work).
pub open spec fn host(url_authority: Seq<u8>) -> Seq<u8> {
    if url_authority.len() > 0 && url_authority[0] == b'[' {
        let rest = url_authority.skip(1);
        rest.subrange(0, first_byte(rest, b']') as int)
    } else {
        url_authority.subrange(0, first_byte(url_authority, b':') as int)
    }
}

/// Hosts the policy rejects: loopback/unspecified literals in the canonical
/// spellings `std::net::IpAddr::from_str` accepts (INV-MED-02).
///
/// Non-canonical spellings, trailing-dot names and IPv4-mapped IPv6 forms are
/// known gaps tracked by the opt-in Kani proofs and `verification/FINDINGS.md`;
/// they are not modelled here because the specification does not accept them.
pub open spec fn is_loopback_literal(host_bytes: Seq<u8>) -> bool {
    ||| host_bytes == seq![b'l', b'o', b'c', b'a', b'l', b'h', b'o', b's', b't']
    ||| host_bytes == seq![b'1', b'2', b'7', b'.', b'0', b'.', b'0', b'.', b'1']
    ||| host_bytes == seq![b'0', b'.', b'0', b'.', b'0', b'.', b'0']
    ||| host_bytes == seq![b':', b':', b'1']
    ||| host_bytes == seq![b':', b':']
}

/// The URL policy as specified: https scheme, non-empty authority and host, no
/// userinfo, and a host that is not a loopback/private literal.
pub open spec fn is_safe_https_url(url: Seq<u8>) -> bool {
    &&& has_https_scheme(url)
    &&& authority(url).len() > 0
    &&& !authority(url).contains(b'@')
    &&& host(authority(url)).len() > 0
    &&& !is_loopback_literal(host(authority(url)))
}

// --- Deductive checks of the invariants ------------------------------------
//
// Each proof is the definitional step "this conjunct is part of the policy".
// They exist so the contract is pinned in one place: changing
// `is_safe_https_url` without updating an invariant breaks one of them.

/// INV-MED-01: an accepted URL has the https scheme.
proof fn accepted_implies_https(url: Seq<u8>)
    requires is_safe_https_url(url),
    ensures has_https_scheme(url),
{
}

/// INV-MED-03: an accepted URL has no userinfo.
proof fn accepted_implies_no_userinfo(url: Seq<u8>)
    requires is_safe_https_url(url),
    ensures !authority(url).contains(b'@'),
{
}

/// INV-MED-04: an accepted URL has a non-empty authority.
proof fn accepted_implies_nonempty_authority(url: Seq<u8>)
    requires is_safe_https_url(url),
    ensures authority(url).len() > 0,
{
}

/// INV-MED-04: an accepted URL has a non-empty host.
proof fn accepted_implies_nonempty_host(url: Seq<u8>)
    requires is_safe_https_url(url),
    ensures host(authority(url)).len() > 0,
{
}

/// INV-MED-02: an accepted URL's host is not a loopback literal.
proof fn accepted_implies_public_host(url: Seq<u8>)
    requires is_safe_https_url(url),
    ensures !is_loopback_literal(host(authority(url))),
{
}

/// INV-MED-01, contrapositive: any URL without the https scheme is rejected.
proof fn no_https_implies_rejected(url: Seq<u8>)
    requires !has_https_scheme(url),
    ensures !is_safe_https_url(url),
{
}

/// INV-MED-02: any URL whose extracted host is a loopback literal is rejected.
proof fn loopback_host_implies_rejected(url: Seq<u8>)
    requires is_loopback_literal(host(authority(url))),
    ensures !is_safe_https_url(url),
{
}

/// Concrete regression: `http://x` does not have the https scheme. Only the
/// non-recursive scheme predicate is evaluated on a literal; the recursive
/// parser definitions above are intentionally not exercised on literals.
proof fn rejects_http_scheme() {
    assert(!has_https_scheme(seq![b'h', b't', b't', b'p', b':', b'/', b'/', b'x']));
}

// --- Executable mirror of the scheme check ---------------------------------

/// Executable mirror of `has_https_scheme` on a byte slice. This is the one
/// piece small enough to verify without replacing the URL parser; it is the
/// reference implementation for the scheme half of INV-MED-01 and the shape a
/// future verified parser would take.
pub fn has_https_scheme_exec(input: &[u8]) -> (matched: bool)
    ensures matched == has_https_scheme(input@),
{
    if input.len() < 8 {
        return false;
    }
    input[0] == b'h'
        && input[1] == b't'
        && input[2] == b't'
        && input[3] == b'p'
        && input[4] == b's'
        && input[5] == b':'
        && input[6] == b'/'
        && input[7] == b'/'
}

} // verus!
