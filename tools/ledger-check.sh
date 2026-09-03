#!/usr/bin/env bash
# Fail if LEDGER.md, on-disk module directories, and MODULE.bazel disagree.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
exec python3 "$root/tools/ci/check_ledger.py" "$@"
