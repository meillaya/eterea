#!/usr/bin/env bash
# Build through mbx when it is available, otherwise plain cargo.
set -euo pipefail
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"
if command -v mbx >/dev/null 2>&1; then
  exec mbx build "$@"
fi
if [ -x .work/tools/mbx ]; then
  exec .work/tools/mbx build "$@"
fi
exec cargo build "$@"
