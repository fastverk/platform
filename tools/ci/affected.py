#!/usr/bin/env python3
"""Map a git diff onto imported module directories for path-filtered CI.

A module is an immediate child of the vehicle root that contains MODULE.bazel.
Git repo ≠ Bazel module: this script never treats the vehicle root as a workspace.

Prints a GitHub Actions job output block:

    matrix=<json>
    any=true|false

If CI vehicle files change (.github/workflows/ci.yml, tools/ci/**,
tools/ledger-check.sh, tools/changed-modules.sh), every imported module is
affected. README/LEDGER-only changes affect none.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
VEHICLE_FORCE_ALL_PREFIXES = (
    ".github/workflows/ci.yml",
    "tools/ci/",
    "tools/ledger-check.sh",
    "tools/changed-modules.sh",
)

OS_RUNNERS = ("ubuntu-latest", "macos-latest")


def run(args: list[str]) -> str:
    return subprocess.check_output(args, cwd=ROOT, text=True).strip()


def imported_modules() -> list[str]:
    names = []
    for child in sorted(ROOT.iterdir()):
        if child.is_dir() and (child / "MODULE.bazel").is_file():
            names.append(child.name)
    return names


def load_overrides() -> dict:
    path = Path(__file__).with_name("modules.json")
    data = json.loads(path.read_text())
    return {k: v for k, v in data.items() if not k.startswith("_")}


def changed_files(base: str, head: str) -> list[str]:
    if not base or set(base) == {"0"}:
        # New branch / empty before SHA: treat as vehicle-wide so the first
        # push still exercises imported modules.
        return ["tools/ci/affected.py"]
    try:
        out = run(["git", "diff", "--name-only", f"{base}...{head}"])
    except subprocess.CalledProcessError:
        out = run(["git", "diff", "--name-only", base, head])
    return [line for line in out.splitlines() if line]


def affected(modules: list[str], files: list[str]) -> list[str]:
    if any(
        f == p.rstrip("/") or f.startswith(p)
        for f in files
        for p in VEHICLE_FORCE_ALL_PREFIXES
    ):
        return list(modules)
    hit = []
    for name in modules:
        prefix = name + "/"
        if any(f == name or f.startswith(prefix) for f in files):
            hit.append(name)
    return hit


def matrix_rows(name: str, overrides: dict) -> list[dict]:
    ov = overrides.get(name, {})
    extra = ov.get("extra") or []
    os_list = ov.get("os") or list(OS_RUNNERS)
    bazel_by_os = ov.get("bazel_by_os") or {}
    default_bazel = ov.get("bazel", "test //... --test_output=errors")
    rows = []
    for os_name in os_list:
        extra_cmd = ""
        extra_workdir = "."
        setup_protoc = "false"
        if extra:
            ex = extra[0]
            extra_os = ex.get("os")
            if extra_os is None or extra_os == os_name:
                extra_cmd = ex["cmd"]
                extra_workdir = ex.get("workdir", ".")
                if ov.get("setup_protoc") or ex.get("setup_protoc"):
                    setup_protoc = "true"
        rows.append(
            {
                "module": name,
                "os": os_name,
                "bazel": bazel_by_os.get(os_name, default_bazel),
                "extra_cmd": extra_cmd,
                "extra_workdir": extra_workdir,
                "noworkspace_rc": "true" if ov.get("noworkspace_rc") else "false",
                "setup_go": "true" if ov.get("setup_go") else "false",
                "setup_protoc": setup_protoc,
            }
        )
    return rows


def resolve_range() -> tuple[str, str]:
    head = os.environ.get("AFFECTED_HEAD") or os.environ.get("GITHUB_SHA") or "HEAD"
    base = os.environ.get("AFFECTED_BASE") or ""
    if not base:
        event = os.environ.get("GITHUB_EVENT_NAME", "")
        if event == "pull_request":
            base = os.environ.get("GITHUB_BASE_SHA") or os.environ.get(
                "AFFECTED_PR_BASE", "origin/main"
            )
        else:
            base = os.environ.get("GITHUB_EVENT_BEFORE", "")
    return base, head


def main() -> int:
    modules = imported_modules()
    overrides = load_overrides()
    if os.environ.get("AFFECTED_ALL") == "1":
        names = modules
    else:
        base, head = resolve_range()
        files = changed_files(base, head)
        names = affected(modules, files)
    rows = [row for n in names for row in matrix_rows(n, overrides)]
    payload = {"include": rows}
    any_modules = "true" if rows else "false"
    if os.environ.get("GITHUB_OUTPUT"):
        with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as fh:
            fh.write(f"any={any_modules}\n")
            fh.write(f"matrix={json.dumps(payload)}\n")
    else:
        print(
            json.dumps(
                {"any": any_modules, "matrix": payload, "modules": names},
                indent=2,
            )
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
