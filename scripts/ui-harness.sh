#!/usr/bin/env bash
# Build and run the headless Rune UI verification harness.
#
#   bash scripts/ui-harness.sh --update   # capture the matrix and rewrite the goldens
#   bash scripts/ui-harness.sh            # capture the matrix and verify the goldens
#   bash scripts/ui-harness.sh --build    # force the cargo build step
#   bash scripts/ui-harness.sh --no-build # never build; use the binary as it is
#
# The wrapper builds eterea by default. If this shell cannot run cargo (for
# example PATH points at a rustup shim with no installed toolchain) it falls
# back to an existing binary and refuses a binary older than the app sources,
# because a stale binary would verify the wrong code. Point RUST_TOOLCHAIN_BIN
# at a directory containing cargo/rustc to build with a specific toolchain, or
# use `devenv shell` / direnv from the repo root.
#
# Environment overrides:
#   CARGO_HOME, CARGO_TARGET_DIR   cargo locations (default: .work/... in this checkout)
#   RUST_TOOLCHAIN_BIN             prepended to PATH for cargo/rustc (optional)
#   CARGO                          cargo binary to run (optional)
#   MESA_LAVAPIPE                  Nix store path of Mesa with lavapipe (headless Vulkan)
#   VK_ICD_FILENAMES, LD_LIBRARY_PATH  used as-is when already set
#
# Headless Rune needs a Vulkan adapter. On a machine without a GPU (CI, agent
# shells) point MESA_LAVAPIPE at a Mesa build that includes lavapipe; on a
# desktop with a working Vulkan/macOS/Windows driver neither variable is needed.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
CARGO_HOME=${CARGO_HOME:-$ROOT/.work/cargo-home}
CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$ROOT/.work/rune-spike/target}
MESA_LAVAPIPE=${MESA_LAVAPIPE:-/nix/store/1vkzwrp4ny20iljfnv9valq0bfn8czlv-mesa-26.2.3}
BIN=$CARGO_TARGET_DIR/debug/eterea
CARGO=${CARGO:-cargo}

if [ -n "${RUST_TOOLCHAIN_BIN:-}" ]; then
    export PATH=$RUST_TOOLCHAIN_BIN:$PATH
fi

if [ -n "$MESA_LAVAPIPE" ] && [ -d "$MESA_LAVAPIPE" ]; then
    export VK_ICD_FILENAMES=${VK_ICD_FILENAMES:-$MESA_LAVAPIPE/share/vulkan/icd.d/lvp_icd.x86_64.json}
    export LD_LIBRARY_PATH=${LD_LIBRARY_PATH:-$MESA_LAVAPIPE/lib}
fi

build=auto
forward=()
for argument in "$@"; do
    case "$argument" in
        --build) build=1 ;;
        --no-build) build=0 ;;
        -h | --help) build=0; forward+=("$argument") ;;
        *) forward+=("$argument") ;;
    esac
done

cargo_usable() {
    command -v "$CARGO" >/dev/null 2>&1 && "$CARGO" --version >/dev/null 2>&1
}

stale_binary() {
    local newer
    newer=$(find "$ROOT/src" -name '*.rs' -newer "$BIN" -print -quit 2>/dev/null || true)
    [ -n "$newer" ]
}

if [ "$build" != 0 ]; then
    if cargo_usable; then
        echo "ui-harness: cargo build -p eterea-desktop -p eterea-core"
        (
            cd "$ROOT"
            CARGO_HOME=$CARGO_HOME CARGO_TARGET_DIR=$CARGO_TARGET_DIR "$CARGO" build -p eterea-desktop -p eterea-core
        )
        build=done
    elif [ "$build" = 1 ]; then
        echo "ui-harness: error: $CARGO is not usable in this shell; fix PATH or set RUST_TOOLCHAIN_BIN/CARGO" >&2
        exit 2
    else
        echo "ui-harness: warning: $CARGO is not usable in this shell; using the existing binary" >&2
        build=skipped
    fi
fi

if [ ! -x "$BIN" ]; then
    echo "ui-harness: error: app binary not found: $BIN" >&2
    echo "           build it with: CARGO_HOME=$CARGO_HOME CARGO_TARGET_DIR=$CARGO_TARGET_DIR cargo build -p eterea-desktop" >&2
    exit 2
fi

if [ "$build" = skipped ] && stale_binary; then
    echo "ui-harness: error: $BIN is older than src/; build it before verifying (fix PATH, or run --build)" >&2
    exit 2
fi

cd "$ROOT"
exec python3 "$ROOT/verification/ui/harness.py" --binary "$BIN" "${forward[@]+"${forward[@]}"}"
