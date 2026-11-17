#!/usr/bin/env bash
# Install the pinned eterea verification toolchain into <repo>/.tools.
#
# Guarantees:
#   * every write stays inside the eterea workspace (default $repo/.tools);
#   * no sudo, no system package manager, no global rustup/cargo state;
#   * failures are recorded per step and do not stop the remaining steps.
#
# Usage:
#   scripts/verify/install.sh                 # install verus kani miri fuzz
#   scripts/verify/install.sh verus kani      # install a subset
#   scripts/verify/install.sh --force kani    # redownload / reinstall
#   DOWNLOAD_TIMEOUT=120 scripts/verify/install.sh   # shorter network budget
#
# Layout after a successful run:
#   .tools/verus/             Verus release (VERUS_VERSION)
#   .tools/kani/              Kani release (KANI_VERSION); kani-driver is
#                             symlinked to bin/cargo-kani so `cargo kani` works
#   .tools/rustup/            RUSTUP_HOME for the Verus and Miri toolchains
#   .tools/cargo-home-rustup  CARGO_HOME used only by rustup; its bin/ also
#                             holds the rustup shim used by the Verus wrapper
#   .tools/cargo-fuzz/        cargo-fuzz install root (bin/cargo-fuzz)
#   .tools/downloads/         cached release archives (sha256-verified)
#   .tools/logs/              per-step logs and install-status.txt
#
# Notes discovered while pinning these versions (2026-11-17):
#   * Verus 0.2026.11.02.8bf68ee is a rustup-managed release: the `verus`
#     wrapper needs `rustup` on PATH and the toolchain in VERUS_TOOLCHAIN (see
#     version.json in the release). install.sh installs both into the
#     workspace RUSTUP_HOME/CARGO_HOME.
#   * Kani 0.68.0's tarball ships `bin/kani-driver` but not `bin/cargo-kani`.
#     kani-driver switches to cargo-plugin mode when invoked as `cargo-kani`,
#     so install.sh creates that symlink. `cargo kani` then works after
#     `PATH="$repo/.tools/kani/bin:$PATH"`.
#
# Use scripts/verify/all.sh to run the checks. If the network is unavailable,
# this stops after DOWNLOAD_TIMEOUT/BUILD_TIMEOUT seconds per artifact and
# records exactly which command failed; the scripts remain runnable on a
# machine that can download later.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
# shellcheck source=versions.env
source "${SCRIPT_DIR}/versions.env"

TOOLS="${ROOT}/.tools"
DOWNLOADS="${TOOLS}/downloads"
LOGS="${TOOLS}/logs"
TMP="${TOOLS}/tmp"
STATUS_FILE="${LOGS}/install-status.txt"
RUSTUP_HOME="${TOOLS}/rustup"
RUSTUP_CARGO_HOME="${TOOLS}/cargo-home-rustup"
RUSTUP_BIN="${RUSTUP_CARGO_HOME}/bin/rustup"

FORCE=0
REQUESTED=()
for arg in "$@"; do
    case "${arg}" in
        --force) FORCE=1 ;;
        -h|--help) sed -n '2,50p' "$0"; exit 0 ;;
        verus|kani|miri|fuzz) REQUESTED+=("${arg}") ;;
        *) echo "unknown argument: ${arg}" >&2; exit 2 ;;
    esac
done
if [ "${#REQUESTED[@]}" -eq 0 ]; then
    REQUESTED=(verus kani miri fuzz)
fi

mkdir -p "${TOOLS}" "${DOWNLOADS}" "${LOGS}" "${TMP}" "${RUSTUP_HOME}" "${RUSTUP_CARGO_HOME}"
: > "${STATUS_FILE}.new"

say()  { printf '%s\n' "$*"; }
note() { printf '  %s\n' "$*"; }
record() { # record <tool> <status> <detail>
    printf '%-6s %-9s %s\n' "$1" "$2" "$3" >> "${STATUS_FILE}.new"
}
have() { command -v "$1" >/dev/null 2>&1; }

if [ "$(uname -s)" != "Linux" ] || [ "$(uname -m)" != "x86_64" ]; then
    say "unsupported host: $(uname -s)/$(uname -m); the pins in versions.env are x86_64 Linux only" >&2
    exit 2
fi
if ! have curl || ! have sha256sum || ! have tar || ! have unzip; then
    say "install.sh needs curl, sha256sum, tar and unzip on PATH" >&2
    exit 2
fi

