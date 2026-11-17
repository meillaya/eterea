#!/usr/bin/env bash
# Package eterea as a runnable, self-contained Linux x86_64 bundle.
#
# Why this exists: `target/release/eterea` built in the devenv shell runs only
# inside that shell. winit dlopens Wayland/X11/xkbcommon/fontconfig libraries by
# soname, so a file-manager launch (no LD_LIBRARY_PATH) fails with
# "The wayland library could not be loaded". This script collects the runtime
# libraries the binary needs into <bundle>/lib, rewrites the ELF RUNPATH to
# $ORIGIN/lib, and emits a desktop entry, hicolor icons, provenance and
# checksums.
#
# Run it INSIDE the devenv shell (that is where the toolchain and the library
# closure come from):
#
#   devenv shell -- bash scripts/package-linux-release.sh
#   devenv shell -- bash scripts/package-linux-release.sh --skip-build
#
# Options:
#   --skip-build        use the existing target/release/eterea (refused when it
#                       is older than the desktop sources; see --allow-stale)
#   --allow-stale       mechanics/test escape hatch: accept a --skip-build
#                       binary older than the sources; marks PROVENANCE.md
#   --bundle-glibc      also bundle glibc + the dynamic loader (default: NO;
#                       the bundle uses the host glibc, like AppImage)
#   --bundle-gpu        also bundle libvulkan/libGL/libEGL (default: NO; the
#                       host desktop's GPU stack is used, so Mesa/ICD and the
#                       host glibc always match)
#   --interpreter PATH  ELF interpreter to write into eterea
#                       (default /lib64/ld-linux-x86-64.so.2)
#   --dist DIR          output directory (default: <repo>/dist)
#   --bundle-name NAME  override the eterea-<version>-linux-x86_64 directory name
#   --no-tar            do not create the .tar.gz
#   -h, --help          show this help
#
# Layout produced under <dist>/<bundle-name>/:
#   eterea                    ELF binary, RUNPATH=$ORIGIN/lib (direct launch)
#   eterea-launcher           POSIX sh: runs the binary through the bundled
#                             dynamic loader, for hosts without the build
#                             environment's loader path (portable launch)
#   lib/                      collected runtime libraries + ld-linux-x86-64.so.2
#   share/applications/       eterea.desktop
#   share/icons/hicolor/      eterea.png at each size + scalable SVG
#   PROVENANCE.md             build metadata (no personal paths or URLs)
#   SHA256SUMS                checksum of every file in the bundle
# plus <bundle-name>.tar.gz and its .sha256 next to the directory.
#
# Notes:
#   * The bundle intentionally carries glibc + the dynamic loader so that
#     `eterea-launcher` works on non-Nix distributions. `eterea` itself keeps
#     the build interpreter (Nix store path); it is the artifact that runs on
#     the build host and in environments where that store path exists.
#   * No launcher fallback is needed for the direct ELF: RUNPATH=$ORIGIN/lib
#     fixes the winit dlopen failure. The launcher only exists because an ELF
#     interpreter path cannot be made relative; it is documented, optional, and
#     not used by the desktop entry.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

DIST="${ROOT}/dist"
BUNDLE_NAME=""
SKIP_BUILD=0
ALLOW_STALE=0
BUNDLE_GLIBC=0
BUNDLE_GPU=0
MAKE_TAR=1
INTERPRETER="/lib64/ld-linux-x86-64.so.2"

usage() {
    sed -n '2,50p' "${BASH_SOURCE[0]}"
}

die() {
    printf 'package-linux-release: error: %s\n' "$*" >&2
    exit 1
}

log() {
    printf 'package-linux-release: %s\n' "$*"
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --skip-build) SKIP_BUILD=1 ;;
        --allow-stale) ALLOW_STALE=1 ;;
        --bundle-glibc) BUNDLE_GLIBC=1 ;;
        --bundle-gpu) BUNDLE_GPU=1 ;;
        --interpreter)
            [ "$#" -ge 2 ] || die "--interpreter requires a path"
            INTERPRETER="$2"
            shift
            ;;
        --dist)
            [ "$#" -ge 2 ] || die "--dist requires a path"
            DIST="$2"
            shift
            ;;
        --bundle-name)
            [ "$#" -ge 2 ] || die "--bundle-name requires a value"
            BUNDLE_NAME="$2"
            shift
            ;;
        --no-tar) MAKE_TAR=0 ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            die "unknown argument: $1 (try --help)"
            ;;
    esac
    shift
done

