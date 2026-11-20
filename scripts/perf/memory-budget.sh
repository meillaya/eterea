#!/usr/bin/env bash
# Reusable before/after memory harness for the eterea desktop app.
#
# Launches the app for a fixed settle window in a fully isolated state directory
# (its own database copy, settings, XDG dirs and media/video cache), samples
# /proc while it runs, stops it, and prints one markdown table row. The raw
# metrics are kept as JSON so a "before" run and an "after" run can be compared
# (and gated with --strict against .work/perf/plan.md acceptance thresholds).
#
#   scripts/perf/memory-budget.sh --case demo
#   scripts/perf/memory-budget.sh --case empty --binary result/bin/eterea
#   scripts/perf/memory-budget.sh --case archive --db .work/perf/baseline/bookmarks.db
#   scripts/perf/memory-budget.sh --case archive \
#       --config-dir .work/perf/baseline/config-auto --strict
#   scripts/perf/memory-budget.sh --case stress --stress-rows 400 --stress-media
#
# The case definitions mirror the memory project's measurements:
#   empty    an empty archive with media off (ETEREA_HIDE_MEDIA=1)
#   archive  a copy of a real archive with isolated caches; media off unless the
#            settings file in --config-dir enables media
#   demo     the deterministic bundled demo deck (ETEREA_DEMO=1, local images)
#   stress   ETEREA_DEMO=1 plus ETEREA_DEMO_STRESS_ROWS=N (and optionally
#            ETEREA_DEMO_STRESS_MEDIA=1), the T4/F5 stress hooks. When the
#            binary predates those hooks the row is still produced and a note
#            says the switches had no observable effect.
#
# Isolation contract: everything this script writes lives under --out-dir, which
# must be under <repo>/.work. The script copies the source database (read-only
# SQLite backup) into the out-dir and never touches the source, the default
# profile or the default caches. It reaps the app (and any launcher children) on
# exit; --no-wipe reuses an existing out-dir (useful to prewarm the disk media
# cache before a measurement run).
#
# Measured values per sample: VmRSS/VmHWM/VmPeak/Threads from /proc/PID/status,
# Pss/Private_Clean+Private_Dirty from /proc/PID/smaps_rollup, CPU utime/stime
# from /proc/PID/stat, the cgroup memory.current delta for the app's scope, host
# VRAM used from /sys/class/drm/card*/device/mem_info_vram_used (global, so a
# delta is recorded too), and the image-cache bytes/slots from the
# ETEREA_MEMORY_LOG=1 stderr telemetry (T4/F5) when the binary supports it.
#
# --strict thresholds (release build, reference host; .work/perf/plan.md):
#   empty                  RSS within +-8% of 190 MiB
#   archive, media on      RSS <= 300 MiB (auto-media first screen)
#   stress, --stress-media image-cache bytes <= 192 MiB +10% and RSS <= 480 MiB
#                          (requires --stress-rows >= 400)
#   stress, no media       RSS within +-10% of the demo baseline (requires
#                          --stress-rows >= 5000 and --reference-json pointing at
#                          a demo-case metrics JSON)
#
# Exit codes: 0 measured (strict passed), 1 strict failure, 2 usage/config error,
# 3 environment error (missing binary/db), 4 app exited before the settle window.
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd -- "$script_dir/../.." && pwd)
work_root=$repo_root/.work

