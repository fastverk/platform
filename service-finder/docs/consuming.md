# Consuming the service-finder

The finder answers `Resolve(capability, selector) → [endpoints]` (+ `Watch`) from a
live cache of k8s Services labeled `finder.fastverk.dev/capability`. This guide is
for the two sides of adoption — **producers** (make a backend discoverable) and
**consumers** (find one) — plus **cross-project** use (e.g. aion).

## Producer: make a backend discoverable

Stamp the backend's **Service** (nothing else — no CRD the finder must know):

```yaml
metadata:
  labels:
    finder.fastverk.dev/capability: ast-parser        # the List filter
  annotations:
    # advertised selector attributes (value = string or array)
    finder.fastverk.dev/selectors: '{"ext":[".rs",".rlib"],"language":"rust"}'
    # optional: pin an explicit URL (external / non-Service target); else the
    # named ports become http://<svc>.<ns>.svc.cluster.local:<port>
    # finder.fastverk.dev/endpoint: "https://graphd.example.com:443"
spec:
  ports:
    - name: grpc     # the port name a consumer asks for
      port: 50060
```

- **From an operator**: set the label + annotation in the reconciler that renders
  the Service (see the `LanguageParser` reconciler in fastverk-deploy — it stamps
  `capability=ast-parser` + `{ext,language}`).
- **From a Helm chart**: add them to the Service template's labels/annotations.

## Consumer, Rust — the client lib

Add the client crate (git-dep at a tag; no kube client, no RBAC):

```toml
[dependencies]
service-finder-client = { git = "https://github.com/fastverk/service-finder", tag = "service-finder-client-v0.0.1" }
```

```rust
use service_finder_client::Finder;

let finder = Finder::connect(
    std::env::var("FINDER_ADDR")
        .unwrap_or_else(|_| "http://service-finder.fastverk.svc.cluster.local:50060".into()),
);

// one-shot (empty result ⇒ fall back to your own default; it is NOT an error):
if let Some(url) = finder.resolve_one("ast-parser", &[("ext", ".rs")], "grpc").await? {
    // dial url
}

// live cache — discovery off the hot path, survives finder restarts:
let parsers = finder.watched("ast-parser", &[], "grpc");
let now = parsers.current();      // Arc<Vec<Endpoint>>, always last-known-good
```

Use `watched(...)` in a long-running service so a resolve is a synchronous
in-memory read; the background task keeps it fresh via `Watch` and reconnects with
backoff. This is the drop-in for the hand-rolled boot-once discovery in
`botnoc/web/src/discovery.rs`, `controlplane/src/rbe.rs`, `plugin-tbzl/src/graphd.rs`.

## Consumer, any other language — the gRPC contract

The proto (`proto/fastverk/finder/v1/finder.proto`) is small; inline it and call
`fastverk.finder.v1.Finder/Resolve` (or `/Watch`). See the klad ingest's
`codesearch/finder-client.ts` for a TypeScript (grpc-js) example that maintains a
`Watch`-backed cache and degrades gracefully when the finder is down. The finder
has server reflection, so `grpcurl` needs no proto:

```
grpcurl -plaintext -d '{"capability":"ast-parser","selector":{"ext":".rs"},"port_name":"grpc"}' \
  service-finder.fastverk.svc.cluster.local:50060 fastverk.finder.v1.Finder/Resolve
```

## Cross-project: using it from another project (aion)

The finder is **fleet-agnostic** — it is just a gRPC endpoint over labeled Services.
Nothing about it is fastverk-specific except where you point it and what it watches.
Three deployment models, pick by topology:

1. **One finder per fleet/namespace (recommended default).** Deploy a finder
   instance in the aion namespace with `discoveryNamespace: aion` (chart value); it
   watches aion's Services and keeps the namespaced Role (least privilege). aion
   services set `FINDER_ADDR=http://service-finder.aion.svc.cluster.local:50060` and
   use the same client lib / gRPC contract. Fastverk and aion each own their finder;
   no cross-namespace coupling.

2. **One cluster-wide finder (shared cluster).** Deploy a single finder with a
   `ClusterRole` (widen the chart's Role → ClusterRole) and an empty
   `discoveryNamespace` set to watch all namespaces. Every fleet resolves against it;
   keep capabilities namespaced by convention (e.g. `capability=aion.graphd`) so two
   fleets don't collide on a bare `graphd`. Simplest to operate, broadest RBAC.

3. **Separate cluster.** aion runs its own finder from this repo — the chart
   (`deploy/charts/service-finder`) and the client lib are reusable as-is. aion pins
   the image (or builds its own) and consumes the client crate as a git-dep. The
   `fastverk.finder.v1` contract is the shared interface across clusters.

**What aion needs, minimally:** (a) a finder instance reachable at some `FINDER_ADDR`
watching the namespace(s) its backends live in; (b) its discoverable Services stamped
with `finder.fastverk.dev/capability`; (c) the client lib (Rust) or the inlined proto
(other langs). No kube client in the consumer, no shared code beyond the proto.

> Not a fit: targets that are external URLs / credentials / secrets (OAuth,
> forge tokens) or backends already addressed by NATS subject-routing — resolve
> those with their existing mechanisms, not the finder.
