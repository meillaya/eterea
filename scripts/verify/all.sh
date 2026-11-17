#!/usr/bin/env bash
# Run every verification layer that is installed under .tools/.
#
#   scripts/verify/all.sh                  # normal: missing tools are SKIP
#   scripts/verify/all.sh --strict         # missing tools are FAIL
#   scripts/verify/all.sh --gaps           # include the known-gap Kani proofs
#   scripts/verify/all.sh --skip-kani --skip-miri --skip-verus
#   scripts/verify/all.sh --fast        # Kani: only the quickly-verified set
#
# Layers, in order:
#   1. harness   cargo build/test of verification/kani without Kani. This proves
#                the harness crate and the property helpers compile on stable
#                Rust; the #[kani::proof] items are cfg(kani)-gated.
#   2. kani      scripts/verify/kani.sh   (model checking; pinned Kani 0.68.0)
#   3. miri      scripts/verify/miri.sh   (UB/interpretation check on pinned
#                nightly; verification/miri crate)
#   4. verus     scripts/verify/verus.sh  (deductive proofs; pinned Verus)
#
# Results are also appended to .tools/logs/all-status.txt (workspace-local,
# never committed). The exit code is 0 only if every executed layer passed;
# SKIP is not a pass unless --strict turns it into a failure.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
# shellcheck source=versions.env
source "${SCRIPT_DIR}/versions.env"

TOOLS="${ROOT}/.tools"
LOGS="${TOOLS}/logs"
STATUS_LOG="${LOGS}/all-status.txt"
CARGO="${CARGO:-cargo}"

STRICT=0
RUN_KANI=1
RUN_MIRI=1
RUN_VERUS=1
KANI_EXTRA=()
for arg in "$@"; do
    case "${arg}" in
        --strict) STRICT=1 ;;
        --gaps) KANI_EXTRA+=(--gaps) ;;
        --fast) KANI_EXTRA+=(--harness action_for_table_matches_spec --harness action_for_unmapped_keys_are_none --harness action_for_future_keys_are_not_global --harness action_for_single_byte_keys --harness url_rejects_fragments) ;;
        --skip-kani) RUN_KANI=0 ;;
        --skip-miri) RUN_MIRI=0 ;;
        --skip-verus) RUN_VERUS=0 ;;
        -h|--help) sed -n '2,32p' "$0"; exit 0 ;;
        *) echo "all.sh: unknown argument: ${arg}" >&2; exit 2 ;;
    esac
done

mkdir -p "${LOGS}"
: > "${STATUS_LOG}.new"
overall=0

say() { printf '%s\n' "$*"; }
record() { # record <layer> <PASS|FAIL|SKIP> <detail>
    printf '%-9s %-5s %s\n' "$1" "$2" "$3" | tee -a "${STATUS_LOG}.new"
}

say "eterea verification layers"
say "  repo:   ${ROOT}"
say "  rustc:  $(rustc --version 2>/dev/null || echo 'rustc not on PATH')"
if [ -x "${TOOLS}/verus/verus" ]; then
    say "  verus:  ${TOOLS}/verus (pin ${VERUS_VERSION}, toolchain ${VERUS_TOOLCHAIN})"
else
    say "  verus:  not installed (${TOOLS}/verus)"
fi
if [ -x "${TOOLS}/kani/bin/cargo-kani" ]; then
    say "  kani:   ${TOOLS}/kani (pin ${KANI_VERSION})"
else
    say "  kani:   not installed (${TOOLS}/kani)"
fi
if [ -x "${TOOLS}/rustup/toolchains/${MIRI_NIGHTLY}-x86_64-unknown-linux-gnu/bin/cargo-miri" ]; then
    say "  miri:   ${MIRI_NIGHTLY} (pin in versions.env)"
else
    say "  miri:   not installed (${TOOLS}/rustup)"
fi
say ""

# --- 1. harness crate on stable Rust -----------------------------------------
say "[harness] cargo test (verification/kani, cfg(kani) proofs disabled)"
if ( cd "${ROOT}/verification/kani" && \
     CARGO_HOME="${TOOLS}/cargo-home" CARGO_TARGET_DIR="${TOOLS}/target/harness" \
     timeout "${BUILD_TIMEOUT:-300}" "${CARGO}" test --quiet 2>&1 | tee "${LOGS}/harness.log" ); then
    record harness PASS "cargo test verification/kani"
else
    record harness FAIL "see ${LOGS}/harness.log"
    overall=1
fi

# --- 2. Kani -----------------------------------------------------------------
if [ "${RUN_KANI}" -eq 0 ]; then
    record kani SKIP "--skip-kani"
elif [ ! -x "${TOOLS}/kani/bin/cargo-kani" ]; then
    if [ "${STRICT}" -eq 1 ]; then record kani FAIL "not installed"; overall=1; else record kani SKIP "not installed (bash scripts/verify/install.sh kani)"; fi
else
    say "[kani] ${KANI_VERSION}: baseline model-checking harnesses"
    if bash "${SCRIPT_DIR}/kani.sh" "${KANI_EXTRA[@]}" 2>&1 | tee "${LOGS}/kani.log"; then
        record kani PASS "cargo kani (baseline)"
    else
        record kani FAIL "see ${LOGS}/kani.log"
        overall=1
    fi
fi

# --- 3. Miri -----------------------------------------------------------------
if [ "${RUN_MIRI}" -eq 0 ]; then
    record miri SKIP "--skip-miri"
elif [ ! -x "${TOOLS}/rustup/toolchains/${MIRI_NIGHTLY}-x86_64-unknown-linux-gnu/bin/cargo-miri" ]; then
    if [ "${STRICT}" -eq 1 ]; then record miri FAIL "not installed"; overall=1; else record miri SKIP "not installed (bash scripts/verify/install.sh miri)"; fi
else
    say "[miri] ${MIRI_NIGHTLY}: cargo miri test"
    if bash "${SCRIPT_DIR}/miri.sh" 2>&1 | tee "${LOGS}/miri.log"; then
        record miri PASS "cargo miri test verification/miri"
    else
        record miri FAIL "see ${LOGS}/miri.log"
        overall=1
    fi
fi

# --- 4. Verus ----------------------------------------------------------------
if [ "${RUN_VERUS}" -eq 0 ]; then
    record verus SKIP "--skip-verus"
elif [ ! -x "${TOOLS}/verus/verus" ]; then
    if [ "${STRICT}" -eq 1 ]; then record verus FAIL "not installed"; overall=1; else record verus SKIP "not installed (bash scripts/verify/install.sh verus)"; fi
else
    say "[verus] ${VERUS_VERSION}: deductive reference proofs"
    if bash "${SCRIPT_DIR}/verus.sh" 2>&1 | tee "${LOGS}/verus.log"; then
        record verus PASS "verus verification/verus"
    else
        record verus FAIL "see ${LOGS}/verus.log"
        overall=1
    fi
fi

mv "${STATUS_LOG}.new" "${STATUS_LOG}"
say ""
if [ "${overall}" -eq 0 ]; then
    say "all executed verification layers passed (see ${STATUS_LOG}; SKIP means the pinned tool is not installed)"
else
    say "one or more verification layers failed; see ${STATUS_LOG} and ${LOGS}/*.log" >&2
fi
say "reminder: this covers the pure crates and the reference proofs only; it is not a proof of the whole app. See verification/COVERAGE.md."
exit "${overall}"
