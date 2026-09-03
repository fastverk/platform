"""Shared LEDGER.md row parsing for vehicle CI checks.

Imported by check_ledger.py and check_drift.py so the two scripts cannot
diverge on how a row is read. Behavior of the parser matches the original
inline copy in check_ledger.py; extra keys (source) are ignored by the
ledger check.
"""

from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
LEDGER = ROOT / "LEDGER.md"

ROW = re.compile(
    r"^\| (?P<module>[\w.-]+) \| (?P<status>imported|pending|excluded|absorb) \|",
    re.M,
)

_SOURCE_URL = re.compile(r"https://github.com/(?P<owner>[^/\s]+)/(?P<repo>[^)/\s]+)")


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
        row["source"] = cols[2]
        rows.append(row)
    return rows


def github_repo(source: str) -> tuple[str, str] | None:
    """Return (owner, repo) from a ledger Source-repo cell, or None."""
    m = _SOURCE_URL.search(source or "")
    if not m:
        return None
    return m.group("owner"), m.group("repo").removesuffix(".git")
