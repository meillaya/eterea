#!/usr/bin/env bash
# Run the pinned Miri nightly against verification/miri.
#
#   scripts/verify/miri.sh              # cargo miri test on verification/miri
#   scripts/verify/miri.sh --no-strict-provenance   # drop -Zmiri-strict-provenance
#   scripts/verify/miri.sh <extra cargo miri args>
#
# The crate under verification/ is a standalone workspace that depends on
# eterea-logic and eterea-ui-model by path, so Miri never has to resolve the
# main workspace (which pulls the Rune git dependencies).
#
# Miri is optional: if the pinned nightly is missing the script prints the
# install command and exits 4 so `all.sh` can report SKIP.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
# shellcheck source=versions.env
source "${SCRIPT_DIR}/versions.env"

TOOLS="${ROOT}/.tools"
RUSTUP_HOME="${TOOLS}/rustup"
RUSTUP_CARGO_HOME="${TOOLS}/cargo-home-rustup"
RUSTUP_BIN="${RUSTUP_CARGO_HOME}/bin/rustup"
TOOLCHAIN_BIN="${RUSTUP_HOME}/toolchains/${MIRI_NIGHTLY}-x86_64-unknown-linux-gnu/bin"
MIRI_DIR="${ROOT}/verification/miri"

if [ ! -x "${TOOLCHAIN_BIN}/cargo-miri" ]; then
    echo "miri: ${MIRI_NIGHTLY} with miri is not installed under ${TOOLS}" >&2
    echo "miri: install it with: bash scripts/verify/install.sh miri" >&2
    exit 4
fi
if [ ! -f "${MIRI_DIR}/Cargo.toml" ]; then
    echo "miri: test crate missing at ${MIRI_DIR}" >&2
    exit 2
fi

MIRIFLAGS="${MIRIFLAGS:--Zmiri-strict-provenance}"
PASSTHROUGH=()
for arg in "$@"; do
    case "${arg}" in
        --no-strict-provenance) MIRIFLAGS="${MIRIFLAGS//-Zmiri-strict-provenance/}" ;;
        *) PASSTHROUGH+=("${arg}") ;;
    esac
done

export RUSTUP_HOME
export CARGO_HOME="${RUSTUP_CARGO_HOME}"
export PATH="${RUSTUP_CARGO_HOME}/bin:${PATH}"
export CARGO_TARGET_DIR="${TOOLS}/target/miri"
export MIRIFLAGS="${MIRIFLAGS}"
# cargo-miri keeps its sysroot in the platform cache directory. Point that at
# .tools so a verification run never writes under the user's home.
export XDG_CACHE_HOME="${TOOLS}/cache"
mkdir -p "${CARGO_TARGET_DIR}" "${XDG_CACHE_HOME}"

cd "${MIRI_DIR}"
echo "miri: ${MIRI_NIGHTLY} (${MIRI_RUSTC_VERSION}), MIRIFLAGS=${MIRIFLAGS}"
exec cargo "+${MIRI_NIGHTLY}" miri test "${PASSTHROUGH[@]}"
