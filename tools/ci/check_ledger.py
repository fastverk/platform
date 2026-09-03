#!/usr/bin/env python3
"""Check LEDGER.md against the trees that are actually on disk.

Imported include rows must exist as <module>/MODULE.bazel with matching
name + version. Pending include/optional rows must not have a MODULE.bazel
(do not pretend they landed). Absorb and excluded names must not appear as
module directories.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
LEDGER = ROOT / "LEDGER.md"

ROW = re.compile(
    r"^\| (?P<module>[\w.-]+) \| (?P<status>imported|pending|excluded|absorb) \|",
    re.M,
)

def parse_module_bazel(text: str) -> tuple[str | None, str | None]:
    name = version = None
    in_call = False
    for line in text.splitlines():
        t = line.strip()
        if t.startswith("#"):
            continue
        if not in_call:
            if t.startswith("module("):
                in_call = True
            else:
                continue
        if name is None:
            m = re.search(r'name\s*=\s*"([^"]+)"', t)
            if m:
                name = m.group(1)
        if version is None:
            m = re.search(r'version\s*=\s*"([^"]+)"', t)
            if m:
                version = m.group(1)
        if ")" in t and (name and version):
            break
        if t.endswith(")") and in_call:
            break
    return name, version


def parse_ledger() -> list[dict]:
    rows = []
    section = None
    for line in LEDGER.read_text().splitlines():
        if line.startswith("## Includes"):
            section = "include"
            continue
        if line.startswith("## Optional"):
            section = "optional"
            continue
        if line.startswith("## Absorb"):
            section = "absorb"
            continue
        if line.startswith("## Excludes"):
            section = "exclude"
            continue
        if line.startswith("### ") or line.startswith("## "):
            continue
        m = ROW.match(line)
        if not m:
            continue
        cols = [c.strip() for c in line.strip().strip("|").split("|")]
        row = {
            "module": cols[0],
            "status": cols[1],
            "section": section,
        }
        if row["status"] in {"excluded", "absorb"}:
            if len(cols) < 3:
                continue
            rows.append(row)
            continue
        if len(cols) < 6:
            continue
        row["declared_name"] = cols[4].strip("`")
        row["declared_version"] = cols[5].strip("`")
        row["sha"] = cols[3].strip().strip("`")
        rows.append(row)
    return rows


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
