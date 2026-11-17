# Verification findings

Findings produced by this verification work, reported against the owner of the
affected crate. Last updated: 2026-11-17. The verification harnesses in
`verification/kani` track `src/logic` as it is; each finding lists the check
that reproduces it.

The Lead owns `src/logic`; nothing in `verification/` or `scripts/verify/`
changes application behaviour. Fix the code or update the invariant
deliberately — do not loosen a corpus to make it pass.

**Open findings: none.** The URL policy was frozen by the Lead on 2026-11-17
after the fix below; the harness corpora were re-run against the frozen code.

## F-1 (RESOLVED 2026-11-17, medium): non-canonical numeric IPv4 spellings were accepted — INV-MED-02

Originally: `is_safe_https_url` fell back to a host-character check when
`std::net::IpAddr::from_str` failed, which accepted resolvers' inet_aton
spellings of loopback/private addresses.

Observed before the fix (2026-11-17, first harness run): accepted
`https://127.1/x`, `https://2130706433/`, `https://0x7f.0.0.1/`.

**Fix:** the Lead rewrote the host classification in `src/logic` so
non-canonical numeric hosts are rejected while ordinary hostnames keep working
(`127.1`, `2130706433`, `0x7f.0.0.1`, `0177.0.0.1`, `1.2.3.999`,
`1.2.3.4.5`, `127.0.1`, `10.1`, `0x7f.1` are all rejected now).

**Re-verified against the frozen code** (2026-11-17 15:0x):

* plain `cargo test` in `verification/kani`: 10 passed, 1 ignored (the ignored
  test is only meaningful if a gap exists), 0 failed;
* Miri (`scripts/verify/miri.sh`): see `COVERAGE.md` for the final numbers;
* the literals are now in `url_policy::MUST_REJECT`, so the regression is
  locked. `KNOWN_GAPS` is empty and `scripts/verify/kani.sh --gaps` is vacuous
  (it passes by construction); the opt-in mechanism stays for the next finding.

Reproduce the fix:

```bash
cd verification/kani && cargo test          # literal regression floor
scripts/verify/kani.sh --gaps               # vacuous now; would FAIL if a gap returned
```

## F-2 (resolved before the freeze, kept for the record): earlier policy gaps

The first harness run (before the Lead rewrote `src/logic`) observed the
following accepted-but-specified-private/malformed URLs. All are now rejected
by the current implementation and moved into `MUST_REJECT`:

| Input | Old result | Current result |
| --- | --- | --- |
| `https://[::ffff:127.0.0.1]/x` | accepted | rejected |
| `https://[::ffff:10.0.0.1]/x` | accepted | rejected |
| `https://[fe80::1]/x` | accepted | rejected |
| `https://[fd00::1]/x` | accepted | rejected |
| `https://localhost./x` | accepted | rejected |
| `https://127.0.0.1./x` | accepted | rejected |
| `https://?q=1` | accepted | rejected |
| `https://x.com:0/x`, `:99999` | accepted | rejected |
| `https://x.com/path#frag` | accepted | rejected |

This is why the harness crate's corpora are pinned to current behaviour: a
change in policy shows up as a failing corpus test, reviewed deliberately.

## F-3 (trusted, not a bug): DNS-resolved hostnames are not checked

`is_safe_https_url` validates the *literal* host only. A public hostname that
resolves to a private address is out of scope for the pure function and is
documented in its doc comment (INV-MED-06/media pipeline owns that residual
risk). No harness claims it is covered.

## F-4 (policy notes, not gaps)

* Non-ASCII (IDN) hosts are rejected because the host-character fallback is
  ASCII-only. That is fail-closed and fine for X/Twitter media URLs, which are
  punycode. It is listed in `MUST_REJECT` so the behaviour is explicit.
* The scheme is matched case-insensitively (`HTTPS://`, `hTtPs://` are
  accepted), consistent with RFC 3986. `verification/SPEC.md` states it that
  way; an earlier draft said "lower-case only" and was corrected.
* Fragments are rejected (the current doc comment says "no fragment"). The
  fragment is not sent to servers, so rejecting is stricter than strictly
  necessary; it is the frozen contract and is covered by a symbolic Kani
  harness + corpora.

## How to report the next finding

1. Add the concrete input to `KNOWN_GAPS` in
   `verification/kani/src/url_policy.rs` (or a new invariant in `SPEC.md` and a
   harness for non-URL findings).
2. Run `cd verification/kani && cargo test` and
   `scripts/verify/kani.sh --gaps`: the new entry is expected to FAIL, which is
   the finding.
3. Record it here with the invariant id, observed result, expected result and
   the exact command that reproduces it.
4. Message the Lead with crate path, input, command and expected behaviour.
5. When fixed: move the literal to `MUST_REJECT`, empty `KNOWN_GAPS`, and
   update `COVERAGE.md`.

## F-5 (RESOLVED 2026-11-17, high): malformed archive JS wrapper panicked the JSON parser — INV-IMP-01

`cargo-fuzz` found a panic in `JsonParser::extract_payload` while fuzzing the
import path. A wrapper that contains a `[` after a `]` made the inclusive slice
`&trimmed[start..=end]` use a start greater than its end:

```text
thread '<unnamed>' panicked at src/backend/src/ingestion/json_parser.rs:183:30:
byte range starts at 29 but ends at 28
```

Fuzz input (byte-for-byte): `window.YTD.tweets.part0 = K];[\n`.

The untrusted-import invariant is that malformed input returns an error, never a
panic (INV-IMP-01). Imported archive JavaScript is untrusted.

**Fix:** `extract_payload` now checks `end < start` and returns
`Error::InvalidFormat` before slicing. The regression test is
`ingestion::json_parser::tests::malformed_archive_wrapper_is_an_error_not_a_panic`
in `src/backend/src/ingestion/json_parser.rs`.

Reproduce:

```bash
cargo-fuzz run import_text fuzz/artifacts/import_text/<crash-file>
cargo test -p eterea-core --lib malformed_archive_wrapper
```

The fuzz target was rerun after the fix; see `COVERAGE.md` for the final run.
