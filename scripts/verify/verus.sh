#!/usr/bin/env bash
# Run Verus 0.2026.11.02.8bf68ee against verification/verus.
#
#   scripts/verify/verus.sh                       # check every .rs reference file
#   scripts/verify/verus.sh verification/verus/url_policy.rs
#   scripts/verify/verus.sh <any verus flags>     # passthrough
#
# The pinned Verus release is rustup-managed: the wrapper needs `rustup` on
# PATH and the VERUS_TOOLCHAIN toolchain in .tools/rustup. Both are installed
# by scripts/verify/install.sh verus. The environment is fully local to the
# workspace: RUSTUP_HOME/CARGO_HOME under .tools, no ~/.rustup writes.
#
# Verus is optional: if .tools/verus is missing the script prints the install
# command and exits 4 so `all.sh` can report SKIP.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
# shellcheck source=versions.env
source "${SCRIPT_DIR}/versions.env"

TOOLS="${ROOT}/.tools"
VERUS_DIR="${TOOLS}/verus"
RUSTUP_HOME="${TOOLS}/rustup"
RUSTUP_CARGO_HOME="${TOOLS}/cargo-home-rustup"
VERUS_SRC_DIR="${ROOT}/verification/verus"

if [ ! -x "${VERUS_DIR}/verus" ]; then
    echo "verus: not installed at ${VERUS_DIR}" >&2
    echo "verus: install it with: bash scripts/verify/install.sh verus" >&2
    exit 4
fi
if [ ! -x "${RUSTUP_CARGO_HOME}/bin/rustup" ]; then
    echo "verus: rustup missing at ${RUSTUP_CARGO_HOME}/bin/rustup" >&2
    echo "verus: install it with: bash scripts/verify/install.sh verus" >&2
    exit 4
fi
if [ ! -d "${VERUS_SRC_DIR}" ]; then
    echo "verus: source dir missing at ${VERUS_SRC_DIR}" >&2
    exit 2
fi

export RUSTUP_HOME
export CARGO_HOME="${RUSTUP_CARGO_HOME}"
export PATH="${RUSTUP_CARGO_HOME}/bin:${PATH}"
export RUSTUP_TOOLCHAIN="${VERUS_TOOLCHAIN}"

if [ "$#" -gt 0 ]; then
    exec "${VERUS_DIR}/verus" "$@"
fi

status=0
shopt -s nullglob
files=("${VERUS_SRC_DIR}"/*.rs)
if [ "${#files[@]}" -eq 0 ]; then
    echo "verus: no .rs files under ${VERUS_SRC_DIR}" >&2
    exit 2
fi
for file in "${files[@]}"; do
    echo "verus: checking $(realpath --relative-to="${ROOT}" "${file}")"
    # --crate-type lib: the reference files are specifications, not binaries.
    if ! "${VERUS_DIR}/verus" --crate-type=lib "${file}"; then
        status=1
    fi
done
exit "${status}"
