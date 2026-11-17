#!/usr/bin/env bash
# Smoke-test a packaged eterea Linux bundle.
#
# Two modes:
#   * process mode (default): starts the app for N seconds and fails when it
#     exits early or reports a window/library error. Catches the original
#     "The wayland library could not be loaded" failure.
#   * snapshot mode (--snap): runs the app headlessly with Rune's tooling
#     (`--snap out.png --tree out.json --at 1`), waits for a clean exit and
#     requires real non-empty files. This is the stronger check: it proves the
#     bundled app created a window, a GPU surface and rendered a frame with the
#     system Mesa driver. It needs a release build with Rune's `tooling` feature.
#
# Run it from the repository root, with a display or with `xvfb-run` on PATH:
#
#   scripts/smoke-linux-bundle.sh dist/eterea-*-linux-x86_64 --seconds 3
#   scripts/smoke-linux-bundle.sh dist/eterea-*-linux-x86_64 --snap
#   VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.x86_64.json \
#     scripts/smoke-linux-bundle.sh dist/eterea-*-linux-x86_64 --snap
#
# Options:
#   --seconds N     process mode: seconds the app must stay alive (default 3, min 2)
#   --snap [FILE]   snapshot mode; FILE defaults to a .png under .tools/tmp
#   --tree [FILE]   snapshot mode: also require a non-empty Rune tree JSON
#   --at SECONDS    snapshot mode: frame time (default 1)
#   --size WxH      snapshot mode: window size (default 1280x820)
#   --scene NAME    snapshot mode: pass --scene NAME (Rune scenes)
#   --launcher      run eterea-launcher instead of the direct ELF
#   --no-xvfb       use the inherited DISPLAY instead of starting a fresh
#                   xvfb-run instance (default: always use a fresh Xvfb when
#                   xvfb-run is available, even if DISPLAY is set)
#   --log FILE      write the app log to FILE (default under .tools/tmp/)
#   --icd PATH      set VK_ICD_FILENAMES (Mesa lavapipe/software Vulkan)
#   --xvfb-run PATH path to xvfb-run when it is not on PATH
#   -h, --help      show this help
#
# Exit codes: 0 checked behaviour passed; 1 failed; 2 usage error.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

TARGET=""
DEADLINE_SECONDS=3
USE_LAUNCHER=0
LOG_FILE=""
ICD=""
XVFB_RUN=""
USE_XVFB=1
SNAP_MODE=0
SNAP_FILE=""
TREE_FILE=""
SNAP_AT="1"
SNAP_SIZE="1280x820"
SNAP_SCENE=""

# Keep the original CLI so the fresh-Xvfb re-exec can pass it along after the
# parser below has consumed the positional parameters.
ORIGINAL_ARGS=("$@")

usage() {
    sed -n '2,38p' "${BASH_SOURCE[0]}"
}

die() {
    printf 'smoke-linux-bundle: FAIL: %s\n' "$*" >&2
    exit 1
}

note() {
    printf 'smoke-linux-bundle: %s\n' "$*"
}

take_value() { # take_value <option> <value>: print the value or usage-error
    [ "$#" -ge 2 ] || { usage >&2; exit 2; }
    printf '%s\n' "$2"
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --seconds) DEADLINE_SECONDS="$(take_value "$1" "${2:-}")"; shift ;;
        --snap)
            SNAP_MODE=1
            case "${2:-}" in
                ''|-*) ;;
                *) SNAP_FILE="$2"; shift ;;
            esac
            ;;
        --tree)
            SNAP_MODE=1
            case "${2:-}" in
                ''|-*) TREE_FILE="auto" ;;
                *) TREE_FILE="$2"; shift ;;
            esac
            ;;
        --at) SNAP_AT="$(take_value "$1" "${2:-}")"; shift ;;
        --size) SNAP_SIZE="$(take_value "$1" "${2:-}")"; shift ;;
        --scene) SNAP_SCENE="$(take_value "$1" "${2:-}")"; shift ;;
        --launcher) USE_LAUNCHER=1 ;;
        --log) LOG_FILE="$(take_value "$1" "${2:-}")"; shift ;;
        --icd) ICD="$(take_value "$1" "${2:-}")"; shift ;;
        --xvfb-run) XVFB_RUN="$(take_value "$1" "${2:-}")"; shift ;;
        --no-xvfb) USE_XVFB=0 ;;
        -h|--help) usage; exit 0 ;;
        -*)
            usage >&2
            exit 2
            ;;
        *)
            [ -z "${TARGET}" ] || { usage >&2; exit 2; }
            TARGET="$1"
            ;;
    esac
    shift
