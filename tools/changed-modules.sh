#!/usr/bin/env bash
# Map a git diff onto imported module directories (path-filtered CI).
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
exec python3 "$root/tools/ci/affected.py" "$@"