[ -f "${ROOT}/Cargo.toml" ] || die "not in the eterea repository: ${ROOT}"
[ -n "${LD_LIBRARY_PATH:-}" ] || die \
    "LD_LIBRARY_PATH is empty: run this inside 'devenv shell -- bash scripts/package-linux-release.sh'"
command -v patchelf >/dev/null 2>&1 || die "patchelf not found: run inside devev shell"
command -v readelf >/dev/null 2>&1 || die "readelf not found: run inside devenv shell"

VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "${ROOT}/src/desktop/Cargo.toml" | head -1)"
[ -n "${VERSION}" ] || die "cannot read eterea-desktop version from src/desktop/Cargo.toml"
if [ -z "${BUNDLE_NAME}" ]; then
    BUNDLE_NAME="eterea-${VERSION}-linux-x86_64"
fi

TARGET_DIR="${CARGO_TARGET_DIR:-${ROOT}/target}"
BIN="${TARGET_DIR}/release/eterea"
BUNDLE="${DIST}/${BUNDLE_NAME}"

if [ "${SKIP_BUILD}" -eq 0 ]; then
    log "building release binary (cargo build -p eterea-desktop --release)"
    if ! ( cd "${ROOT}" && cargo build -p eterea-desktop --release ); then
        die "release build failed; refusing to package (no fallback to an existing binary)
       fix the build, or run a mechanics test explicitly with --skip-build --allow-stale"
    fi
else
    log "WARNING: --skip-build: using the existing ${BIN}"
    log "WARNING: release packaging must build from current sources (this mode is for tests only)"
fi
[ -x "${BIN}" ] || die "release binary missing: ${BIN} (build it or drop --skip-build)"

# A --skip-build binary older than the desktop sources is almost always a stale
# integration artifact; refuse it unless the caller explicitly allows it.
STALE_REASON=""
for input in "${ROOT}/src/desktop/Cargo.toml" "${ROOT}/Cargo.toml" "${ROOT}/Cargo.lock"; do
    if [ -f "${input}" ] && [ "${input}" -nt "${BIN}" ]; then
        STALE_REASON="${input}"
        break
    fi
done
if [ -z "${STALE_REASON}" ]; then
    newer_src="$(find "${ROOT}/src/desktop/src" "${ROOT}/src/app/src" "${ROOT}/src/backend/src" \
        "${ROOT}/src/media/src" "${ROOT}/src/logic/src" "${ROOT}/src/ui-model/src" \
        -name '*.rs' -newer "${BIN}" -print -quit 2>/dev/null || true)"
    STALE_REASON="${newer_src}"
fi
if [ -n "${STALE_REASON}" ]; then
    if [ "${SKIP_BUILD}" -eq 0 ]; then
        # A fresh build that is still older than a source file only happens with
        # clock skew; fail loudly instead of shipping it.
        die "binary ${BIN} is older than ${STALE_REASON} immediately after a build"
    fi
    if [ "${ALLOW_STALE}" -eq 0 ]; then
        die "refusing to package a stale binary: ${BIN} is older than ${STALE_REASON}
       run without --skip-build to build from current sources, or pass --allow-stale
       for a deliberate mechanics test (PROVENANCE.md will say the binary is stale)"
    fi
    log "WARNING: packaging a binary older than ${STALE_REASON} (--allow-stale mechanics test)"
    STALE_NOTE="yes (--allow-stale mechanics test; not a shippable artifact)"
else
    STALE_NOTE="no"
fi

log "assembling ${BUNDLE}"
rm -rf "${BUNDLE}"
mkdir -p "${BUNDLE}/lib" \
    "${BUNDLE}/share/applications" \
    "${BUNDLE}/share/icons"

# --- library search roots ---------------------------------------------------
# Order matters: the active devenv environment first, then the resolved
# development-shell closure (which contains the X11/xcb stack winit dlopens but
# devenv does not put on LD_LIBRARY_PATH for a Wayland session).
roots=()
IFS=':' read -ra ld_dirs <<< "${LD_LIBRARY_PATH}"
for dir in "${ld_dirs[@]}"; do
    if [ -n "${dir}" ] && [ -d "${dir}" ]; then
        roots+=("${dir}")
    fi
done

PROFILE_LINK="${ROOT}/.devenv/profile"
if [ -L "${PROFILE_LINK}" ] && command -v nix-store >/dev/null 2>&1; then
    profile_path="$(readlink -f "${PROFILE_LINK}")"
    while IFS= read -r store_path; do
        if [ -n "${store_path}" ] && [ -d "${store_path}/lib" ]; then
            roots+=("${store_path}/lib")
        fi
    done < <(nix-store -qR "${profile_path}" 2>/dev/null || true)
fi