done

for number in "${DEADLINE_SECONDS}" "${SNAP_AT}"; do
    case "${number}" in
        ''|*[!0-9]*) die "seconds/at must be integers: ${number}" ;;
    esac
done
if [ "${DEADLINE_SECONDS}" -lt 2 ]; then
    die "--seconds must be at least 2 (the smoke check must observe the app start)"
fi

[ -n "${TARGET}" ] || { usage >&2; exit 2; }
if [ -d "${TARGET}" ]; then
    if [ "${USE_LAUNCHER}" -eq 1 ]; then
        BINARY="${TARGET}/eterea-launcher"
    else
        BINARY="${TARGET}/eterea"
    fi
else
    BINARY="${TARGET}"
fi
[ -x "${BINARY}" ] || die "not an executable: ${BINARY}"

if [ -n "${ICD}" ]; then
    [ -f "${ICD}" ] || die "--icd file does not exist: ${ICD}"
    export VK_ICD_FILENAMES="${ICD}"
fi

if [ -z "${LOG_FILE}" ]; then
    LOG_DIR="${ROOT}/.tools/tmp"
    if ! mkdir -p "${LOG_DIR}" 2>/dev/null; then
        LOG_DIR="${TMPDIR:-/tmp}"
    fi
    LOG_FILE="${LOG_DIR}/smoke-linux-bundle-$(date +%Y%m%d-%H%M%S).log"
fi
mkdir -p "$(dirname "${LOG_FILE}")"

# Release/CI smoke must not depend on the caller's session: always start a
# fresh Xvfb (with -a picking a free display) and make sure WAYLAND_DISPLAY is
# not inherited, because a stale Wayland handle is what makes winit attempt the
# session compositor instead of the virtual X server. --no-xvfb opts out.
if [ "${USE_XVFB}" -eq 1 ]; then
    if [ "${SMOKE_LINUX_BUNDLE_UNDER_XVFB:-0}" = "1" ]; then
        # Already inside our own fresh Xvfb; clean Wayland vars defensively.
        unset WAYLAND_DISPLAY WAYLAND_SOCKET
    else
        if [ -z "${XVFB_RUN}" ]; then
            if command -v xvfb-run >/dev/null 2>&1; then
                XVFB_RUN="$(command -v xvfb-run)"
            else
                die "xvfb-run not on PATH (install xvfb, pass --xvfb-run PATH, or --no-xvfb)"
            fi
        fi
        note "starting a fresh Xvfb: ${XVFB_RUN} -a -screen 0 ${SNAP_SIZE}x24"
        exec "${XVFB_RUN}" -a -s "-screen 0 ${SNAP_SIZE}x24" \
            env -u WAYLAND_DISPLAY -u WAYLAND_SOCKET \
            SMOKE_LINUX_BUNDLE_UNDER_XVFB=1 \
            bash "${BASH_SOURCE[0]}" "${ORIGINAL_ARGS[@]}"
    fi
elif [ -z "${DISPLAY:-}" ]; then
    die "--no-xvfb was given but DISPLAY is unset"
fi

note "binary: ${BINARY}"
note "display: DISPLAY=${DISPLAY:-unset} WAYLAND_DISPLAY=${WAYLAND_DISPLAY:-unset}"
note "vulkan: VK_ICD_FILENAMES=${VK_ICD_FILENAMES:-unset}"

# A clean-ish environment: no LD_LIBRARY_PATH, preload or Wayland override; the
# bundle must resolve its own libraries. DISPLAY, HOME, PATH, XDG_RUNTIME_DIR
# and the Vulkan ICD variables pass through because they are host/session state.
run_binary() {
    (
        unset LD_LIBRARY_PATH LD_PRELOAD WAYLAND_DISPLAY WAYLAND_SOCKET
        exec "$@"
    ) >>"${LOG_FILE}" 2>&1
}