# download <url> <dest> <sha256> <logfile>
# Skips a file whose sha256 already matches. A partial file is never reused:
# transfer goes to <dest>.part and is removed on failure.
download() {
    local url="$1" dest="$2" sha="$3" logfile="$4"
    if [ -f "${dest}" ] && [ "${FORCE}" -eq 0 ]; then
        if echo "${sha}  ${dest}" | sha256sum -c - >/dev/null 2>&1; then
            note "cached $(basename "${dest}")"
            return 0
        fi
        note "checksum mismatch, refetching $(basename "${dest}")"
        rm -f "${dest}"
    fi
    rm -f "${dest}.part"
    note "GET ${url} (timeout ${DOWNLOAD_TIMEOUT}s)"
    curl --location --fail --show-error \
        --connect-timeout "${CONNECT_TIMEOUT}" \
        --max-time "${DOWNLOAD_TIMEOUT}" \
        --retry 2 --retry-delay 3 \
        --output "${dest}.part" "${url}" >>"${logfile}" 2>&1
    local code=$?
    if [ "${code}" -ne 0 ]; then
        rm -f "${dest}.part"
        return "${code}"
    fi
    echo "${sha}  ${dest}.part" | sha256sum -c - >>"${logfile}" 2>&1 || {
        note "sha256 mismatch for ${url}; refusing to install"
        rm -f "${dest}.part"
        return 3
    }
    mv "${dest}.part" "${dest}"
}

# extract_single_dir <archive-kind> <archive> <target-dir> <logfile>
# Extracts into a scratch dir and renames the single top-level directory to
# <target-dir>, so scripts get a stable path.
extract_single_dir() {
    local kind="$1" archive="$2" target="$3" logfile="$4"
    local scratch="${TMP}/extract-$(basename "${archive}")"
    rm -rf "${scratch}" "${target}.new"
    mkdir -p "${scratch}"
    case "${kind}" in
        zip) unzip -q "${archive}" -d "${scratch}" >>"${logfile}" 2>&1 ;;
        tgz) tar --no-same-owner -xzf "${archive}" -C "${scratch}" >>"${logfile}" 2>&1 ;;
        *) note "unknown archive kind ${kind}"; return 2 ;;
    esac
    local entries=()
    while IFS= read -r entry; do entries+=("${entry}"); done < <(find "${scratch}" -mindepth 1 -maxdepth 1)
    if [ "${#entries[@]}" -ne 1 ]; then
        note "expected one top-level directory in ${archive}, found ${#entries[@]}"
        return 2
    fi
    mv "${entries[0]}" "${target}.new"
    rm -rf "${target}"
    mv "${target}.new" "${target}"
    rm -rf "${scratch}"
}

# rustup_env <command...>: run a command with rustup state inside the workspace.
rustup_env() {
    RUSTUP_HOME="${RUSTUP_HOME}" CARGO_HOME="${RUSTUP_CARGO_HOME}" \
        PATH="${RUSTUP_CARGO_HOME}/bin:${PATH}" "$@"
}

# ensure_rustup <logfile>: install rustup 1.29.3 under .tools if missing.
ensure_rustup() {
    local logfile="$1"
    if [ -x "${RUSTUP_BIN}" ] && [ "${FORCE}" -eq 0 ]; then
        return 0
    fi
    local bin="${TOOLS}/rustup-init"
    download "${RUSTUP_INIT_URL}" "${bin}" "${RUSTUP_INIT_SHA256}" "${logfile}" || return 1
    chmod +x "${bin}"
    RUSTUP_HOME="${RUSTUP_HOME}" CARGO_HOME="${RUSTUP_CARGO_HOME}" \
        timeout "${BUILD_TIMEOUT:-300}" "${bin}" -y --no-modify-path --profile minimal \
        --default-toolchain none >>"${logfile}" 2>&1
    if [ ! -x "${RUSTUP_BIN}" ]; then
        # Older rustup puts the binary in RUSTUP_HOME/bin instead of CARGO_HOME/bin.
        if [ -x "${RUSTUP_HOME}/bin/rustup" ]; then
            RUSTUP_BIN="${RUSTUP_HOME}/bin/rustup"
        else
            note "rustup binary missing after rustup-init"
            return 1
        fi
    fi
}

