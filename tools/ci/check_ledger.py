#!/usr/bin/env python3
"""Check LEDGER.md against the trees that are actually on disk.

Imported include rows must exist as <module>/MODULE.bazel with matching
name + version. Pending include/optional rows must not have a MODULE.bazel
(do not pretend they landed). Absorb and excluded names must not appear as
module directories.
"""

from __future__ import annotations

import sys
from pathlib import Path

# Scripts are invoked as `python3 tools/ci/check_*.py`; keep the sibling
# parser import working regardless of cwd.
_CI = Path(__file__).resolve().parent
if str(_CI) not in sys.path:
    sys.path.insert(0, str(_CI))

from ledger import ROOT, parse_ledger, parse_module_bazel  # noqa: E402


def main() -> int:
    errors: list[str] = []
    rows = parse_ledger()
    if not rows:
        errors.append("LEDGER.md parsed zero rows")

    includes = [r for r in rows if r.get("section") == "include"]
    optionals = [r for r in rows if r.get("section") == "optional"]
    absorbs = [r for r in rows if r["status"] == "absorb"]
    excludes = [r for r in rows if r["status"] == "excluded"]

    on_disk = sorted(
        p.name for p in ROOT.iterdir() if p.is_dir() and (p / "MODULE.bazel").is_file()
    )

    imported = [r for r in includes if r["status"] == "imported"]
    pending = [r for r in includes if r["status"] == "pending"]
    optional_pending = [r for r in optionals if r["status"] == "pending"]

    imported_names = {r["module"] for r in imported}
    pending_names = {r["module"] for r in pending}
    optional_names = {r["module"] for r in optionals}
    absorb_names = {r["module"] for r in absorbs}
    exclude_names = {r["module"] for r in excludes}

    if imported_names != set(on_disk):
        errors.append(
            f"imported rows {sorted(imported_names)} != on-disk modules {on_disk}"
        )

    for r in imported:
        mb = ROOT / r["module"] / "MODULE.bazel"
        if not mb.is_file():
            errors.append(f"{r['module']}: imported but MODULE.bazel missing")
            continue
        name, version = parse_module_bazel(mb.read_text())
        if name != r["declared_name"]:
            errors.append(
                f"{r['module']}: MODULE.bazel name {name!r} != ledger {r['declared_name']!r}"
            )
        if version != r["declared_version"]:
            errors.append(
                f"{r['module']}: MODULE.bazel version {version!r} != ledger {r['declared_version']!r}"
            )

    for r in pending + optional_pending:
        if (ROOT / r["module"] / "MODULE.bazel").is_file():
            errors.append(
                f"{r['module']}: pending in ledger but MODULE.bazel exists on disk"
            )

    for name in exclude_names | absorb_names | pending_names | optional_names:
        if (ROOT / name).exists():
            errors.append(f"{name}: listed as non-imported but path exists at repo root")

    # Vehicle root must not become a Bazel module.
    if (ROOT / "MODULE.bazel").is_file():
        errors.append("root MODULE.bazel must not exist (git repo ≠ Bazel module)")

    if errors:
        print("ledger check FAILED:")
        for e in errors:
            print(f"  - {e}")
        return 1
    print(
        f"ledger check OK: {len(imported)} imported / {len(pending)} pending "
        f"(includes); {len(optional_pending)} optional pending; "
        f"{len(absorbs)} absorb; {len(excludes)} excluded"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