# Keep the first occurrence of each root.
declare -A seen_root=()
unique_roots=()
for root in "${roots[@]}"; do
    if [ -z "${seen_root[${root}]:-}" ]; then
        seen_root["${root}"]=1
        unique_roots+=("${root}")
    fi
done
log "library search roots: ${#unique_roots[@]}"

find_lib() {
    local name="$1" root
    for root in "${unique_roots[@]}"; do
        if [ -e "${root}/${name}" ]; then
            printf '%s\n' "${root}/${name}"
            return 0
        fi
    done
    return 1
}

# --- collect libraries ------------------------------------------------------
# copy_lib copies a library and, recursively, the NEEDED libraries ldd resolves
# for it. Libraries are copied under their soname (the name dlopen/ld.so uses).
declare -A copied=()
# Host-provided libraries: do not ship glibc/loader unless --bundle-glibc.
GLIBC_LIBS=(libc.so.6 libm.so.6 libgcc_s.so.1 libdl.so.2 libpthread.so.0
    librt.so.1 libresolv.so.2 libutil.so.1 libnsl.so.1 libanl.so.1
    libmvec.so.1 libBrokenLocale.so.1)
copy_lib() {
    local src="$1" real name
    [ -e "${src}" ] || return 0
    real="$(readlink -f "${src}")"
    [ -f "${real}" ] || return 0
    name="$(basename "${src}")"
    if [ "${BUNDLE_GLIBC}" -eq 0 ]; then
        local glibc_lib
        for glibc_lib in "${GLIBC_LIBS[@]}"; do
            if [ "${name}" = "${glibc_lib}" ]; then
                return 0
            fi
        done
        case "${name}" in
            ld-linux*.so*|ld-*.so*) return 0 ;;
        esac
    fi
    if [ -n "${copied[${name}]:-}" ]; then
        return 0
    fi
    copied["${name}"]=1
    cp -L "${real}" "${BUNDLE}/lib/${name}"
    chmod 0644 "${BUNDLE}/lib/${name}"
    local line dep_path
    while IFS= read -r line; do
        case "${line}" in
            *" => /"*)
                dep_path="${line#* => }"
                dep_path="${dep_path%% (*}"
                copy_lib "${dep_path}"
                ;;
        esac
    done < <(ldd "${real}" 2>/dev/null || true)
}

log "collecting NEEDED libraries"
while IFS= read -r line; do
    case "${line}" in
        *" => /"*)
            dep_path="${line#* => }"
            dep_path="${dep_path%% (*}"
            copy_lib "${dep_path}"
            ;;
    esac
done < <(ldd "${BIN}")

# Libraries winit/wgpu/fontconfig dlopen by soname at runtime: these never show
# up in ldd because the binary does not link them at load time.
required_dlopen_libs=(
    libwayland-client.so.0 libwayland-cursor.so.0 libwayland-egl.so.1
    libxkbcommon.so.0 libxkbcommon-x11.so.0
    libX11.so.6 libX11-xcb.so.1 libXcursor.so.1 libXrandr.so.2
    libXi.so.6 libXinerama.so.1
    libfontconfig.so.1 libfreetype.so.6 libexpat.so.1 libz.so.1
    libxcb.so.1
)
optional_dlopen_libs=()
if [ "${BUNDLE_GPU}" -eq 1 ]; then
    optional_dlopen_libs=(libvulkan.so.1 libGL.so.1 libEGL.so.1)
fi

log "collecting dlopen() libraries"
missing_required=()
for name in "${required_dlopen_libs[@]}"; do
    if found="$(find_lib "${name}")"; then
        copy_lib "${found}"
    else
        missing_required+=("${name}")
    fi
done
if [ "${#missing_required[@]}" -gt 0 ]; then
    die "missing required runtime libraries in the devenv closure: ${missing_required[*]}
       (the bundle would fail to start; run inside 'devenv shell' and check desktop libs)"
fi
for name in "${optional_dlopen_libs[@]}"; do
    if found="$(find_lib "${name}")"; then
        copy_lib "${found}"
    else
        log "warning: optional library not found: ${name}"
    fi
done
# Every libxcb extension available in the closure: x11rb/xkbcommon-x11 dlopen
# them by soname and the set differs per backend path.
declare -A seen_xcb=()
for root in "${unique_roots[@]}"; do
    for candidate in "${root}"/libxcb-*.so.*; do
        [ -e "${candidate}" ] || continue
        name="$(basename "${candidate}")"
        # Keep sonames (libxcb-foo.so.N); skip versioned real files (.so.N.M.K)
        # so the same library is not bundled twice under two names.
        case "${name}" in
            libxcb-*.so.*.*) continue ;;
        esac
        if [ -z "${seen_xcb[${name}]:-}" ]; then
            seen_xcb["${name}"]=1
            copy_lib "${candidate}"
        fi
    done