install_verus() {
    local log="${LOGS}/install-verus.log"
    : > "${log}"
    say "[verus] ${VERUS_VERSION} (toolchain ${VERUS_TOOLCHAIN})"
    if ! ensure_rustup "${log}"; then
        record verus FAIL "ensure_rustup"
        return 1
    fi
    if ! rustup_env timeout "${BUILD_TIMEOUT:-300}" "${RUSTUP_BIN}" toolchain install \
            "${VERUS_TOOLCHAIN}" --profile minimal >>"${log}" 2>&1; then
        record verus FAIL "rustup toolchain install ${VERUS_TOOLCHAIN}"
        return 1
    fi
    if [ "${FORCE}" -eq 0 ] && [ -x "${TOOLS}/verus/verus" ]; then
        note "already extracted"
    else
        local zip="${DOWNLOADS}/${VERUS_ASSET}"
        download "${VERUS_URL}" "${zip}" "${VERUS_SHA256}" "${log}" || { record verus FAIL "download: ${VERUS_URL}"; return 1; }
        extract_single_dir zip "${zip}" "${TOOLS}/verus" "${log}" || { record verus FAIL "extract: ${zip}"; return 1; }
    fi
    if [ ! -x "${TOOLS}/verus/verus" ]; then
        record verus FAIL "no executable at .tools/verus/verus after extract"
        return 1
    fi
    local version
    version="$(cd "${TOOLS}/verus" && RUSTUP_TOOLCHAIN="${VERUS_TOOLCHAIN}" \
        RUSTUP_HOME="${RUSTUP_HOME}" CARGO_HOME="${RUSTUP_CARGO_HOME}" \
        PATH="${RUSTUP_CARGO_HOME}/bin:${PATH}" ./verus --version 2>&1 | head -2 | tr '\n' ' ')"
    case "${version}" in
        *"${VERUS_VERSION}"*) ;;
        *) record verus FAIL "verus --version did not report ${VERUS_VERSION}: ${version}"; return 1 ;;
    esac
    note "installed: ${version}"
    record verus OK "verus ${VERUS_VERSION}"
}

install_kani() {
    local log="${LOGS}/install-kani.log"
    : > "${log}"
    say "[kani] ${KANI_VERSION}"
    if [ "${FORCE}" -eq 0 ] && [ -x "${TOOLS}/kani/bin/kani-driver" ]; then
        note "already extracted"
    else
        local tgz="${DOWNLOADS}/${KANI_ASSET}"
        download "${KANI_URL}" "${tgz}" "${KANI_SHA256}" "${log}" || { record kani FAIL "download: ${KANI_URL}"; return 1; }
        extract_single_dir tgz "${tgz}" "${TOOLS}/kani" "${log}" || { record kani FAIL "extract: ${tgz}"; return 1; }
    fi
    # Kani 0.68.0 ships kani-driver; the cargo plugin name is the dispatch key.
    if [ ! -e "${TOOLS}/kani/bin/cargo-kani" ]; then
        ln -sfn kani-driver "${TOOLS}/kani/bin/cargo-kani"
    fi
    if [ ! -x "${TOOLS}/kani/bin/cargo-kani" ]; then
        record kani FAIL "no executable at .tools/kani/bin/cargo-kani"
        return 1
    fi
    # The release tarball omits the Rust toolchain on purpose: the official
    # install flow (`cargo install kani-verifier` + `cargo kani setup`) puts a
    # nightly under <bundle>/toolchain. Do the equivalent non-interactively.
    if ! ensure_rustup "${log}"; then
        record kani FAIL "ensure_rustup"
        return 1
    fi
    local bundle_toolchain
    bundle_toolchain="$(cat "${TOOLS}/kani/rust-toolchain-version" 2>/dev/null || true)"
    if [ "${bundle_toolchain}" != "${KANI_TOOLCHAIN}" ]; then
        record kani FAIL "bundle expects toolchain '${bundle_toolchain}', versions.env pins '${KANI_TOOLCHAIN}'"
        return 1
    fi
    if [ "${FORCE}" -eq 0 ] && [ -x "${TOOLS}/kani/toolchain/bin/cargo" ]; then
        note "toolchain already linked"
    else
        rustup_env timeout "${BUILD_TIMEOUT:-300}" "${RUSTUP_BIN}" toolchain install \
            "${KANI_TOOLCHAIN_NIGHTLY}" --profile minimal >>"${log}" 2>&1 \
            || { record kani FAIL "rustup toolchain install ${KANI_TOOLCHAIN_NIGHTLY}"; return 1; }
        ln -sfn "../rustup/toolchains/${KANI_TOOLCHAIN}" "${TOOLS}/kani/toolchain"
    fi
    if [ ! -x "${TOOLS}/kani/toolchain/bin/cargo" ]; then
        record kani FAIL "no <bundle>/toolchain/bin/cargo after rustup toolchain install"
        return 1
    fi
    local version
    version="$("${TOOLS}/kani/bin/cargo-kani" --version 2>&1 | head -1)"
    case "${version}" in
        *"${KANI_VERSION}"*) ;;
        *) record kani FAIL "cargo-kani --version did not report ${KANI_VERSION}: ${version}"; return 1 ;;
    esac
    note "installed: ${version} (toolchain ${KANI_TOOLCHAIN})"
    record kani OK "${version} (toolchain ${KANI_TOOLCHAIN})"
}

