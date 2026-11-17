# Verus layer

Deductive verification plan and reference model for eterea's URL policy,
checked with **Verus 0.2026.11.02.8bf68ee**.

```
verification/verus/
├── README.md       # this file: what is proved, what moves into Verus, what does not
└── url_policy.rs   # Verus reference model of INV-MED-01..04
```

## Current status

`scripts/verify/verus.sh` on 2026-11-17:

```
verus: checking verification/verus/url_policy.rs
verification results:: 10 verified, 0 errors
```

What that does and does not mean:

* It proves 10 lemmas **of the reference model in `url_policy.rs`**: accepted
  URLs have an HTTPS scheme, a non-empty authority and host, no userinfo, and a
  non-loopback literal host; URLs without the scheme or with a loopback-literal
  host are rejected. The executable `has_https_scheme_exec` is proved equivalent
  to its spec.
* It is **not a proof of `src/logic/src/lib.rs`**. That function is ordinary
  Rust built on `str::strip_prefix/split/find`, `str::chars` and
  `std::net::IpAddr::from_str`; Verus has no verified model of those std
  routines. The reference model is a separate byte-level specification kept
  deliberately close to the intended implementation.
* The recursive parser specs (`first_byte`, `authority`, `host`) are definition
  models, not evaluated in the proofs: the lemmas are definitional implications
  over an arbitrary `Seq<u8>`. Evaluating recursive specs on concrete literals
  is where Verus cost blows up for zero added assurance, so concrete regression
  cases for those parts live in Kani/Miri/cargo test instead.

The executable mirror covers only the scheme check because that is the one part
cheap to re-express and verify today. `has_https_scheme_exec` is the shape the
rest of the parser would take.

## What would move into Verus (the plan)

To prove the real `is_safe_https_url`, the Lead (owner of `src/logic`) would
rewrite it as a Verus-annotated crate, as a sequence of small verified steps:

1. **Scheme split.** Find `://` with a verified search loop; prove the returned
   scheme index is a real separator and that `has_https_scheme_exec` matches
   `has_https_scheme`. No std string methods.
2. **ASCII/control validation.** Verified byte loop proving no byte is a control
   character or ASCII whitespace (`u8` classification is a pure function).
   `chars()` is dropped in favour of bytes; non-ASCII stays rejected as today.
3. **Authority extraction.** Verified byte loop finding the first `/` or `?`
   and a function mirroring `authority`; prove the extracted slice is a
   sub-slice of the input and its length matches the spec function.
4. **Userinfo and port.** Verified byte scans for `@` in the authority and for
   `:port` validation (`1..=65535`, digits). Straightforward loop invariants.
5. **Host extraction and trailing dots.** Verified bracket/port split plus a
   `trim_end_matches('.')` loop; prove the host relation matches the spec.
6. **Address classification.** This is the hard boundary. Either:
   * write verified parsers for dotted-quad IPv4 and canonical IPv6 literals
     (weeks of work, substantial spec surface), or
   * keep `std::net::IpAddr::from_str` as a **trusted extern spec**: declare its
     postcondition (returned `IpAddr` is public iff the literal is public) and
     state explicitly in `COVERAGE.md` that std's parser is trusted. Kani/Miri
     then cover the concrete and bounded behaviour of that path.
   Recommendation: do (b) first, prove everything around it, and keep the
   literal-classification in Kani/Miri until a verified parser is justified.
7. **Hostcharacter fallback.** Verified predicate over alphanumerics/dot/dash/
   underscore, with the F-1 non-canonical-IPv4 finding resolved as part of this
   step (see `verification/FINDINGS.md`).

After that, the exec fn gets `ensures result == is_safe_https_url(s@)` and the
Verus file becomes the real proof instead of a reference model. `cargo-verus`
can build the crate in CI; `scripts/verify/verus.sh` already pins the tool.

## What stays Kani-only

* Properties whose inputs are too large/symbolic for Verus' SMT encoding but
  fine for bounded model checking: symbolic single-byte key mapping, 9–12 byte
  symbolic URL classes, `#[cfg(kani)]`-gated exhaustive checks.
* Anything relying on the real, unverified std implementations (string
  comparison, `IpAddr::from_str` internals) until step 6's trusted-extern
  decision is made.
* Regression proofs over concrete corpora (cheaper to express as Kani
  harnesses, and the same corpus runs in `cargo test`).

## What stays Miri / runtime

* UB detection in the exercised paths (`verification/miri`, plain tests).
* DNS resolution, TCP/TLS behaviour, redirect handling and request hygiene
  (INV-MED-05..08): these are runtime properties of the media pipeline, not
  provable from pure functions.
* Everything UI/geometry/rendering: Rune harness once the desktop shell exists.

## Commands

```bash
scripts/verify/install.sh verus     # Verus + rustup-managed toolchain 1.99.1
scripts/verify/verus.sh             # check every *.rs under verification/verus
scripts/verify/verus.sh verification/verus/url_policy.rs
```

The pinned Verus release is rustup-managed: the wrapper needs `rustup` on PATH
and the toolchain recorded in its `version.json` (`1.99.1` here). Both are
installed under `.tools/`; no global rustup state is read or written.
