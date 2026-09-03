#!/usr/bin/env python3
"""Report source-repo drift for imported LEDGER.md rows.

For each imported include row, query the GitHub API for the source repo's
default-branch HEAD and compare/<ledger_sha>...<head>. Print a plain-text
summary always; write a markdown table to $GITHUB_STEP_SUMMARY when set.

Exit 1 if at least one reachable source is ahead of the ledger SHA.
Unreachable / private / 404 sources are warnings and do not fail the run.
A request failure for one module does not abort the rest of the audit.

Does not import, rewrite, or bump any module. Fixing drift is:

    git subtree pull --prefix=<dir> https://github.com/fastverk/<dir>.git main   # no --squash
    # then update that row's Source SHA in LEDGER.md
"""

from __future__ import annotations

import json
import os
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

_CI = Path(__file__).resolve().parent
if str(_CI) not in sys.path:
    sys.path.insert(0, str(_CI))

from ledger import parse_ledger, github_repo  # noqa: E402

API = "https://api.github.com"
USER_AGENT = "fastverk-platform-drift-check"
TIMEOUT = 30
# 403 without a token is usually secondary rate-limit, not a private repo.
_RETRY_STATUSES = {403, 429, 502, 503}
_MAX_ATTEMPTS = 4


def _headers() -> dict[str, str]:
    headers = {
        "Accept": "application/vnd.github+json",
        "User-Agent": USER_AGENT,
        "X-GitHub-Api-Version": "2022-11-28",
    }
    token = os.environ.get("GITHUB_TOKEN", "").strip()
    if token:
        headers["Authorization"] = f"Bearer {token}"
    return headers


def _http_error_detail(exc: urllib.error.HTTPError) -> str:
    retry_after = exc.headers.get("Retry-After") if exc.headers else None
    remaining = (
        exc.headers.get("X-RateLimit-Remaining") if exc.headers else None
    )
    body = ""
    try:
        raw = exc.read()
        if raw:
            parsed = json.loads(raw.decode())
            body = str(parsed.get("message") or "")[:160]
    except (OSError, json.JSONDecodeError, UnicodeDecodeError):
        body = ""
    bits = [f"HTTP {exc.code}"]
    if retry_after:
        bits.append(f"Retry-After={retry_after}")
    if remaining is not None:
        bits.append(f"X-RateLimit-Remaining={remaining}")
    if body:
        bits.append(body)
    return "; ".join(bits)


def github_get(path: str) -> tuple[int, object | None, str]:
    """GET an api.github.com path. Returns (status, json_or_none, error)."""
    url = API + path
    last_status = 0
    last_err = "request failed"
    for attempt in range(_MAX_ATTEMPTS):
        req = urllib.request.Request(url, headers=_headers(), method="GET")
        try:
            with urllib.request.urlopen(req, timeout=TIMEOUT) as resp:
                raw = resp.read()
                try:
                    body = json.loads(raw.decode()) if raw else None
                except json.JSONDecodeError as e:
                    return resp.status, None, f"invalid JSON: {e}"
                return resp.status, body, ""
        except urllib.error.HTTPError as e:
            last_status = e.code
            last_err = _http_error_detail(e)
            retryable = e.code in _RETRY_STATUSES
            if e.code == 403:
                # Distinguish a real private/forbidden repo from a rate limit.
                msg = last_err.lower()
                retryable = (
                    "rate limit" in msg
                    or "secondary rate" in msg
                    or "retry later" in msg
                    or "abuse" in msg
                )
            if not retryable or attempt == _MAX_ATTEMPTS - 1:
                return e.code, None, last_err
            delay = min(8, 2**attempt)
            retry_after = e.headers.get("Retry-After") if e.headers else None
            if retry_after:
                try:
                    delay = max(delay, int(retry_after))
                except ValueError:
                    pass
            time.sleep(delay)
        except urllib.error.URLError as e:
            last_status = 0
            last_err = f"request failed: {e.reason}"
            if attempt == _MAX_ATTEMPTS - 1:
                return 0, None, last_err
            time.sleep(min(8, 2**attempt))
        except TimeoutError:
            last_status = 0
            last_err = "request timed out"
            if attempt == _MAX_ATTEMPTS - 1:
                return 0, None, last_err
            time.sleep(min(8, 2**attempt))
        except OSError as e:
            last_status = 0
            last_err = f"request failed: {e}"
            if attempt == _MAX_ATTEMPTS - 1:
                return 0, None, last_err
            time.sleep(min(8, 2**attempt))
    return last_status, None, last_err


def short_sha(sha: str) -> str:
    sha = (sha or "").strip()
    return sha[:12] if sha else "—"


