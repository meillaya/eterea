#!/usr/bin/env bash
# Run the cargo-fuzz targets against the pure URL policy and the import parsers.
#
#   scripts/verify/fuzz.sh                 # 10 s per target
#   FUZZ_TIME=30 scripts/verify/fuzz.sh    # longer local run
#   scripts/verify/fuzz.sh url_policy      # one target
#
# All state stays under the workspace: .tools/target/fuzz, .tools/fuzz-corpora,
# .tools/cache and fuzz/artifacts. The committed seeds live in fuzz/corpus/.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
# shellcheck source=versions.env
source "${SCRIPT_DIR}/versions.env"

TOOLS="${ROOT}/.tools"
FUZZ_BIN="${TOOLS}/cargo-fuzz/bin/cargo-fuzz"
NIGHTLY_BIN="${TOOLS}/rustup/toolchains/${MIRI_NIGHTLY}-x86_64-unknown-linux-gnu/bin"

if [ ! -x "${FUZZ_BIN}" ]; then
    echo "fuzz: cargo-fuzz is not installed under ${TOOLS}" >&2
    echo "fuzz: install it with: bash scripts/verify/install.sh fuzz" >&2
    exit 4
fi
if [ ! -x "${NIGHTLY_BIN}/cargo" ]; then
    echo "fuzz: pinned nightly is not installed under ${TOOLS}" >&2
    exit 4
fi
if [ ! -f "${ROOT}/fuzz/Cargo.toml" ]; then
    echo "fuzz: fuzz targets missing at ${ROOT}/fuzz" >&2
    exit 2
fi

export RUSTUP_HOME="${TOOLS}/rustup"
export CARGO_HOME="${ROOT}/.work/cargo-home"
export CARGO_TARGET_DIR="${TOOLS}/target/fuzz"
export XDG_CACHE_HOME="${TOOLS}/cache"
export TMPDIR="${TOOLS}/tmp"
export RUSTUP_TOOLCHAIN="${MIRI_NIGHTLY}"
export PATH="${NIGHTLY_BIN}:${TOOLS}/cargo-fuzz/bin:${PATH}"
mkdir -p "${CARGO_TARGET_DIR}" "${XDG_CACHE_HOME}" "${TMPDIR}" "${ROOT}/fuzz/artifacts"

FUZZ_TIME="${FUZZ_TIME:-10}"
TARGETS=("$@")
if [ ${#TARGETS[@]} -eq 0 ]; then
    TARGETS=(url_policy import_text)
fi

cd "${ROOT}"
failed=0
for target in "${TARGETS[@]}"; do
    seeds="${ROOT}/fuzz/corpus/${target}"
    corpus="${TOOLS}/fuzz-corpus/${target}"
    if [ ! -d "${seeds}" ]; then
        echo "fuzz: no seed corpus for ${target}" >&2
        failed=1
        continue
    fi
    mkdir -p "${corpus}"
    cp -n "${seeds}"/* "${corpus}/" 2>/dev/null || true
    echo "fuzz: ${target} for ${FUZZ_TIME}s (working corpus: ${corpus})"
    if timeout $((FUZZ_TIME + 120)) "${FUZZ_BIN}" run "${target}" "${corpus}" -- \
        -max_total_time="${FUZZ_TIME}" -rss_limit_mb=4096 -print_final_stats=1; then
        echo "fuzz: ${target} PASS"
    else
        echo "fuzz: ${target} FAIL" >&2
        failed=1
    fi
done
exit "${failed}"
