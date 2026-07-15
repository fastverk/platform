# service-finder

Semantic / **capability-based service discovery** for fastverk. One gRPC call —

```
Resolve(capability, selector) -> [endpoints]      // + Watch(...) -> stream
```

— replaces the half-dozen places across the platform that each hand-roll the same
loop: *list k8s Services/CRs by an attribute → read the endpoint → synthesize
`svc.cluster.local` DNS → dial*. Consumers stay **k8s-agnostic**: no kube client,
no RBAC, testable off-cluster. The finder is the one component that talks to the
API server.

## Why

A survey of the fastverk repos found this pattern independently reimplemented
5–6 times (botnoc's `web/src/discovery.rs`, the `mcp-catalog`, the control-plane
`rbe.rs`, the modgraph precompute, the `plugin-tbzl` graphd client, the
`LanguageParser` parser registry) — with a 6th about to be hand-rolled in the
`ConsolePlugin` operator's stubbed `ensureRegistered`. Five operators already
publish `.status.endpoint`. The justification for a shared primitive is that
**mechanism duplication**, not a proliferation of selectors (the selector is
almost always "a name/label → a cluster endpoint").

## Contract

`proto/fastverk/finder/v1/finder.proto` — `fastverk.finder.v1.Finder`:

- **`Resolve`** — one-shot; a local lookup against a live cache (off the network
  hot path). Empty result (not an error) when nothing is registered → the caller
  falls back to its own default, exactly as the hand-rolled finders do today.
- **`Watch`** — an initial `SNAPSHOT` then a `CHANGED` event (the full new set)
  whenever the matching endpoints change. This is the live update the in-process
  finders never had (e.g. `discovery.rs` polls only at boot).

## Registration convention (on the backing k8s Service)

The finder is **CRD-agnostic** — it resolves labeled Services, never a specific
CRD:

| key | on | meaning |
|---|---|---|
| label `finder.fastverk.dev/capability=<cap>` | Service | the capability it serves (the List filter) |
| annotation `finder.fastverk.dev/selectors` | Service | JSON of advertised attrs, e.g. `{"ext":[".rs",".rlib"],"language":"rust"}` (value = string or array) |
| annotation `finder.fastverk.dev/endpoint` | Service | optional explicit URL override (external / non-Service target) |
| named ports (`grpc`,`http`,…) | Service | endpoints derived as `http://<svc>.<ns>.svc.cluster.local:<port>` |

**Match rule:** a Service matches a `Resolve` selector when, for *every* key in the
query, the Service advertises that key and the query value is among its value(s).
Empty selector → all Services carrying the capability. This is the exact
generalization of `discovery.rs` (plugin-id → Service becomes
`capability="console-plugin"`, no selector).

### Examples

```
# code-search parser routing (the first consumer)
Resolve("ast-parser", {"ext": ".rs"}, port="grpc")

# console plugin gateway (discovery.rs migration)
Resolve("console-plugin", {}, port="http")

# build-graph query plane (modgraph precompute / rbe.rs)
Resolve("graphd", {"repo": "fastverk/botnoc"}, port="grpc")
```

## What's in this repo

- **Daemon** `//:service-finder-server` (`src/main.rs`) — serves the Finder on
  `:50060` (`FINDER_ADDR`), backed by a `reflector` cache of capability-labeled
  Services in `POD_NAMESPACE`. gRPC health + reflection registered.
  - `src/resolver.rs` — the pure capability/selector → endpoints mapping (unit
    tested, no cluster needed).
  - `src/registry.rs` — the live Service cache + change notifier.
  - `src/service.rs` — the `Finder` gRPC surface.
- **Client lib** `//client/rust:service-finder-client` — the reusable client
  in-cluster Rust consumers (botnoc `discovery.rs`, controlplane `rbe.rs`) dial
  the finder with. `Watched` keeps a live, last-known-good snapshot so discovery
  is off the request hot path and survives finder restarts.
- **Chart** `deploy/charts/service-finder` — Deployment + Service (ClusterIP,
  gRPC :50060) + ServiceAccount + a Role granting only `list`/`watch` on
  `services`. `helm lint` clean.

## Build

```
cargo build --workspace && cargo test        # green
helm lint deploy/charts/service-finder       # green
```

Image (immediate path, from a Mac):

```
docker buildx build --platform linux/amd64 \
  -t 042825952740.dkr.ecr.us-east-1.amazonaws.com/service-finder:<sha> --push .
```

The house style is a Bazel-built OCI image (`//:service-finder-image_push`,
`tools/oci` + `--config=rbe`), wired exactly like `forge`/`agents`; standing that
up on RBE is a follow-up (the Dockerfile is the interim path).