def audit_row(row: dict) -> dict:
    """Return a result dict for one imported row. Never raises."""
    module = row["module"]
    ledger_sha = row.get("sha") or ""
    result = {
        "module": module,
        "ledger_sha": ledger_sha,
        "head_sha": "",
        "ahead": 0,
        "state": "unreachable",
        "detail": "",
    }
    parsed = github_repo(row.get("source") or "")
    if not parsed:
        result["detail"] = "could not parse source repo URL from ledger"
        return result
    owner, repo = parsed
    slug = f"{owner}/{repo}"

    status, repo_body, err = github_get(f"/repos/{owner}/{repo}")
    if status in (401, 403, 404):
        result["detail"] = f"{slug}: {err or f'HTTP {status}'} (unreachable/private)"
        return result
    if status != 200 or not isinstance(repo_body, dict):
        result["detail"] = f"{slug}: {err or f'HTTP {status}'}"
        return result

    default_branch = repo_body.get("default_branch") or "main"
    status, head_body, err = github_get(
        f"/repos/{owner}/{repo}/commits/{urllib.request.quote(default_branch, safe='')}"
    )
    if status in (401, 403, 404):
        result["detail"] = (
            f"{slug}@{default_branch}: {err or f'HTTP {status}'} (unreachable)"
        )
        return result
    if status != 200 or not isinstance(head_body, dict):
        result["detail"] = f"{slug}@{default_branch}: {err or f'HTTP {status}'}"
        return result
    head_sha = str(head_body.get("sha") or "")
    result["head_sha"] = head_sha
    if not head_sha:
        result["detail"] = f"{slug}: default-branch HEAD missing sha"
        return result

    if not ledger_sha:
        result["detail"] = "ledger SHA empty"
        return result

    compare_path = f"/repos/{owner}/{repo}/compare/{ledger_sha}...{head_sha}"
    status, cmp_body, err = github_get(compare_path)
    if status in (401, 403, 404):
        result["detail"] = f"{slug} compare: {err or f'HTTP {status}'} (unreachable)"
        return result
    if status != 200 or not isinstance(cmp_body, dict):
        result["detail"] = f"{slug} compare: {err or f'HTTP {status}'}"
        return result

    ahead = int(cmp_body.get("ahead_by") or 0)
    result["ahead"] = ahead
    if ahead > 0:
        result["state"] = "drift"
        result["detail"] = f"{ahead} commit{'s' if ahead != 1 else ''} ahead"
    else:
        result["state"] = "in sync"
        result["detail"] = "in sync"
    return result


def write_step_summary(results: list[dict]) -> None:
    path = os.environ.get("GITHUB_STEP_SUMMARY", "").strip()
    if not path:
        return
    lines = [
        "## Source drift",
        "",
        "Imported LEDGER.md SHAs vs each source repo's default-branch HEAD.",
        "This workflow is report-only and is not on the PR path.",
        "",
        "| Module | Status | Ahead | Ledger SHA | Source HEAD | Notes |",
        "| --- | --- | ---: | --- | --- | --- |",
    ]
    for r in results:
        if r["state"] == "in sync":
            status = "in sync"
        elif r["state"] == "drift":
            status = f"{r['ahead']} commit{'s' if r['ahead'] != 1 else ''} ahead"
        else:
            status = "unreachable"
        lines.append(
            f"| `{r['module']}` | {status} | {r['ahead'] if r['state'] != 'unreachable' else '—'} "
            f"| `{short_sha(r['ledger_sha'])}` | `{short_sha(r['head_sha'])}` "
            f"| {r['detail']} |"
        )
    lines.append("")
    Path(path).write_text("\n".join(lines) + "\n")


def main() -> int:
    rows = [
        r
        for r in parse_ledger()
        if r.get("section") == "include" and r.get("status") == "imported"
    ]
    if not rows:
        print("drift check: no imported include rows in LEDGER.md")
        return 1

    results = [audit_row(r) for r in rows]
    in_sync = [r for r in results if r["state"] == "in sync"]
    drifted = [r for r in results if r["state"] == "drift"]
    unreachable = [r for r in results if r["state"] == "unreachable"]

    print(
        f"drift check: {len(in_sync)} in sync, {len(drifted)} drifted, "
        f"{len(unreachable)} unreachable "
        f"({len(results)} imported)"
    )
    for r in results:
        head = short_sha(r["head_sha"])
        led = short_sha(r["ledger_sha"])
        if r["state"] == "in sync":
            print(f"  {r['module']}: in sync ({led})")
        elif r["state"] == "drift":
            print(f"  {r['module']}: {r['detail']} ({led} -> {head})")
        else:
            print(f"  WARNING {r['module']}: unreachable — {r['detail']}")

    write_step_summary(results)

    if drifted:
        print(f"drift check FAILED: {len(drifted)} module(s) ahead of ledger SHA")
        return 1
    print("drift check OK: no drifted modules")
    return 0


if __name__ == "__main__":
    sys.exit(main())
