//! Bounded properties for [`eterea_logic::is_safe_https_url`].
//!
//! These corpora are the single source of truth shared by:
//! * the plain-Rust tests in `tests/properties.rs` (run by `cargo test`),
//! * the Miri tests in `verification/miri` (run by `scripts/verify/miri.sh`),
//! * the `#[kani::proof]` harnesses in `src/proofs/url.rs` (run by `cargo kani`).
//!
//! Invariant ids refer to `verification/SPEC.md`:
//! * INV-MED-01: an accepted URL's scheme is HTTPS (RFC 3986 makes the scheme
//!   case-insensitive; `HTTP`, `FTP` and non-scheme strings are rejected).
//! * INV-MED-02: an accepted URL's host is public; loopback, private,
//!   link-local, unique-local, multicast and unspecified literals are rejected,
//!   including IPv6 literals and IPv4-mapped IPv6 forms.
//! * INV-MED-03: an accepted URL has no userinfo (`user:pass@` or `user@`).
//! * INV-MED-04: an accepted URL has a non-empty, syntactically plausible host
//!   (no empty/query-only authority, no invalid port, no fragment, no control
//!   characters).
//!
//! The implementation lives in `src/logic` and is owned by the Lead; this crate
//! only observes it. **Re-run these checks whenever `src/logic` changes**: the
//! corpora intentionally pin today's contract so a silent loosening fails CI.

use eterea_logic::is_safe_https_url;

/// URLs the policy must reject. Every entry was verified rejected on
/// 2026-11-17 against `src/logic/src/lib.rs`; the list is the regression floor
/// for INV-MED-01..04.
pub const MUST_REJECT: &[&str] = &[
    // scheme / shape
    "http://pbs.twimg.com/media/x.jpg",
    "ftp://example.com/x",
    "example.com/x",
    "://example.com/",
    "https:example.com/",
    "https://",
    "https:///x",
    // fragment and control characters
    "https://example.com/#frag",
    "https://x.com/path#frag",
    "https://exa mple.com/",
    "https://exa\tmple.com/",
    "https://example.com/\u{0000}",
    " https://x.com/",
    // userinfo
    "https://user:pass@x.com/x",
    "https://user@x.com/x",
    "https://@x.com/",
    "https://x.com@evil.com/",
    // empty or malformed authority
    "https://?q=1",
    "https://[::1",
    // port validation
    "https://example.com:0/x",
    "https://example.com:65536/x",
    "https://example.com:/x",
    "https://example.com:abc/x",
    // loopback / private / link-local / multicast / unspecified IPv4
    "https://localhost/x",
    "https://LOCALHOST/x",
    "https://localhost./x",
    "https://a.localhost/x",
    "https://127.0.0.1/x",
    "https://127.0.0.2/x",
    "https://127.0.0.1./x",
    // non-canonical numeric IPv4 spellings that resolvers still translate to
    // loopback/private addresses (finding F-1, fixed 2026-11-17)
    "https://127.1/x",
    "https://127.0.1/x",
    "https://10.1/x",
    "https://2130706433/",
    "https://0x7f.0.0.1/",
    "https://0177.0.0.1/",
    "https://0x7f.1/",
    "https://1.2.3.999/",
    "https://1.2.3.4.5/",
    "https://10.0.0.1/x",
    "https://172.16.0.1/x",
    "https://192.168.0.1/x",
    "https://169.254.1.1/x",
    "https://0.0.0.0/x",
    "https://224.0.0.1/x",
    "https://255.255.255.255/x",
    "https://100.64.0.1/x",
    "https://198.18.0.1/x",
    // loopback / unspecified / link-local / unique-local / multicast IPv6
    "https://[::1]/x",
    "https://[::]/x",
    "https://[0:0:0:0:0:0:0:1]/x",
    "https://[0000::1]/x",
    "https://[::ffff:127.0.0.1]/x",
    "https://[::ffff:10.0.0.1]/x",
    "https://[fe80::1]/x",
    "https://[fc00::1]/x",
    "https://[fd00::1]/x",
    "https://[ff02::1]/x",
    // conservative: non-ASCII (IDN) hosts must be punycode in the archive
    "https://例子.测试/",
];