done

# Portable bundles use the host glibc and do not ship a loader by default. The
# launcher then runs the binary through a host loader; --bundle-glibc copies the
# build interpreter for environments that want a fully self-contained glibc.
BUILD_INTERP="$(readelf -l "${BIN}" | sed -n 's/.*program interpreter: \(.*\)]/\1/p')"
[ -n "${BUILD_INTERP}" ] || die "cannot read the ELF interpreter of ${BIN}"
if [ "${BUNDLE_GLIBC}" -eq 1 ]; then
    [ -e "${BUILD_INTERP}" ] || die "ELF interpreter does not exist: ${BUILD_INTERP}"
    cp -L "${BUILD_INTERP}" "${BUNDLE}/lib/ld-linux-x86-64.so.2"
    chmod 0755 "${BUNDLE}/lib/ld-linux-x86-64.so.2"
    LAUNCHER_LOADER="bundled"
else
    LAUNCHER_LOADER="host"
fi

log "bundled libraries: $(find "${BUNDLE}/lib" -maxdepth 1 -type f -name '*.so*' | wc -l)"

# --- patch ELF metadata -----------------------------------------------------
log "setting RUNPATH=\$ORIGIN/lib on eterea, \$ORIGIN on bundled libraries"
cp "${BIN}" "${BUNDLE}/eterea"
chmod 0755 "${BUNDLE}/eterea"
patchelf --set-interpreter "${INTERPRETER}" --set-rpath '$ORIGIN/lib' "${BUNDLE}/eterea"
for lib in "${BUNDLE}"/lib/*.so*; do
    [ -e "${lib}" ] || continue
    case "$(basename "${lib}")" in
        ld-*.so*)
            # glibc's dynamic loader must not be patched: patchelf on
            # ld-linux-x86-64.so.2 corrupts it and the loader segfaults.
            log "keeping bundled loader unpatched: $(basename "${lib}")"
            continue
            ;;
    esac
    patchelf --set-rpath '$ORIGIN' "${lib}" 2>/dev/null || \
        log "warning: could not rewrite RUNPATH of $(basename "${lib}")"
done

# --- portable launcher ------------------------------------------------------
cat > "${BUNDLE}/eterea-launcher" <<'LAUNCHER'
#!/bin/sh
# Portable launcher: runs eterea with the host glibc and the libraries in lib/.
# It prefers a host dynamic loader so host Mesa/Vulkan drivers load against the
# same glibc the desktop session uses. With --bundle-glibc the build loader in
# lib/ is the fallback.
set -eu
self="$0"
case "${self}" in
    */*) ;;
    *) self="$(command -v -- "${self}")" ;;
esac
here="$(CDPATH= cd -- "$(dirname -- "${self}")" && pwd)"
for loader in /lib64/ld-linux-x86-64.so.2 /lib/ld-linux-x86-64.so.2 \
    /usr/lib/ld-linux-x86-64.so.2 /usr/lib64/ld-linux-x86-64.so.2; do
    if [ -e "${loader}" ]; then
        exec "${loader}" --library-path "${here}/lib" "${here}/eterea" "$@"
    fi
done
if [ -e "${here}/lib/ld-linux-x86-64.so.2" ]; then
    exec "${here}/lib/ld-linux-x86-64.so.2" --library-path "${here}/lib" "${here}/eterea" "$@"
fi
echo "eterea-launcher: no dynamic loader found (host or bundled)" >&2
exit 1
LAUNCHER
chmod 0755 "${BUNDLE}/eterea-launcher"

# --- desktop entry and icons ------------------------------------------------
install -Dm644 "${ROOT}/assets/linux/eterea.desktop" \
    "${BUNDLE}/share/applications/eterea.desktop"

BRAND="${ROOT}/docs/assets/brand"
if [ -d "${BRAND}" ]; then
    # Final brand assets: icon-<size>.png for every hicolor size, plus the
    # scalable eterea-icon.svg. Discover the sizes instead of hard-coding them
    # so a new export cannot silently leave a stale hicolor layout.
    icon_count=0
    for icon in "${BRAND}"/icon-*.png; do
        [ -f "${icon}" ] || continue
        size="$(basename "${icon}")"
        size="${size#icon-}"
        size="${size%.png}"
        case "${size}" in
            ''|*[!0-9]*) continue ;;
        esac
        install -Dm644 "${icon}" \
            "${BUNDLE}/share/icons/hicolor/${size}x${size}/apps/eterea.png"
        icon_count=$((icon_count + 1))
    done
    if [ "${icon_count}" -eq 0 ]; then
        log "warning: no docs/assets/brand/icon-<size>.png assets found; bundle has no PNG icons"
    fi
    if [ -f "${BRAND}/eterea-icon.svg" ]; then
        install -Dm644 "${BRAND}/eterea-icon.svg" \
            "${BUNDLE}/share/icons/hicolor/scalable/apps/eterea.svg"
    else
        log "warning: brand icon missing: docs/assets/brand/eterea-icon.svg"
    fi
