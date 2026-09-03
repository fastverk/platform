# Ledger

Provenance for every fastverk gateway/adapter include, every optional later
row, every absorb, and every explicit exclude. This file is the source of
truth for what belongs in this git vehicle.

**Git repo ≠ Bazel module.** Importing a tree does not rename `module(name=...)`
and does not rewrite `module(version=...)`. SHAs below are the source default
branch (`main`) at ledger write / import time.

Status:

- `imported` — subdirectory present; SHA is the subtree-imported commit.
- `pending` — listed for a follow-up PR; SHA is source `main` HEAD when this
  row was written (or `—` when the source is not readable from this token).
  Do not pretend these are in the tree.
- `absorb` — must not appear as a module directory here; residue belongs with
  another imported module.
- `excluded` — must not appear as a module directory here.

Imported via `git subtree add` (no squash) from each source `main` SHA:

- Cluster 1: `forge`, `tracker`, `service-finder`, `wave`.

Public protos already live in
[`fastverk/contracts`](https://github.com/fastverk/contracts)
(`fastverk_contracts`). This vehicle keeps the implementation trees (Rust
gateways, adapters, OCI, Helm, operators). Do **not** rewrite proto ownership
in this PR.

## Includes (public fastverk gateway / adapter modules)

| Module | Status | Source repo | Source SHA | `module(name)` | `module(version)` | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| forge | imported | [fastverk/forge](https://github.com/fastverk/forge) | `98591f75f411701cea00bcd0cf54f803cc2a140d` | forge | 0.0.6 | cluster 1; source CI is `bazel test //...` on Linux (incl. gateway OCI) and `//:forge_test` on macOS; tags `v0.0.1`–`v0.0.6`; registry.tbzl.dev has `forge` 0.0.1–0.0.6 |
| tracker | imported | [fastverk/tracker](https://github.com/fastverk/tracker) | `3927db97f7904e5362271187177175b82a39a005` | tracker | 0.0.4 | cluster 1; source CI is `bazel test //...` on Linux and `//:tracker_test //:conformance` on macOS; tags `v0.0.1`–`v0.0.4` |
| service-finder | imported | [fastverk/service-finder](https://github.com/fastverk/service-finder) | `abc764147a63ef0c48b84ad102010980ed8d5415` | service_finder | 0.0.1 | cluster 1; dir is `service-finder`, Bazel name is `service_finder`; source has no `ci.yml` — `codegen-drift.yml` (`cargo run -p codegen`) + `publish.yml`; tags are `service-finder-client-v0.0.1`–`v0.0.3` (Rust client crate), not the Bazel module; not on the registry |
| wave | imported | [fastverk/wave](https://github.com/fastverk/wave) | `c689d650fe33e34507c42d3dcb65c56954454a07` | wave | 0.1.0 | cluster 1; source CI is `bazel test //...` on Linux and macOS; tags include `v0.0.1`, `v0.1.0`, `v0.2.0`; MODULE.bazel on HEAD is `0.1.0` (kept); registry still at 0.0.1 |

## Optional later

Not imported in this PR. `geetch` is private (404 from this token); leave
pending until a follow-up with access can confirm `module(name)` / version.

| Module | Status | Source repo | Source SHA | `module(name)` | `module(version)` | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| geetch | pending | [fastverk/geetch](https://github.com/fastverk/geetch) (private) | — | — | — | implements forge.v1; not readable from this token; do not subtree until confirmed public-or-granted |

## Absorb

Never subtree these as a module directory in this vehicle. Residue belongs
with an imported module.

| Name | Status | Why |
| --- | --- | --- |
| plugin-planning | absorb | never subtree as a module here; residue belongs with `wave` |

## Follow-up (not this PR)

- [ ] Each imported implementation `bazel_dep(name = "fastverk_contracts", ...)`
      and stop exporting its own copy of the proto. Recorded only; do **not**
      rewrite proto ownership in this PR. See
      [fastverk/contracts LEDGER](https://github.com/fastverk/contracts/blob/main/LEDGER.md).
- [ ] Import `geetch` if/when it is an intended public (or granted) include.
- [ ] Publish new versions from this vehicle's `<module>/vX.Y.Z` tags via
      tomato-bazel/bazel-registry `rels`.

## Follow-up import checklist

Unchecked rows are **not** in this tree. Import with `git subtree add` (no
squash) from the source default branch, then flip the row to `imported` and
set the SHA to the commit that landed.

- [x] forge
- [x] tracker
- [x] service-finder
- [x] wave
- [ ] geetch (optional later; private / pending)

## Excludes

Do not create these directories. Do not import them into this vehicle.

### Control plane / shell / agents

| Name | Status | Why excluded |
| --- | --- | --- |
| botnoc | excluded | botnoc shell / control plane; out of this vehicle |
| deploy | excluded | deploy repo; out of this vehicle |
| agents | excluded | private agent fleet; out of this vehicle |
| agent | excluded | not a gateway/adapter module for this vehicle |

### Plugins

| Name | Status | Why excluded |
| --- | --- | --- |
| plugin-shell | excluded | console-plugin vehicle; not this repo |
| plugins | excluded | plugin collection; not this repo |

### Contracts / spec

| Name | Status | Why excluded |
| --- | --- | --- |
| contracts | excluded | public protos live in [fastverk/contracts](https://github.com/fastverk/contracts) (`fastverk_contracts`); implementations stay here |
| spec | excluded | [fastverk/spec](https://github.com/fastverk/spec) is a separate corpus |

### Desktop

| Name | Status | Why excluded |
| --- | --- | --- |
| desktop | excluded | desktop is a separate vehicle |
| fvkit | excluded | [fastverk/fvkit](https://github.com/fastverk/fvkit) — desktop/runtime vehicle, not this repo |
| fastverk-app | excluded | [fastverk/fastverk-app](https://github.com/fastverk/fastverk-app) — macOS app; desktop vehicle |

### Engines

| Name | Status | Why excluded |
| --- | --- | --- |
| mycelium | excluded | engine; not a gateway/adapter module |
| polyglot | excluded | engine; not a gateway/adapter module |
| agora | excluded | engine; not a gateway/adapter module |
| crank | excluded | engine; not a gateway/adapter module |

## Import method

For each imported row:

```sh
git subtree add --prefix=<dir> https://github.com/fastverk/<dir>.git main
```

No `--squash`. Source history is merged under the prefix; source remotes are
not rewritten. After the add, confirm `<dir>/MODULE.bazel` still declares
the same `name` and `version` as the source default branch (do not reset
versions to a vehicle-wide number). Directory names match the GitHub repo
(`service-finder`); Bazel `module(name)` may differ (`service_finder`).