if [ "${SNAP_MODE}" -eq 1 ]; then
    if [ -z "${SNAP_FILE}" ]; then
        SNAP_FILE="${ROOT}/.tools/tmp/smoke-$(date +%Y%m%d-%H%M%S).png"
    fi
    case "${SNAP_FILE}" in
        /*) ;;
        *) SNAP_FILE="${ROOT}/${SNAP_FILE}" ;;
    esac
    rm -f "${SNAP_FILE}"
    tree_args=()
    if [ -n "${TREE_FILE}" ]; then
        if [ "${TREE_FILE}" = "auto" ]; then
            TREE_FILE="${SNAP_FILE%.png}.tree.json"
        fi
        case "${TREE_FILE}" in
            /*) ;;
            *) TREE_FILE="${ROOT}/${TREE_FILE}" ;;
        esac
        rm -f "${TREE_FILE}"
        tree_args=(--tree "${TREE_FILE}")
    fi
    scene_args=()
    if [ -n "${SNAP_SCENE}" ]; then
        scene_args=(--scene "${SNAP_SCENE}")
    fi
    note "snapshot mode: --snap ${SNAP_FILE} --at ${SNAP_AT} --size ${SNAP_SIZE} ${tree_args[*]:-} ${scene_args[*]:-}"
    note "log: ${LOG_FILE}"
    run_binary timeout 120 "${BINARY}" --snap "${SNAP_FILE}" \
        --at "${SNAP_AT}" --size "${SNAP_SIZE}" "${tree_args[@]}" "${scene_args[@]}"
    status=$?
    if [ "${status}" -ne 0 ]; then
        echo "--- app log (tail) ---" >&2
        tail -n 30 "${LOG_FILE}" >&2 || true
        die "headless snapshot run exited ${status}"
    fi
    [ -s "${SNAP_FILE}" ] || die "snapshot file was not created: ${SNAP_FILE}"
    snap_bytes="$(stat -c%s "${SNAP_FILE}")"
    if [ "${snap_bytes}" -lt 1000 ]; then
        die "snapshot file is suspiciously small (${snap_bytes} bytes): ${SNAP_FILE}"
    fi
    if [ -n "${TREE_FILE}" ]; then
        [ -s "${TREE_FILE}" ] || die "tree file was not created: ${TREE_FILE}"
        tree_bytes="$(stat -c%s "${TREE_FILE}")"
        if [ "${tree_bytes}" -lt 100 ]; then
            die "tree file is suspiciously small (${tree_bytes} bytes): ${TREE_FILE}"
        fi
        note "tree: ${TREE_FILE} (${tree_bytes} bytes)"
    fi
    note "PASS: rendered a ${snap_bytes}-byte snapshot with a real window/surface"
    note "snapshot: ${SNAP_FILE}"
    exit 0
fi

note "log: ${LOG_FILE}"
run_binary "${BINARY}" &
PID=$!

start="$(date +%s)"
while :; do
    if ! kill -0 "${PID}" 2>/dev/null; then
        set +e
        wait "${PID}"
        status=$?
        set -e
        echo "--- app log (tail) ---" >&2
        tail -n 20 "${LOG_FILE}" >&2 || true
        die "process exited after $(( $(date +%s) - start ))s (status ${status}); expected it to stay alive for ${DEADLINE_SECONDS}s"
    fi
    now="$(date +%s)"
    if [ "$(( now - start ))" -ge "${DEADLINE_SECONDS}" ]; then
        break
    fi
    sleep 0.2
done

elapsed="$(( $(date +%s) - start ))"
kill -TERM "${PID}" 2>/dev/null || true
for _ in 1 2 3 4 5 6 7 8 9 10; do
    kill -0 "${PID}" 2>/dev/null || break
    sleep 0.2
done
kill -KILL "${PID}" 2>/dev/null || true
wait "${PID}" 2>/dev/null || true

if grep -qE 'could not create the window|The wayland library could not be loaded|Failed to load one of xlib' "${LOG_FILE}"; then
    echo "--- app log (tail) ---" >&2
    tail -n 20 "${LOG_FILE}" >&2 || true
    die "the app reported a window/library failure (see log)"
fi

note "PASS: process stayed alive for ${elapsed}s with a window (log: ${LOG_FILE})"
exit 0