usage() {
    cat <<'USAGE'
Usage:
  memory-budget.sh [options] --case empty|archive|demo|stress

Options:
  --binary PATH        app binary (default: target/release/eterea; the Nix
                       `result/bin/eterea` wrapper also works)
  --label NAME         row label (default: derived from the case)
  --out-dir DIR        isolated state dir, must be under <repo>/.work
                       (default: .work/perf/run-<label>)
  --seconds N          settle window in seconds (default: case-dependent,
                       8 empty / 10 archive media-off / 16 archive media-on /
                       10 demo / 16 stress)
  --case CASE          empty | archive | demo | stress
  --db PATH            archive source database (default:
                       ~/.local/share/eterea/bookmarks.db); copied read-only
  --config-dir DIR     settings dir under <repo>/.work; its settings.json
                       decides whether media is on (media:true) or off
                       (default: a generated media-off/on config in the out-dir)
  --stress-rows N      ETEREA_DEMO_STRESS_ROWS for the stress case (default 400)
  --stress-media       also set ETEREA_DEMO_STRESS_MEDIA=1
  --launcher CMD       prefix used to launch the binary, split on whitespace
                       (e.g. "devenv shell --" for a cargo-built binary that
                       needs the development shell's runtime libraries)
  --env KEY=VALUE      extra environment assignment for the app (repeatable;
                       e.g. --env LD_LIBRARY_PATH=... to run a cargo-built
                       binary with the same libraries as the Nix wrapper)
  --strict             apply the acceptance thresholds above; nonzero on failure
  --json PATH          also write the raw metrics JSON under <repo>/.work
                       (always also written to <out-dir>/metrics.json)
  --reference-json P   demo-case metrics JSON used by the stress no-media gate
  --no-wipe            keep an existing out-dir (media cache) instead of
                       recreating it
  --header             print the markdown header before the data row
  -h, --help           show this help

Case aliases --empty, --archive [PATH], --demo and --stress are accepted.
The script prints one markdown table row to stdout and diagnostics to stderr.
USAGE
}

die() {
    printf 'memory-budget: error: %s\n' "$*" >&2
    exit 2
}

note() {
    printf 'memory-budget: %s\n' "$*" >&2
}

# Resolve a path that must live under <repo>/.work and print it.
under_work() {
    local path=$1 real
    real=$(realpath -m -- "$path")
    case $real in
        "$work_root"/*) printf '%s\n' "$real" ;;
        *) die "path must be under $work_root: $path" ;;
    esac
}

binary=$repo_root/target/release/eterea
label=""
out_dir=""
seconds=""
case_name=""
db_path=""
config_dir=""
stress_rows=""
stress_media=0
strict=0
json_path=""
reference_json=""
launcher=""
extra_env=()
no_wipe=0
header=0

while [ $# -gt 0 ]; do
    case $1 in
        --binary) [ $# -ge 2 ] || die "--binary needs a path"; binary=$2; shift 2 ;;
        --label) [ $# -ge 2 ] || die "--label needs a name"; label=$2; shift 2 ;;
        --out-dir) [ $# -ge 2 ] || die "--out-dir needs a path"; out_dir=$2; shift 2 ;;
        --seconds) [ $# -ge 2 ] || die "--seconds needs a number"; seconds=$2; shift 2 ;;
        --case) [ $# -ge 2 ] || die "--case needs empty|archive|demo|stress"; case_name=$2; shift 2 ;;
        --empty) case_name=empty; shift ;;
        --demo) case_name=demo; shift ;;
        --archive)
            case_name=archive
            if [ $# -ge 2 ] && [ "${2#--}" = "$2" ]; then db_path=$2; shift 2; else shift; fi
            ;;
        --stress) case_name=stress; shift ;;
        --db) [ $# -ge 2 ] || die "--db needs a path"; db_path=$2; shift 2 ;;
        --config-dir) [ $# -ge 2 ] || die "--config-dir needs a path"; config_dir=$2; shift 2 ;;
        --stress-rows) [ $# -ge 2 ] || die "--stress-rows needs a number"; stress_rows=$2; shift 2 ;;
        --stress-media) stress_media=1; shift ;;
        --launcher) [ $# -ge 2 ] || die "--launcher needs a command"; launcher=$2; shift 2 ;;
        --env)
            [ $# -ge 2 ] || die "--env needs KEY=VALUE"
            case $2 in
                *=*) extra_env+=("$2") ;;
                *) die "--env needs KEY=VALUE: $2" ;;
            esac
            shift 2
            ;;
        --strict) strict=1; shift ;;
        --json) [ $# -ge 2 ] || die "--json needs a path"; json_path=$2; shift 2 ;;
        --reference-json) [ $# -ge 2 ] || die "--reference-json needs a path"; reference_json=$2; shift 2 ;;
        --no-wipe) no_wipe=1; shift ;;
        --header) header=1; shift ;;
        -h | --help) usage; exit 0 ;;
        *) die "unknown argument: $1 (try --help)" ;;
    esac
done

case $case_name in
    empty | archive | demo | stress) ;;
    "") die "a case is required: --case empty|archive|demo|stress" ;;
    *) die "unknown case '$case_name'; expected empty|archive|demo|stress" ;;
esac

case $seconds in
    "") ;;
    *[!0-9]*) die "--seconds must be a positive integer: $seconds" ;;
    *) [ "$seconds" -ge 3 ] || die "--seconds must be at least 3" ;;
esac

if ! command -v python3 >/dev/null 2>&1; then
    die "python3 is required"
fi

if [ -n "$launcher" ]; then
    read -r -a launch_prefix <<<"$launcher"
else
    launch_prefix=()
fi

# --- case configuration -----------------------------------------------------

media_on=0
case_label=$case_name

if [ -n "$config_dir" ]; then
    config_dir=$(under_work "$config_dir")
    [ -d "$config_dir" ] || die "config dir does not exist: $config_dir"
fi

if [ -n "$db_path" ] && [ "$case_name" != archive ]; then
    die "--db only applies to --case archive"
fi

if [ -n "$stress_rows" ]; then
    case $stress_rows in
        *[!0-9]*) die "--stress-rows must be a positive integer: $stress_rows" ;;
        *) [ "$stress_rows" -ge 1 ] || die "--stress-rows must be positive" ;;
    esac
fi

if [ -n "$json_path" ]; then
    json_path=$(under_work "$json_path")
    mkdir -p "$(dirname -- "$json_path")"
fi

# --- binary -----------------------------------------------------------------

[ -e "$binary" ] || die "binary not found: $binary"
[ -f "$binary" ] && [ -x "$binary" ] || die "binary is not an executable file: $binary"
binary_real=$(realpath -- "$binary")
binary_sha256=$(sha256sum -- "$binary_real" | cut -d' ' -f1)
binary_mtime=$(date -u -r "$binary_real" '+%Y-%m-%dT%H:%M:%SZ')
binary_kind=ELF
first_two=$(head -c 2 -- "$binary_real" 2>/dev/null || true)
if [ "$first_two" = '#!' ]; then
    binary_kind=script
fi

# --- out-dir ----------------------------------------------------------------

if [ -z "$label" ]; then
    case $case_name in
        archive) label=archive ;;
        stress) label="stress-$([ -n "$stress_rows" ] && printf '%s' "$stress_rows" || printf '400')" ;;
        *) label=$case_name ;;
    esac
fi
# Keep labels filesystem/table safe.
label_slug=$(printf '%s' "$label" | tr -c 'A-Za-z0-9._-' '-')
[ -n "$label_slug" ] || label_slug=run

if [ -z "$out_dir" ]; then
    out_dir=$work_root/perf/run-$label_slug
fi
out_dir=$(under_work "$out_dir")
case $out_dir in
    "$work_root" | "$repo_root") die "refusing to use $out_dir as the out-dir" ;;
esac

if [ -e "$out_dir" ] && [ "$no_wipe" -eq 0 ]; then
    rm -rf -- "$out_dir"
fi
mkdir -p -- "$out_dir"
out_dir=$(cd -- "$out_dir" && pwd)

# --- database and settings --------------------------------------------------

db_copy=""
empty_db=""
settings_media_state=""

write_settings() {
    local dir=$1 media=$2 avatars=$3
    mkdir -p -- "$dir"
    printf '{"avatars": %s, "media": %s}\n' "$avatars" "$media" >"$dir/settings.json"
}

if [ -n "$config_dir" ]; then
    effective_config=$config_dir
    if [ -f "$effective_config/settings.json" ]; then
        media_on=$(python3 - "$effective_config/settings.json" <<'PY'
import json, sys
try:
    with open(sys.argv[1], "r", encoding="utf-8") as handle:
        data = json.load(handle)
except Exception:
    print(1)
    raise SystemExit(0)
print(1 if data.get("media", True) else 0)
PY
)
    else
        media_on=1
    fi
    settings_media_state="from --config-dir"
else
    effective_config=$out_dir/config
    case $case_name in
        empty | archive)
            write_settings "$effective_config" false false
            media_on=0
            settings_media_state="generated media-off"
            ;;
        demo | stress)
            write_settings "$effective_config" true true
            media_on=1
            settings_media_state="generated media-on"
            ;;
    esac
fi

if [ "$case_name" = empty ] && [ "$media_on" -eq 1 ]; then
    # The empty case is defined as media off; keep a caller config dir but force
    # the switch so the case keeps its meaning.
    note "empty case: forcing media off even though the config enables media"
    media_on=0
fi

case $case_name in
    archive)
        if [ -z "$db_path" ]; then
            db_path=$HOME/.local/share/eterea/bookmarks.db
        fi
        [ -e "$db_path" ] || die "archive database not found: $db_path"
        db_copy=$out_dir/archive/bookmarks.db
        mkdir -p -- "$(dirname -- "$db_copy")"
        python3 - "$db_path" "$db_copy" <<'PY'
import os, shutil, sqlite3, sys
from urllib.parse import quote

source, target = sys.argv[1], sys.argv[2]
if not os.path.exists(source):
    print(f"source database missing: {source}", file=sys.stderr)
    raise SystemExit(3)
try:
    uri = "file:" + quote(source) + "?mode=ro"
    reader = sqlite3.connect(uri, uri=True)
    writer = sqlite3.connect(target)
    reader.backup(writer)
    writer.close()
    reader.close()
except Exception as error:  # WAL files a read-only connection cannot read
    print(f"read-only backup failed ({error}); copying database files", file=sys.stderr)
    for suffix in ("", "-wal", "-shm"):
        if os.path.exists(source + suffix):
            shutil.copyfile(source + suffix, target + suffix)
    writer = sqlite3.connect(target)
    try:
        writer.execute("pragma wal_checkpoint(truncate)")
    finally:
        writer.close()
if not os.path.exists(target) or os.path.getsize(target) == 0:
    print("database copy is empty", file=sys.stderr)
    raise SystemExit(3)
PY
        if [ "$media_on" -eq 1 ]; then case_label=archive-auto; fi
        ;;
    empty)
        empty_db=$out_dir/empty/bookmarks.db
        mkdir -p -- "$(dirname -- "$empty_db")"
        ;;
esac

# --- case-dependent seconds -------------------------------------------------

if [ -z "$seconds" ]; then
    case $case_name in
        empty) seconds=8 ;;
        demo) seconds=10 ;;
        stress) seconds=16 ;;
        archive) if [ "$media_on" -eq 1 ]; then seconds=16; else seconds=10; fi ;;
    esac
fi

# --- launch environment ------------------------------------------------------

env_assign=(
    "ETEREA_MEMORY_LOG=1"
    "ETEREA_CONFIG_DIR=$effective_config"
    "XDG_DATA_HOME=$out_dir/xdg/data"
    "XDG_CONFIG_HOME=$out_dir/xdg/config"
    "XDG_CACHE_HOME=$out_dir/xdg/cache"
    "ETEREA_CACHE_DIR=$out_dir/media-cache"
)
env_unset=(
    -u ETEREA_HIDE_MEDIA
    -u ETEREA_DEMO
    -u ETEREA_DEMO_STRESS_ROWS
    -u ETEREA_DEMO_STRESS_MEDIA
    -u ETEREA_PLAY
    -u ETEREA_IMPORT
    -u ETEREA_SELECTED
    -u ETEREA_ROUTE
)
case $case_name in
    empty)
        env_assign+=("ETEREA_DB=$empty_db" "ETEREA_HIDE_MEDIA=1")
        ;;
    archive)
        env_assign+=("ETEREA_DB=$db_copy")
        if [ "$media_on" -eq 1 ]; then
            env_unset+=(-u ETEREA_HIDE_MEDIA)
        else
            env_assign+=("ETEREA_HIDE_MEDIA=1")
        fi
        ;;
    demo)
        env_assign+=("ETEREA_DEMO=1")
        ;;
    stress)
        if [ -z "$stress_rows" ]; then stress_rows=400; fi
        env_assign+=("ETEREA_DEMO=1" "ETEREA_DEMO_STRESS_ROWS=$stress_rows")
        if [ "$stress_media" -eq 1 ]; then
            env_assign+=("ETEREA_DEMO_STRESS_MEDIA=1")
            case_label="$case_label-media"
        fi
        ;;
esac

launch_cmd=(env "${env_unset[@]}" "${env_assign[@]}" "${extra_env[@]}" "${launch_prefix[@]}" "$binary_real")
printf '%s\n' "${launch_cmd[*]}" >"$out_dir/command.txt"
{
    printf 'out_dir=%s\n' "$out_dir"
    printf 'binary=%s\n' "$binary_real"
    printf 'binary_sha256=%s\n' "$binary_sha256"
    printf 'case=%s\n' "$case_name"
    printf 'label=%s\n' "$label"
    printf 'seconds=%s\n' "$seconds"
    printf 'media_on=%s\n' "$media_on"
    printf 'stress_rows=%s\n' "${stress_rows:-}"
    printf 'stress_media=%s\n' "$stress_media"
    printf 'launcher=%s\n' "$launcher"
    printf 'extra_env=%s\n' "${extra_env[*]:-}"
    printf 'config_dir=%s\n' "$effective_config"
} >"$out_dir/params.txt"

# --- process lifecycle helpers ----------------------------------------------

launched_pid=""
cleanup_done=0

kill_tree() {
    local root=$1 app_pid=$2
    python3 - "$root" "$app_pid" <<'PY' || true
import os, signal, sys, time

root = int(sys.argv[1])
app = None
if sys.argv[2]:
    try:
        app = int(sys.argv[2])
    except ValueError:
        app = None

def alive(pid):
    return os.path.isdir(f"/proc/{pid}")

def descendants(pid):
    children = {}
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        try:
            with open(f"/proc/{entry}/stat", "rb") as handle:
                data = handle.read()
        except OSError:
            continue
        close = data.rfind(b")")
        if close < 0:
            continue
        fields = data[close + 2 :].split()
        if len(fields) < 2:
            continue
        children.setdefault(int(fields[1]), []).append(int(entry))
    seen, order, stack = set(), [], [pid]
    while stack:
        current = stack.pop()
        for child in children.get(current, []):
            if child not in seen:
                seen.add(child)
                order.append(child)
                stack.append(child)
    return order

def signal_tree(pid, signum):
    for candidate in [pid] + descendants(pid):
        if not alive(candidate):
            continue
        try:
            os.kill(candidate, signum)
        except OSError:
            pass

app_targets = [app] if app and alive(app) else [root]
for target in app_targets:
    signal_tree(target, signal.SIGTERM)
signal_tree(root, signal.SIGTERM)
deadline = time.time() + 2.0
while time.time() < deadline:
    remaining = [p for p in ([app] if app else []) + [root] if p and alive(p)]
    if not remaining:
        break
    time.sleep(0.05)
for target in app_targets:
    signal_tree(target, signal.SIGKILL)
signal_tree(root, signal.SIGKILL)
PY
}

cleanup() {
    if [ "$cleanup_done" -eq 1 ]; then
        return
    fi
    cleanup_done=1
    local app_pid=""
    if [ -f "$out_dir/app.pid" ]; then
        app_pid=$(cat "$out_dir/app.pid" 2>/dev/null || true)
    fi
    if [ -n "$launched_pid" ]; then
        if [ -n "$app_pid" ] && [ -d "/proc/$app_pid" ]; then
            kill_tree "$launched_pid" "$app_pid"
        elif [ -d "/proc/$launched_pid" ]; then
            kill_tree "$launched_pid" ""
        fi
        wait "$launched_pid" 2>/dev/null || true
    fi
}
trap cleanup EXIT INT TERM

# --- sample the host before launch -----------------------------------------

cgroup_dir_for_pid() {
    python3 - "$1" <<'PY'
import os, sys
try:
    with open(f"/proc/{sys.argv[1]}/cgroup") as handle:
        lines = [line.strip() for line in handle if line.strip()]
except OSError:
    raise SystemExit(0)
path = ""
for line in lines:
    parts = line.split(":", 2)
    if len(parts) == 3:
        path = parts[2]
print(path)
PY
}

cgroup_rel=$(cgroup_dir_for_pid "$$")
cgroup_path=""
if [ -n "$cgroup_rel" ]; then
    cgroup_path=/sys/fs/cgroup$cgroup_rel
fi
cgroup_before=""
if [ -n "$cgroup_path" ] && [ -r "$cgroup_path/memory.current" ]; then
    cgroup_before=$(cat "$cgroup_path/memory.current" 2>/dev/null || true)
fi
vram_before_json=$(python3 - <<'PY'
import glob, json
used = {}
for path in sorted(glob.glob("/sys/class/drm/card*/device/mem_info_vram_used")):
    try:
        with open(path) as handle:
            used[path.split("/")[4]] = int(handle.read().strip())
    except (OSError, ValueError):
        pass
print(json.dumps({"used": used, "total": sum(used.values())}))
PY
)

# --- launch and measure -----------------------------------------------------

note "launching $binary_real (case=$case_name, label=$label, settle=${seconds}s)"
"${launch_cmd[@]}" >"$out_dir/app.stdout" 2>"$out_dir/app.stderr" &
launched_pid=$!

measure_status=0
python3 - "$launched_pid" "$binary_real" "$seconds" "$out_dir" "$cgroup_before" "$vram_before_json" <<'PY' || measure_status=$?
import glob
import json
import os
import re
import sys
import time

launched_pid = int(sys.argv[1])
binary = sys.argv[2]
seconds = float(sys.argv[3])
out_dir = sys.argv[4]
cgroup_before = sys.argv[5]
vram_before = json.loads(sys.argv[6])

sample_interval = 2.0
resolve_timeout = 10.0
status = "ok"
error = ""

def read(path):
    with open(path, "r", encoding="utf-8", errors="replace") as handle:
        return handle.read()

def alive(pid):
    return os.path.isdir(f"/proc/{pid}")

def exe_of(pid):
    try:
        return os.readlink(f"/proc/{pid}/exe")
    except OSError:
        return ""

def cmdline_of(pid):
    try:
        with open(f"/proc/{pid}/cmdline", "rb") as handle:
            return handle.read().replace(b"\0", b" ").decode("utf-8", "replace").strip()
    except OSError:
        return ""

def descendants(pid):
    children = {}
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        try:
            with open(f"/proc/{entry}/stat", "rb") as handle:
                data = handle.read()
        except OSError:
            continue
        close = data.rfind(b")")
        if close < 0:
            continue
        fields = data[close + 2 :].split()
        if len(fields) < 2:
            continue
        children.setdefault(int(fields[1]), []).append(int(entry))
    seen, order, stack = set(), [], [pid]
    while stack:
        current = stack.pop()
        for child in children.get(current, []):
            if child not in seen:
                seen.add(child)
                order.append(child)
                stack.append(child)
    return order

def looks_like_app(pid):
    executable = exe_of(pid)
    if not executable:
        return False
    base = os.path.basename(executable)
    if base == os.path.basename(binary):
        return os.path.realpath(executable) == os.path.realpath(binary)
    lowered = base.lower()
    if "eterea" in lowered and "bash" not in lowered and "sh" != lowered:
        return True
    command = cmdline_of(pid)
    return "eterea" in command and "devenv" not in command.split(" ")[0]

def resolve_app_pid():
    deadline = time.time() + resolve_timeout
    while time.time() < deadline:
        if alive(launched_pid):
            candidates = []
            if looks_like_app(launched_pid):
                candidates.append(launched_pid)
            candidates.extend(pid for pid in descendants(launched_pid) if looks_like_app(pid))
            if candidates:
                # Prefer the process with a real /proc/PID/status and a name that
                # is not a shell; the first candidate is the closest descendant.
                return candidates[0]
        time.sleep(0.05)
    return None

pidfile = os.path.join(out_dir, "app.pid")
app_pid = resolve_app_pid()
if app_pid is None:
    with open(os.path.join(out_dir, "measure.json"), "w", encoding="utf-8") as handle:
        json.dump(
            {"status": "start_failed", "error": "could not find the app process after launch", "samples": []},
            handle,
            indent=2,
        )
    print("could not find the app process after launch", file=sys.stderr)
    raise SystemExit(3)
with open(pidfile, "w", encoding="utf-8") as handle:
    handle.write(str(app_pid))

def sample():
    metrics = {"t_s": None}
    try:
        status_text = read(f"/proc/{app_pid}/status")
    except OSError:
        return None
    fields = {}
    for line in status_text.splitlines():
        key, _, value = line.partition(":")
        fields[key.strip()] = value.strip()

    def kb(name):
        raw = fields.get(name, "")
        try:
            return int(raw.split()[0])
        except (IndexError, ValueError):
            return None

    def mib(value):
        return None if value is None else round(value / 1024.0, 2)

    metrics["rss_mib"] = mib(kb("VmRSS"))
    metrics["hwm_mib"] = mib(kb("VmHWM"))
    metrics["peak_mib"] = mib(kb("VmPeak"))
    metrics["vsize_mib"] = mib(kb("VmSize"))
    try:
        metrics["threads"] = int(fields.get("Threads", ""))
    except ValueError:
        metrics["threads"] = None
    metrics["rss_anon_mib"] = mib(kb("RssAnon"))
    metrics["rss_file_mib"] = mib(kb("RssFile"))

    try:
        rollup = read(f"/proc/{app_pid}/smaps_rollup")
    except OSError:
        rollup = ""
    rollup_fields = {}
    for line in rollup.splitlines():
        key, _, value = line.partition(":")
        rollup_fields[key.strip()] = value.strip()

    def rollup_kb(name):
        raw = rollup_fields.get(name, "")
        try:
            return int(raw.split()[0])
        except (IndexError, ValueError):
            return None

    metrics["pss_mib"] = mib(rollup_kb("Pss"))
    private = 0
    for part in ("Private_Clean", "Private_Dirty"):
        value = rollup_kb(part)
        if value is not None:
            private += value
    metrics["private_mib"] = mib(private) if rollup else None
    metrics["pss_anon_mib"] = mib(rollup_kb("Pss_Anon"))
    metrics["pss_file_mib"] = mib(rollup_kb("Pss_File"))
    metrics["swap_mib"] = mib(rollup_kb("Swap"))

    try:
        stat_text = read(f"/proc/{app_pid}/stat")
        close = stat_text.rfind(")")
        stat_fields = stat_text[close + 2 :].split()
        clk = os.sysconf("SC_CLK_TCK")
        metrics["cpu_user_s"] = round(int(stat_fields[11]) / clk, 3)
        metrics["cpu_sys_s"] = round(int(stat_fields[12]) / clk, 3)
    except (OSError, ValueError, IndexError):
        metrics["cpu_user_s"] = None
        metrics["cpu_sys_s"] = None

    try:
        with open(f"/proc/{app_pid}/cgroup") as handle:
            lines = [line.strip() for line in handle if line.strip()]
        rel = ""
        for line in lines:
            parts = line.split(":", 2)
            if len(parts) == 3:
                rel = parts[2]
        memory_path = f"/sys/fs/cgroup{rel}/memory.current" if rel else ""
        metrics["cgroup_path"] = rel
        if memory_path and os.path.exists(memory_path):
            with open(memory_path) as handle:
                metrics["cgroup_mib"] = round(int(handle.read().strip()) / (1024 * 1024), 2)
        else:
            metrics["cgroup_mib"] = None
    except OSError:
        metrics["cgroup_mib"] = None

    used = {}
    for path in sorted(glob.glob("/sys/class/drm/card*/device/mem_info_vram_used")):
        try:
            with open(path) as handle:
                used[path.split("/")[4]] = int(handle.read().strip())
        except (OSError, ValueError):
            pass
    metrics["vram_used_mib"] = round(sum(used.values()) / (1024 * 1024), 1) if used else None
    metrics["vram_cards_mib"] = {card: round(value / (1024 * 1024), 1) for card, value in used.items()}
    return metrics

samples = []
final = None
started = time.monotonic()
while True:
    metrics = sample()
    if metrics is None:
        status = "exited_early"
        error = "the app exited before the settle window finished"
        break
    elapsed = time.monotonic() - started
    metrics["t_s"] = round(elapsed, 2)
    samples.append(metrics)
    final = metrics
    remaining = seconds - elapsed
    if remaining <= 0:
        break
    time.sleep(min(sample_interval, remaining))

def parse_memory_log(path):
    telemetry = {
        "telemetry_supported": False,
        "image_cache_bytes": None,
        "image_cache_slots": None,
        "image_cache_pending": None,
        "image_cache_evictions": None,
        "rows_seen": None,
        "raw_lines": [],
    }
    try:
        text = read(path)
    except OSError:
        return telemetry
    bytes_patterns = [
        re.compile(r"image[_-]?cache[_-]?bytes\s*[=:]\s*(\d+)", re.IGNORECASE),
        re.compile(r"image[_-]?cache\s*[=:]\s*(\d+)", re.IGNORECASE),
        re.compile(r"cache[_-]?bytes\s*[=:]\s*(\d+)", re.IGNORECASE),
        re.compile(r"memory[_-]?bytes\s*[=:]\s*(\d+)", re.IGNORECASE),
    ]
    mib_patterns = [
        re.compile(r"image[_-]?cache\s*[=:]\s*(\d+(?:\.\d+)?)\s*(?:MiB|MB|mebibytes)", re.IGNORECASE),
    ]
    slot_patterns = [
        re.compile(r"image[_-]?cache[_-]?slots\s*[=:]\s*(\d+)", re.IGNORECASE),
        re.compile(r"cache[_-]?slots\s*[=:]\s*(\d+)", re.IGNORECASE),
        re.compile(r"slots?\s*[=:]\s*(\d+)", re.IGNORECASE),
    ]
    pending_patterns = [
        re.compile(r"image[_-]?cache[_-]?pending\s*[=:]\s*(\d+)", re.IGNORECASE),
        re.compile(r"cache[_-]?pending\s*[=:]\s*(\d+)", re.IGNORECASE),
    ]
    eviction_patterns = [
        re.compile(r"image[_-]?cache[_-]?evictions\s*[=:]\s*(\d+)", re.IGNORECASE),
        re.compile(r"cache[_-]?evictions\s*[=:]\s*(\d+)", re.IGNORECASE),
    ]
    rows_patterns = [
        re.compile(r"(?:stress[_-]?)?rows\s*[=:]\s*(\d+)", re.IGNORECASE),
        re.compile(r"(?:demo[_-]?)?row[_-]?count\s*[=:]\s*(\d+)", re.IGNORECASE),
    ]
    relevant = []
    for line in text.splitlines():
        if "eterea" not in line.lower() and "memory" not in line.lower() and "cache" not in line.lower():
            continue
        lowered = line.lower()
        if "image" not in lowered and "cache" not in lowered and "memory" not in lowered and "stress" not in lowered:
            continue
        relevant.append(line)
        if "image" in lowered or "cache" in lowered:
            telemetry["telemetry_supported"] = True
        for pattern in bytes_patterns:
            match = pattern.search(line)
            if match:
                telemetry["image_cache_bytes"] = int(match.group(1))
                telemetry["telemetry_supported"] = True
        for pattern in mib_patterns:
            match = pattern.search(line)
            if match:
                telemetry["image_cache_bytes"] = int(round(float(match.group(1)) * 1024 * 1024))
                telemetry["telemetry_supported"] = True
        for pattern in slot_patterns:
            match = pattern.search(line)
            if match:
                telemetry["image_cache_slots"] = int(match.group(1))
        for pattern in pending_patterns:
            match = pattern.search(line)
            if match:
                telemetry["image_cache_pending"] = int(match.group(1))
        for pattern in eviction_patterns:
            match = pattern.search(line)
            if match:
                telemetry["image_cache_evictions"] = int(match.group(1))
        for pattern in rows_patterns:
            if "stress" in lowered or "row" in lowered:
                match = pattern.search(line)
                if match:
                    telemetry["rows_seen"] = int(match.group(1))
    telemetry["raw_lines"] = relevant[-5:]
    return telemetry

telemetry = parse_memory_log(os.path.join(out_dir, "app.stderr"))
cgroup_final = final.get("cgroup_mib") if final else None
cgroup_before_mib = (
    round(int(cgroup_before) / (1024 * 1024), 2) if cgroup_before.isdigit() else None
)
vram_used = final.get("vram_used_mib") if final else None
vram_total = vram_before.get("total")
vram_delta = (
    round(vram_used - vram_total / (1024 * 1024), 1) if vram_used is not None and vram_total else None
)
result = {
    "schema": "eterea-memory-budget/v1",
    "status": status,
    "error": error,
    "app_pid": app_pid,
    "app_exe": exe_of(app_pid) if alive(app_pid) else "",
    "app_cmdline": cmdline_of(app_pid) if alive(app_pid) else "",
    "launched_pid": launched_pid,
    "requested_seconds": seconds,
    "sample_interval_s": sample_interval,
    "samples": samples,
    "final": final,
    "cpu": {
        "user_s": final.get("cpu_user_s") if final else None,
        "sys_s": final.get("cpu_sys_s") if final else None,
    },
    "cgroup": {
        "path": final.get("cgroup_path", "") if final else "",
        "baseline_mib": cgroup_before_mib,
        "measured_mib": cgroup_final,
        "delta_mib": round(cgroup_final - cgroup_before_mib, 2)
        if cgroup_final is not None and cgroup_before_mib is not None
        else None,
    },
    "vram": {
        "used_mib": vram_used,
        "baseline_mib": round(vram_total / (1024 * 1024), 1) if vram_total else None,
        "delta_mib": vram_delta,
        "cards_mib": final.get("vram_cards_mib") if final else None,
    },
    "telemetry": telemetry,
    "ended_at_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
}
with open(os.path.join(out_dir, "measure.json"), "w", encoding="utf-8") as handle:
    json.dump(result, handle, indent=2)
if status != "ok":
    print(error, file=sys.stderr)
    raise SystemExit(3)
PY

# --- stop the app and collect --------------------------------------------

cleanup
cleanup_done=1

# --- format row, evaluate acceptance, write metrics ------------------------

status=0
MB_LABEL=$label \
    MB_CASE=$case_name \
    MB_CASE_LABEL=$case_label \
    MB_MEDIA_ON=$media_on \
    MB_STRESS_ROWS=${stress_rows:-} \
    MB_STRESS_MEDIA=$stress_media \
    MB_STRICT=$strict \
    MB_OUT_DIR=$out_dir \
    MB_JSON_PATH=$json_path \
    MB_REFERENCE_JSON=$reference_json \
    MB_BINARY=$binary_real \
    MB_BINARY_SHA256=$binary_sha256 \
    MB_BINARY_MTIME=$binary_mtime \
    MB_BINARY_KIND=$binary_kind \
    MB_SECONDS=$seconds \
    MB_LAUNCHER=$launcher \
    MB_HEADER=$header \
    MB_CONFIG_DIR=$effective_config \
    MB_DB_PATH=${db_path:-} \
    MB_DB_COPY=${db_copy:-} \
    MB_SETTINGS_STATE=$settings_media_state \
    MB_MEASURE_EXIT=$measure_status \
    MB_COMMAND=$(tr '\n' ' ' <"$out_dir/command.txt") \
    MB_GIT_HEAD=$(git -C "$repo_root" rev-parse --short HEAD 2>/dev/null || true) \
    MB_GIT_DIRTY=$(git -C "$repo_root" status --porcelain 2>/dev/null | wc -l | tr -d ' ') \
    MB_KERNEL=$(uname -r) \
    MB_CPU_MODEL=$(awk -F: '/model name/ {gsub(/^ +| +$/, "", $2); print $2; exit}' /proc/cpuinfo) \
    MB_HOST=$(uname -n) \
    MB_DATE_UTC=$(date -u '+%Y-%m-%dT%H:%M:%SZ') \
    MB_SCRIPT_SHA256=$(sha256sum "$0" | cut -d' ' -f1) \
    python3 - <<'PY'
import json
import math
import os
import sys

out_dir = os.environ["MB_OUT_DIR"]
case_name = os.environ["MB_CASE"]
case_label = os.environ["MB_CASE_LABEL"]
label = os.environ["MB_LABEL"]
media_on = os.environ["MB_MEDIA_ON"] == "1"
stress_rows = os.environ["MB_STRESS_ROWS"]
stress_media = os.environ["MB_STRESS_MEDIA"] == "1"
strict = os.environ["MB_STRICT"] == "1"
seconds = float(os.environ["MB_SECONDS"])
measure_exit = int(os.environ["MB_MEASURE_EXIT"])

with open(os.path.join(out_dir, "measure.json"), "r", encoding="utf-8") as handle:
    measure = json.load(handle)
try:
    with open(os.path.join(out_dir, "app.stderr"), "r", encoding="utf-8", errors="replace") as handle:
        stderr_text = handle.read()
except OSError:
    stderr_text = ""

final = measure.get("final") or {}
telemetry = measure.get("telemetry") or {}
status = measure.get("status", "unknown")

def value(key):
    value = final.get(key)
    return value

def fmt_mib(number):
    return "n/a" if number is None else f"{number:.1f}"

def fmt_int(number):
    return "n/a" if number is None else f"{number}"

rss = value("rss_mib")
pss = value("pss_mib")
private = value("private_mib")
threads = value("threads")
cache_bytes = telemetry.get("image_cache_bytes")
cache_mib = None if cache_bytes is None else round(cache_bytes / (1024 * 1024), 1)
vram = measure.get("vram", {}).get("used_mib")
cgroup = measure.get("cgroup", {}).get("delta_mib")

notes = []
if status != "ok":
    notes.append(f"app {status}: {measure.get('error', '')}".strip())
if case_name == "stress" and cache_bytes is None:
    notes.append(
        "ETEREA_DEMO_STRESS_ROWS/ETEREA_DEMO_STRESS_MEDIA had no observable effect "
        "(binary lacks the T4/F5 stress + memory-log hooks): stress row count unverified"
    )
elif case_name == "stress" and telemetry.get("rows_seen") not in (None, int(stress_rows or 0)):
    notes.append(
        f"stress telemetry reported {telemetry.get('rows_seen')} rows, requested {stress_rows}"
    )
elif case_name != "stress" and cache_bytes is None:
    notes.append("image-cache telemetry not supported by this binary (pre-T4 build)")

row = (
    f"| {label} | {case_label} | {fmt_mib(rss)} | {fmt_mib(pss)} | {fmt_mib(private)} | "
    f"{fmt_int(threads)} | {fmt_mib(cache_mib)} | {fmt_mib(vram)} | {fmt_mib(cgroup)} |"
)

header = (
    "| label | case | RSS MiB | PSS MiB | private MiB | threads | image cache MiB | "
    "VRAM used MiB | cgroup MiB |"
)
separator = "|---|---|---:|---:|---:|---:|---:|---:|---:|"

checks = []
def check(name, passed, detail):
    checks.append({"name": name, "pass": bool(passed), "detail": detail})

release = not os.environ["MB_GIT_DIRTY"] or os.environ["MB_GIT_DIRTY"] == "0"

if measure_exit != 0:
    check("app_started", False, f"measurement failed with exit {measure_exit}")
else:
    check("app_started", True, "app sampled through the settle window")

def parse_reference_rss(path):
    try:
        with open(path, "r", encoding="utf-8") as handle:
            data = json.load(handle)
    except (OSError, ValueError) as error:
        return None, f"cannot read reference JSON: {error}"
    reference = data.get("metrics", data)
    final_ref = reference.get("final") if isinstance(reference, dict) else None
    if isinstance(final_ref, dict) and final_ref.get("rss_mib") is not None:
        return final_ref["rss_mib"], None
    if isinstance(reference, dict) and reference.get("rss_mib") is not None:
        return reference["rss_mib"], None
    return None, "reference JSON has no RSS value"

if strict:
    if status != "ok":
        check("app_healthy", False, "app did not stay up for the settle window")
    else:
        check("app_healthy", True, "app stayed up for the settle window")

    if case_name == "empty":
        target = 190.0
        low, high = target * 0.92, target * 1.08
        rss_value = rss if rss is not None else float("nan")
        check(
            "empty_rss_within_8pct_190mib",
            rss is not None and low <= rss <= high,
            f"RSS {fmt_mib(rss)} MiB vs [{low:.1f}, {high:.1f}] MiB (+-8% of 190)",
        )
    elif case_name == "archive" and media_on:
        check(
            "archive_auto_media_rss_le_300mib",
            rss is not None and rss <= 300.0,
            f"RSS {fmt_mib(rss)} MiB vs <= 300 MiB",
        )
    elif case_name == "stress" and stress_media:
        rows = int(stress_rows or 0)
        if rows < 400:
            check("stress_media_rows_ge_400", False, f"--stress-rows {rows} < 400 (plan uses 400)")
        else:
            check("stress_media_rows_ge_400", True, f"{rows} rows")
            if cache_bytes is None:
                check(
                    "stress_media_cache_telemetry",
                    False,
                    "image-cache bytes unavailable (ETEREA_MEMORY_LOG unsupported or not parsed)",
                )
            else:
                cap = 192.0 * 1.10
                cache_mib_value = cache_bytes / (1024 * 1024)
                check(
                    "stress_media_cache_le_192mib_plus_10pct",
                    cache_mib_value <= cap,
                    f"image cache {cache_mib_value:.1f} MiB vs <= {cap:.1f} MiB",
                )
        check(
            "stress_media_rss_le_480mib",
            rss is not None and rss <= 480.0,
            f"RSS {fmt_mib(rss)} MiB vs <= 480 MiB",
        )
    elif case_name == "stress" and not stress_media:
        rows = int(stress_rows or 0)
        if rows < 5000:
            check(
                "stress_no_media_rows_ge_5000",
                False,
                f"--stress-rows {rows} < 5000 (plan uses 5000 for the no-media gate)",
            )
        else:
            check("stress_no_media_rows_ge_5000", True, f"{rows} rows")
            reference = os.environ.get("MB_REFERENCE_JSON", "")
            if not reference:
                check(
                    "stress_no_media_rss_within_10pct_demo",
                    False,
                    "no --reference-json demo metrics given",
                )
            else:
                reference_rss, error = parse_reference_rss(reference)
                if reference_rss is None:
                    check("stress_no_media_rss_within_10pct_demo", False, error)
                else:
                    low, high = reference_rss * 0.90, reference_rss * 1.10
                    check(
                        "stress_no_media_rss_within_10pct_demo",
                        rss is not None and low <= rss <= high,
                        f"RSS {fmt_mib(rss)} MiB vs demo {reference_rss:.1f} "
                        f"[{low:.1f}, {high:.1f}] MiB (+-10%)",
                    )
    elif case_name == "archive":
        checks.append(
            {
                "name": "archive_media_off_threshold",
                "pass": True,
                "detail": "no fixed threshold for media-off archive (informational)",
            }
        )
    else:
        checks.append(
            {"name": "case_threshold", "pass": True, "detail": "no fixed threshold for this case"}
        )

    for pattern in ("thread 'main' panicked", "panicked at"):
        if pattern in stderr_text:
            check("no_panic", False, f"stderr contains '{pattern}'")
            break
    else:
        check("no_panic", True, "no panic in stderr")

thresholds = {
    "empty_rss_target_mib": 190.0,
    "empty_rss_tolerance": 0.08,
    "archive_auto_media_rss_max_mib": 300.0,
    "stress_media_cache_max_mib": 192.0 * 1.10,
    "stress_media_rss_max_mib": 480.0,
    "stress_no_media_demo_tolerance": 0.10,
}

host = {
    "kernel": os.environ.get("MB_KERNEL", ""),
    "cpu_model": os.environ.get("MB_CPU_MODEL", ""),
    "hostname": os.environ.get("MB_HOST", ""),
    "date_utc": os.environ.get("MB_DATE_UTC", ""),
}
metrics = {
    "schema": "eterea-memory-budget/v1",
    "label": label,
    "case": case_name,
    "case_variant": case_label,
    "strict": strict,
    "status": status,
    "seconds": seconds,
    "binary": {
        "path": os.environ.get("MB_BINARY", ""),
        "sha256": os.environ.get("MB_BINARY_SHA256", ""),
        "mtime_utc": os.environ.get("MB_BINARY_MTIME", ""),
        "kind": os.environ.get("MB_BINARY_KIND", ""),
    },
    "launcher": os.environ.get("MB_LAUNCHER", ""),
    "repo": {
        "git_head": os.environ.get("MB_GIT_HEAD", ""),
        "dirty_paths": int(os.environ.get("MB_GIT_DIRTY") or 0),
    },
    "host": host,
    "environment": {
        "config_dir": os.environ.get("MB_CONFIG_DIR", ""),
        "settings_media_state": os.environ.get("MB_SETTINGS_STATE", ""),
        "db_source": os.environ.get("MB_DB_PATH", ""),
        "db_copy": os.environ.get("MB_DB_COPY", ""),
        "media_on": media_on,
        "stress_rows": stress_rows,
        "stress_media": stress_media,
    },
    "command": os.environ.get("MB_COMMAND", ""),
    "out_dir": out_dir,
    "script_sha256": os.environ.get("MB_SCRIPT_SHA256", ""),
    "row": row,
    "header": header,
    "separator": separator,
    "notes": notes,
    "metrics": measure,
    "acceptance": {
        "thresholds": thresholds,
        "checks": checks,
        "pass": all(item["pass"] for item in checks) if strict else None,
    },
}
if not strict:
    checks = []
    metrics["acceptance"]["checks"] = []

with open(os.path.join(out_dir, "metrics.json"), "w", encoding="utf-8") as handle:
    json.dump(metrics, handle, indent=2)
json_path = os.environ.get("MB_JSON_PATH", "")
if json_path:
    with open(json_path, "w", encoding="utf-8") as handle:
        json.dump(metrics, handle, indent=2)

if os.environ.get("MB_HEADER") == "1":
    print(header)
    print(separator)
print(row)
for note in notes:
    print(f"memory-budget: note: {note}", file=sys.stderr)
for item in checks:
    verdict = "pass" if item["pass"] else "FAIL"
    print(f"memory-budget: check {item['name']}: {verdict} ({item['detail']})", file=sys.stderr)
if strict:
    passed = all(item["pass"] for item in checks)
    print(
        f"memory-budget: strict {'PASS' if passed else 'FAIL'}: "
        f"{sum(1 for i in checks if i['pass'])}/{len(checks)} checks",
        file=sys.stderr,
    )
    raise SystemExit(0 if passed else 1)
if status != "ok":
    raise SystemExit(4)
raise SystemExit(0)
PY
status=$?

exit "$status"
