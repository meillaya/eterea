#!/usr/bin/env bash
# Run Kani 0.68.0 against the standalone harness crate verification/kani.
#
#   scripts/verify/kani.sh                 # verify all baseline harnesses
#   scripts/verify/kani.sh --gaps          # also verify classes with known
#                                          # gaps (expected to FAIL until the
#                                          # Lead tightens src/logic; see
#                                          # verification/FINDINGS.md)
#   scripts/verify/kani.sh --harness url_accepts_only_https   # one harness
#   scripts/verify/kani.sh --list          # list harnesses without verifying
#
# Everything (registry cache, target dir, Kani artifacts) stays under .tools/.
# Kani is optional: if .tools/kani is missing the script prints the install
# command and exits 4, so `all.sh` can report SKIP instead of a fake pass.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
# shellcheck source=versions.env
source "${SCRIPT_DIR}/versions.env"

TOOLS="${ROOT}/.tools"
KANI="${TOOLS}/kani/bin"
HARNESS_DIR="${ROOT}/verification/kani"

if [ ! -x "${KANI}/cargo-kani" ]; then
    echo "kani: not installed at ${TOOLS}/kani" >&2
    echo "kani: install it with: bash scripts/verify/install.sh kani" >&2
    exit 4
fi
if [ ! -f "${HARNESS_DIR}/Cargo.toml" ]; then
    echo "kani: harness crate missing at ${HARNESS_DIR}" >&2
    exit 2
fi

FEATURES=()
PASSTHROUGH=()
for arg in "$@"; do
    case "${arg}" in
        --gaps) FEATURES+=(--features known-gaps) ;;
        *) PASSTHROUGH+=("${arg}") ;;
    esac
done

export PATH="${KANI}:${PATH}"
export CARGO_HOME="${TOOLS}/cargo-home"
export CARGO_TARGET_DIR="${TOOLS}/target/kani"
mkdir -p "${CARGO_HOME}" "${CARGO_TARGET_DIR}"

cd "${HARNESS_DIR}"
echo "kani: cargo-kani $("${KANI}/cargo-kani" --version 2>&1 | head -1) in ${HARNESS_DIR}"
exec cargo kani "${FEATURES[@]}" "${PASSTHROUGH[@]}"