install_miri() {
    local log="${LOGS}/install-miri.log"
    : > "${log}"
    say "[miri] ${MIRI_NIGHTLY} (${MIRI_RUSTC_VERSION})"
    if ! ensure_rustup "${log}"; then
        record miri FAIL "ensure_rustup"
        return 1
    fi
    local miri_cargo="${RUSTUP_HOME}/toolchains/${MIRI_NIGHTLY}-x86_64-unknown-linux-gnu/bin/cargo-miri"
    if [ "${FORCE}" -eq 0 ] && [ -x "${miri_cargo}" ]; then
        note "already installed"
    else
        rustup_env timeout "${BUILD_TIMEOUT:-300}" "${RUSTUP_BIN}" toolchain install "${MIRI_NIGHTLY}" \
            --profile minimal --component "${MIRI_COMPONENT}" --component rust-src \
            >>"${log}" 2>&1 || { record miri FAIL "rustup toolchain install ${MIRI_NIGHTLY}"; return 1; }
    fi
    if [ ! -x "${miri_cargo}" ]; then
        record miri FAIL "cargo-miri missing after install"
        return 1
    fi
    local version
    version="$(RUSTUP_TOOLCHAIN="${MIRI_NIGHTLY}" RUSTUP_HOME="${RUSTUP_HOME}" \
        CARGO_HOME="${RUSTUP_CARGO_HOME}" PATH="${RUSTUP_CARGO_HOME}/bin:${PATH}" \
        "${RUSTUP_BIN}" run "${MIRI_NIGHTLY}" miri --version 2>&1 | head -1)"
    note "installed: miri ${version}"
    record miri OK "miri ${version} (${MIRI_NIGHTLY})"
}

install_fuzz() {
    local log="${LOGS}/install-fuzz.log"
    : > "${log}"
    say "[fuzz] cargo-fuzz ${CARGO_FUZZ_VERSION}"
    if [ "${FORCE}" -eq 0 ] && [ -x "${TOOLS}/cargo-fuzz/bin/cargo-fuzz" ]; then
        note "already installed: $("${TOOLS}/cargo-fuzz/bin/cargo-fuzz" --version 2>&1 | head -1)"
        record fuzz already "${TOOLS}/cargo-fuzz"
        return 0
    fi
    if ! have cargo; then
        record fuzz FAIL "cargo not on PATH"
        return 1
    fi
    mkdir -p "${TOOLS}/cargo-home" "${TOOLS}/target"
    CARGO_HOME="${TOOLS}/cargo-home" \
    CARGO_TARGET_DIR="${TOOLS}/target/cargo-fuzz" \
        timeout "${BUILD_TIMEOUT:-300}" cargo install cargo-fuzz \
        --version "${CARGO_FUZZ_VERSION}" --locked --root "${TOOLS}/cargo-fuzz" \
        >>"${log}" 2>&1 || { record fuzz FAIL "cargo install cargo-fuzz --version ${CARGO_FUZZ_VERSION}"; return 1; }
    local version
    version="$("${TOOLS}/cargo-fuzz/bin/cargo-fuzz" --version 2>&1 | head -1)"
    note "installed: ${version}"
    record fuzz OK "${version}"
}

failures=0
for tool in "${REQUESTED[@]}"; do
    case "${tool}" in
        verus) install_verus ;;
        kani)  install_kani ;;
        miri)  install_miri ;;
        fuzz)  install_fuzz ;;
    esac
    case "${tool}" in
        verus) [ -x "${TOOLS}/verus/verus" ] || failures=$((failures + 1)) ;;
        kani)  [ -x "${TOOLS}/kani/bin/cargo-kani" ] || failures=$((failures + 1)) ;;
        miri)  [ -x "${RUSTUP_HOME}/toolchains/${MIRI_NIGHTLY}-x86_64-unknown-linux-gnu/bin/cargo-miri" ] || failures=$((failures + 1)) ;;
        fuzz)  [ -x "${TOOLS}/cargo-fuzz/bin/cargo-fuzz" ] || failures=$((failures + 1)) ;;
    esac
done

# Reproducibility note: the tool dirs are disposable; downloads are checked
# against versions.env sha256 pins. Do not commit .tools (it is large binary
# tooling); the script keeps a .gitignore inside .tools so a stray `git add .`
# does not stage 200+ MB of binaries.
if [ ! -f "${TOOLS}/.gitignore" ]; then
    printf '*\n!.gitignore\n' > "${TOOLS}/.gitignore"
fi

mv "${STATUS_FILE}.new" "${STATUS_FILE}"
say ""
say "tool status (${STATUS_FILE}):"
cat "${STATUS_FILE}"
say ""
say "logs: ${LOGS}/install-*.log"
if [ "${failures}" -ne 0 ]; then
    say "FAILED: ${failures} tool(s) did not install; see the logs above." >&2
    say "Each log ends with the exact failing command. scripts/verify/all.sh still runs whatever is installed." >&2
    exit 1
fi
say "all requested tools installed under ${TOOLS}"
