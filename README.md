# fastverk/platform

This repository is a **git / CI / release vehicle** for fastverk gateway and
adapter Bazel modules (`forge`, `tracker`, `service-finder`, `wave`). It is
**not** a Bazel module.

**Git repo ≠ Bazel module.** Each subdirectory is its own module, with its own
`MODULE.bazel`, its own version, and its own tests. Consumers keep writing:

```python
bazel_dep(name = "forge", version = "0.0.6")
bazel_dep(name = "tracker", version = "0.0.4")
bazel_dep(name = "service_finder", version = "0.0.1")
bazel_dep(name = "wave", version = "0.1.0")
```

Module names and versions are **not** lockstepped. A change that ships
`forge` 0.0.7 does not bump `wave`. There is no repo-wide `0.0.1`.

Published module identity lives in each subdirectory's `MODULE.bazel`
(`module(name = ..., version = ...)`). This git repo only groups those trees,
runs path-filtered CI, and is the place tags are cut from.

Public gRPC contracts already live in
[`fastverk/contracts`](https://github.com/fastverk/contracts)
(`fastverk_contracts`). Implementations stay here. A later follow-up can
`bazel_dep` that module; this vehicle does not rewrite proto ownership.

## Layout

```
platform/
  README.md                 # this file — vehicle, not a module
  LEDGER.md                 # every include / optional / absorb / exclude row
  .github/workflows/ci.yml  # one path-filtered workflow
  tools/ci/                 # affected-module detection + ledger check
  tools/ledger-check.sh     # CI entrypoint: LEDGER ↔ dirs ↔ MODULE.bazel
  tools/changed-modules.sh  # CI entrypoint: path → imported modules
  forge/                    # module(name = "forge", version = "0.0.6")
  tracker/                  # module(name = "tracker", version = "0.0.4")
  service-finder/           # module(name = "service_finder", version = "0.0.1")
  wave/                     # module(name = "wave", version = "0.1.0")
```

One subdirectory per module. Each imported tree keeps the source repo's
`MODULE.bazel` pins, license, and tests. See [LEDGER.md](LEDGER.md) for which
modules are in the tree today and which are follow-up imports.

Cluster 1 is imported (`forge`, `tracker`, `service-finder`, `wave`), and
**all four source repos are now retired: this vehicle is the edit surface.**
They are not deleted or archived — they keep their history and tags so
published registry versions and `git_override` pins keep resolving. See
[Consolidation](https://docs.fastverk.com/consolidation.html).

## Tags

Tags are **per module**, never repo-wide:

```
<module>/vX.Y.Z
```

Examples: `forge/v0.0.7`, `wave/v0.1.1`.

Do not tag `v0.0.1` (or any other version) at the repository root. That would
imply a lockstep bump of every module.

GitHub's archive for a slash tag on this repo unpacks as
`platform-<module>-vX.Y.Z/<module>/`. That directory is the module root a
registry release must `strip_prefix` to.

## How to cut a release for one module

1. Change only that module's subdirectory. Leave other `MODULE.bazel` versions
   alone.
2. Bump **that** module's `module(version = ...)` and its `CHANGELOG.md` when
   the source tree has one.
3. Merge to this repo's default branch.
4. Tag the merge commit:

   ```sh
   git tag forge/v0.0.7
   git push origin forge/v0.0.7
   ```

5. Publish the registry entry from a bazel-registry checkout (same `rels`
   `--workspaces-root` / `--tag-prefix '<module>/v'` / `--strip-prefix`
   pattern as [tomato-bazel/rules](https://github.com/tomato-bazel/rules)).
   Existing published versions keep resolving to the historical per-repo tags
   (`fastverk/forge` `v0.0.6`, etc.). Only **new** versions use this vehicle's
   tags.

## Path-filtered CI

There is one workflow: [`.github/workflows/ci.yml`](.github/workflows/ci.yml).

A change under `forge/` runs that module's tests, not the whole tree. The
detector is [`tools/ci/affected.py`](tools/ci/affected.py) (also
[`tools/changed-modules.sh`](tools/changed-modules.sh)): it diffs against the
PR base (or the push before-SHA) and maps paths to immediate children that
contain `MODULE.bazel`.

| Change | What runs |
| --- | --- |
| `forge/**` | `forge` only |
| `forge/**` and `wave/**` | those two modules |
| `.github/workflows/ci.yml` or `tools/ci/**` | every imported module |
| `README.md` / `LEDGER.md` only | ledger check, no module test matrix |

Per-module commands reuse each source repo's existing Bazel/Rust gate rather
than inventing a new stack. Overrides live in
[`tools/ci/modules.json`](tools/ci/modules.json):

| Module | Linux | macOS | Extra |
| --- | --- | --- | --- |
| `forge` | `bazel test //...` (includes gateway OCI) | `bazel test //:forge_test` (rules_oci has no macOS platform) | — |
| `tracker` | `bazel test //...` | `bazel test //:tracker_test //:conformance` | — |
| `service-finder` | `bazel test //...` | `bazel test //:service_finder_test` | source `codegen-drift.yml`: `cargo run -p codegen` + stub-diff on Linux |
| `wave` | `bazel test //...` | `bazel test //...` | — |

A module with no test targets (`bazel test` exit 4) falls back to
`bazel build //...`. Buildifier runs as a warning so this PR does not rewrite
imported Starlark.

[`tools/ledger-check.sh`](tools/ledger-check.sh) fails CI if `LEDGER.md`,
on-disk module directories, and each `MODULE.bazel` `name`/`version` disagree.

## Provenance

Imports use `git subtree add` (no `--squash`) from each source repo's default
branch. Source history is not rewritten. Source repos are not deleted or
archived here.

[LEDGER.md](LEDGER.md) records, for every include, optional, absorb, and
exclude row: source repo, default-branch commit SHA, `module(name=...)`,
`module(version=...)`, and whether the tree is imported.

## What this repo is not

- Not a single Bazel module and not a root `MODULE.bazel`.
- Not a lockstep version for the constellation.
- Not [fastverk/botnoc](https://github.com/fastverk/botnoc) (private shell /
  control plane).
- Not [fastverk/contracts](https://github.com/fastverk/contracts) (public
  protos; `fastverk_contracts`). Implementations stay here.
- Not `fastverk/plugin-shell` (console plugins are a different vehicle).
- Not desktop: [fastverk/fvkit](https://github.com/fastverk/fvkit) +
  [fastverk/fastverk-app](https://github.com/fastverk/fastverk-app) stay a
  separate vehicle.
- Not a replacement for the existing implementation GitHub repos in this PR.
  Those remotes stay; this tree is an additional git/CI/release surface.