/// URLs the policy accepts today and must keep accepting (INV-MED-01..04).
pub const MUST_ACCEPT: &[&str] = &[
    "https://pbs.twimg.com/media/x.jpg",
    "https://x.com/a/status/1",
    "https://example.com/",
    "https://sub.example.com/path?q=1",
    "https://example.com./",
    "https://example.com:8443/x",
    "https://example.com:65535/x",
    "HTTPS://example.com/x",
    "hTtPs://Example.COM/",
    "https://[2606:4700:4700::1111]/x",
    "https://xn--e1afmkfd.xn--p1ai/",
];

/// URLs the *specification* wants rejected but the current implementation
/// accepts. **Empty as of 2026-11-17: finding F-1 (non-canonical numeric IPv4
/// spellings) was fixed by the Lead in `src/logic`.** The mechanism is kept so
/// the next finding has a home: add the literal here, it is exercised by the
/// opt-in `known-gaps` Kani proofs and by a plain test, and record it in
/// `verification/FINDINGS.md`.
pub const KNOWN_GAPS: &[&str] = &[];

/// Small subset used by the Kani proofs instead of the full regression list.
///
/// CBMC's cost scales with the concrete corpus, and every URL runs std string
/// character iteration and IP parsing; the full corpora above stay in the plain
/// `cargo test` and Miri runs, where they are cheap. Kani proves the *shape* of
/// the contract over a representative boundary set; each entry exercises a
/// different rejection branch.
pub const KANI_MUST_REJECT: &[&str] = &[
    "http://x.com/",              // wrong scheme
    "https://user@x.com/x",      // userinfo
    "https://127.0.0.1/x",       // loopback literal
    "https://example.com/#frag", // fragment
];

/// Small accepted subset for Kani; covers scheme case, normal host, port.
pub const KANI_MUST_ACCEPT: &[&str] = &[
    "https://pbs.twimg.com/media/x.jpg",
    "HTTPS://example.com:8443/x",
];

/// Returns the authority of an `https://` URL as the policy parses it
/// (bytes after the scheme up to the first `/` or `?`), or `None` when the
/// scheme is not HTTPS.
pub fn authority(url: &str) -> Option<&str> {
    let (scheme, rest) = url.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    Some(&rest[..end])
}

/// Returns the host as the policy parses it, or `None` for a non-HTTPS URL.
pub fn host(url: &str) -> Option<&str> {
    let authority = authority(url)?;
    let host = match authority.strip_prefix('[') {
        Some(inner) => &inner[..inner.find(']')?],
        None => match authority.rsplit_once(':') {
            Some((host, _port)) if !host.contains(':') => host,
            Some(_) => authority,
            None => authority,
        },
    };
    Some(host.trim_end_matches('.'))
}

/// Returns a reason when `url` violates a property that every URL accepted by
/// the *current* implementation must satisfy. Returns `None` for rejected URLs:
/// the policy makes no promise about what it rejects, only what it accepts.
pub fn contract_violation(url: &str) -> Option<&'static str> {
    if !is_safe_https_url(url) {
        return None;
    }
    let (scheme, _rest) = url
        .split_once("://")
        .expect("accepted URL must contain a scheme separator");
    if !scheme.eq_ignore_ascii_case("https") {
        return Some("INV-MED-01: accepted URL scheme is not HTTPS");
    }
    if url.contains('#') {
        return Some("INV-MED-04: accepted URL contains a fragment");
    }
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Some("INV-MED-04: accepted URL contains control/whitespace characters");
    }
    let authority = authority(url).expect("accepted URL must have an authority");
    if authority.is_empty() {
        return Some("INV-MED-04: accepted URL has an empty authority");
    }
    if authority.contains('@') {
        return Some("INV-MED-03: accepted URL has userinfo");
    }
    let host = host(url).expect("accepted URL must have a host");
    if host.is_empty() {
        return Some("INV-MED-04: accepted URL has an empty host");
    }
    if host.eq_ignore_ascii_case("localhost") {
        return Some("INV-MED-02: accepted URL has a loopback host");
    }
    None
}

/// True when the current policy accepts a URL that the specification rejects.
/// Used by tests; a `true` here is a bug report against `src/logic`, not
/// expected behaviour.
pub fn is_known_gap(url: &str) -> bool {
    KNOWN_GAPS.contains(&url) && is_safe_https_url(url)
}
