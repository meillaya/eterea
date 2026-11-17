#!/usr/bin/env bash
# CI entry point for eterea's formal verification gate.
#
# Runs the pinned prover layers with explicit, overridable time budgets:
#
#   verus      deductive reference-model proofs     VERUS_BUDGET_SECONDS       default 300
#   kani       fast verified harness set            KANI_FAST_BUDGET_SECONDS   default 900
#   miri       verification/miri runtime checks     MIRI_BUDGET_SECONDS        default 900
#   kani-full  ALL baseline Kani harnesses          KANI_FULL_BUDGET_SECONDS   default 900
#              (ADVISORY, only with --full; never a pass/fail gate)
#
# Evidence is written under .tools/logs/ (workspace-local, uploaded as a CI
# artifact):
#   ci-status.txt      one line per layer: PASS / FAIL / TIMEOUT / ADVISORY
#   ci-verus.log       full Verus output
#   ci-kani.log        full Kani fast-gate output
#   ci-miri.log        full Miri output
#   ci-kani-full.log   full Kani advisory-run output (only with --full)
#
# A markdown table is appended to $GITHUB_STEP_SUMMARY when that variable is set.
#
# Exit code: 0 when every mandatory layer passes; 1 otherwise. Advisory Kani
# failures/timeouts are recorded as ADVISORY and do not change the exit code.
#
# Scope is the standalone verification crates only (eterea-logic,
# eterea-ui-model, verification/verus reference model). It is NOT a whole-app
# verification claim: the desktop/Rune/GPU/ffmpeg path is out of scope.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
# shellcheck source=versions.env
source "${SCRIPT_DIR}/versions.env"

TOOLS="${ROOT}/.tools"
LOGS="${TOOLS}/logs"
STATUS_FILE="${LOGS}/ci-status.txt"

RUN_FULL=0
for arg in "$@"; do
    case "${arg}" in
        --full) RUN_FULL=1 ;;
        -h|--help) sed -n '2,34p' "$0"; exit 0 ;;
        *) echo "ci.sh: unknown argument: ${arg}" >&2; exit 2 ;;
    esac
done

VERUS_BUDGET_SECONDS="${VERUS_BUDGET_SECONDS:-300}"
KANI_FAST_BUDGET_SECONDS="${KANI_FAST_BUDGET_SECONDS:-900}"
KANI_FULL_BUDGET_SECONDS="${KANI_FULL_BUDGET_SECONDS:-900}"
MIRI_BUDGET_SECONDS="${MIRI_BUDGET_SECONDS:-900}"

# Harnesses that passed the local 2026-11-17 reference run. Keep this list in
# sync with verification/kani/README.md and verification/COVERAGE.md.
KANI_FAST_HARNESSES=(
    action_for_table_matches_spec
    action_for_unmapped_keys_are_none
    action_for_future_keys_are_not_global
    action_for_single_byte_keys
    url_rejects_fragments
)

mkdir -p "${LOGS}"
: > "${STATUS_FILE}"
overall=0
declare -a SUMMARY_ROWS=()

record() { # record <layer> <result> <budget_s> <elapsed_s> <detail>
    local row
    row="$(printf '%-10s %-9s budget=%-5s elapsed=%-5s %s' "$1" "$2" "${3}s" "${4}s" "$5")"
    printf '%s\n' "${row}" | tee -a "${STATUS_FILE}"
}

# run_layer <layer> <budget_s> <logfile> <mandatory:1|0> <command...>
run_layer() {
    local layer="$1" budget="$2" logfile="$3" mandatory="$4"
    shift 4
    local start end elapsed code result detail=""
    start="$(date +%s)"
    timeout "${budget}" "$@" >"${logfile}" 2>&1
    code=$?
    end="$(date +%s)"
    elapsed=$((end - start))
    if [ "${code}" -eq 0 ]; then
        result="PASS"
    elif [ "${code}" -eq 124 ]; then
        result="TIMEOUT"
        detail="exceeded ${budget}s budget; see $(basename "${logfile}")"
    else
        result="FAIL"
        detail="exit ${code}; see $(basename "${logfile}")"
    fi
    if [ "${mandatory}" -eq 0 ]; then
        if [ "${result}" = "PASS" ]; then
            result="PASS(adv)"
        else
            result="ADVISORY"
            detail="${result}: ${detail}"
        fi
    fi
    record "${layer}" "${result}" "${budget}" "${elapsed}" "${detail}"
    SUMMARY_ROWS+=("${layer}|${result}|${budget}s|${elapsed}s|${detail}")
    printf '\n--- %s output (tail) ---\n' "${layer}"
    tail -n 8 "${logfile}" 2>/dev/null || true

    if [ "${mandatory}" -eq 1 ] && { [ "${code}" -ne 0 ]; }; then
        overall=1
    fi
    return 0
}

echo "eterea formal verification gate"
echo "  repo:   ${ROOT}"
echo "  tools:  ${TOOLS}"
echo "  budgets: verus=${VERUS_BUDGET_SECONDS}s kani-fast=${KANI_FAST_BUDGET_SECONDS}s" \
    "miri=${MIRI_BUDGET_SECONDS}s kani-full=${KANI_FULL_BUDGET_SECONDS}s (advisory)"
echo ""

run_layer verus "${VERUS_BUDGET_SECONDS}" "${LOGS}/ci-verus.log" 1 \
    bash "${SCRIPT_DIR}/verus.sh"

KANI_ARGS=()
for harness in "${KANI_FAST_HARNESSES[@]}"; do
    KANI_ARGS+=(--harness "${harness}")
done
run_layer kani "${KANI_FAST_BUDGET_SECONDS}" "${LOGS}/ci-kani.log" 1 \
    bash "${SCRIPT_DIR}/kani.sh" "${KANI_ARGS[@]}"

run_layer miri "${MIRI_BUDGET_SECONDS}" "${LOGS}/ci-miri.log" 1 \
    bash "${SCRIPT_DIR}/miri.sh"

if [ "${RUN_FULL}" -eq 1 ]; then
    # Advisory: the seven expensive URL/MIME harnesses need >240 s of CBMC each
    # on the reference host (std string/IP/case-folding symbolically executed).
    # A timeout or failure here is recorded, not gated; their corpora are
    # runtime-covered by cargo test and Miri.
    run_layer kani-full "${KANI_FULL_BUDGET_SECONDS}" "${LOGS}/ci-kani-full.log" 0 \
        bash "${SCRIPT_DIR}/kani.sh"
fi

echo ""
echo "formal verification status (${STATUS_FILE}):"
cat "${STATUS_FILE}"
echo ""
echo "note: scope is the standalone verification crates and Verus reference model."
echo "      This is not a whole-app verification claim; see docs/verification/formal-verification.md."

if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    {
        echo "### Formal verification gate (\`scripts/verify/ci.sh\`)"
        echo ""
        echo "| layer | result | budget | elapsed | detail |"
        echo "| --- | --- | --- | --- | --- |"
        for row in "${SUMMARY_ROWS[@]}"; do
            IFS='|' read -r layer result budget elapsed detail <<<"${row}"
            echo "| ${layer} | ${result} | ${budget} | ${elapsed} | ${detail} |"
        done
        echo ""
        echo "Scope: standalone verification crates + Verus reference model only."
        echo "Not a whole-app verification claim; see docs/verification/formal-verification.md."
    } >> "${GITHUB_STEP_SUMMARY}"
fi

if [ "${overall}" -ne 0 ]; then
    echo "formal verification gate: FAILED (a mandatory layer failed or timed out)" >&2
    exit 1
fi
echo "formal verification gate: PASSED (mandatory layers)"
exit 0