else
    log "warning: docs/assets/brand missing; bundle has no icons"
fi

# --- provenance and checksums ----------------------------------------------
SOURCE_COMMIT="$(git -C "${ROOT}" rev-parse HEAD 2>/dev/null || printf 'unknown')"
SOURCE_TREE="clean"
if [ -n "$(git -C "${ROOT}" status --porcelain 2>/dev/null | head -1)" ]; then
    SOURCE_TREE="dirty (uncommitted changes in the build tree)"
fi
RUSTC_VERSION="$(rustc --version 2>/dev/null || printf 'unknown')"
BUILD_DATE="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
cat > "${BUNDLE}/PROVENANCE.md" <<EOF
# eterea Linux bundle provenance

- Name: eterea
- Version: ${VERSION}
- Target: linux/x86_64
- Source commit: ${SOURCE_COMMIT}
- Source tree: ${SOURCE_TREE}
- Build environment: devenv development shell (Rust ${RUSTC_VERSION})
- Build command: cargo build -p eterea-desktop --release
- Package command: scripts/package-linux-release.sh
- Build date (UTC): ${BUILD_DATE}
- Runtime libraries: collected from the devenv build environment, stored in lib/
- ELF interpreter: ${INTERPRETER}
- Binary RUNPATH: \$ORIGIN/lib
- Glibc bundled: $( [ "${BUNDLE_GLIBC}" -eq 1 ] && echo "yes (build loader+glibc in lib/)" || echo "no (host glibc)" )
- Portability model: non-glibc libs in lib/, launcher uses a host loader
- Portable launcher: eterea-launcher (${LAUNCHER_LOADER} loader)
- Signing/notarization: not performed
- Checksums: SHA256SUMS lists every file in this bundle
- Stale-binary mechanics test: ${STALE_NOTE}
EOF

log "writing SHA256SUMS"
( cd "${BUNDLE}" && find . -type f ! -name SHA256SUMS -printf '%P\n' | LC_ALL=C sort | xargs -d '\n' sha256sum > SHA256SUMS )

# --- validate the bundle is self-contained ---------------------------------
log "checking that the patched binary resolves every library from lib/"
if ! ( cd "${BUNDLE}" && env -i PATH=/usr/bin:/bin ldd ./eterea ) > "${DIST}/.ldd-check.log" 2>&1; then
    log "warning: clean-environment ldd failed; see dist/.ldd-check.log"
fi
if grep -q 'not found' "${DIST}/.ldd-check.log"; then
    die "bundle has unresolved libraries:
$(grep 'not found' "${DIST}/.ldd-check.log" | head -20)"
fi
rm -f "${DIST}/.ldd-check.log"

# --- tar + checksum ---------------------------------------------------------
if [ "${MAKE_TAR}" -eq 1 ]; then
    log "creating ${BUNDLE_NAME}.tar.gz"
    tar -C "${DIST}" -czf "${DIST}/${BUNDLE_NAME}.tar.gz" "${BUNDLE_NAME}"
    ( cd "${DIST}" && sha256sum "${BUNDLE_NAME}.tar.gz" > "${BUNDLE_NAME}.tar.gz.sha256" )
fi

# --- summary ----------------------------------------------------------------
BUNDLE_SIZE="$(du -sh "${BUNDLE}" | cut -f1)"
LIB_COUNT="$(find "${BUNDLE}/lib" -maxdepth 1 -type f -name '*.so*' | wc -l)"
echo
log "bundle ready: ${BUNDLE} (${BUNDLE_SIZE}, ${LIB_COUNT} shared libraries)"
find "${BUNDLE}" -maxdepth 2 -mindepth 1 -printf '  %P\n' | LC_ALL=C sort
if [ "${MAKE_TAR}" -eq 1 ]; then
    log "tarball: ${DIST}/${BUNDLE_NAME}.tar.gz ($(du -h "${DIST}/${BUNDLE_NAME}.tar.gz" | cut -f1))"
fi
log "launch directly: ${BUNDLE}/eterea"
log "portable launch: ${BUNDLE}/eterea-launcher"
