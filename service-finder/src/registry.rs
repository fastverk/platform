//! registry — the live cache of capability-labeled Services.
//!
//! A single kube `watcher` over Services carrying the `finder.fastverk.dev/
//! capability` label feeds a `reflector` Store, so `Resolve` is a pure in-memory
//! lookup (never a per-call k8s round-trip). Every watch event pings a broadcast
//! channel that `Watch` subscribers use to recompute their snapshot — this is the
//! `Watch` the in-process finders (discovery.rs polls only at boot) never had.

use std::collections::{BTreeMap, HashMap};

use futures::StreamExt;
use k8s_openapi::api::core::v1::Service;
use kube::runtime::{reflector, watcher, WatchStreamExt};
use kube::{Api, Client};
use tokio::sync::broadcast;

use crate::groups::GroupSpec;
use crate::pb::Endpoint;
use crate::resolver::{self, LabelAlias};

/// A cloneable handle to the shared Service cache + change notifier.
#[derive(Clone)]
pub struct Registry {
    store: reflector::Store<Service>,
    ns: String,
    aliases: Vec<LabelAlias>,
    tx: broadcast::Sender<()>,
}

impl Registry {
    /// Start the reflector over Services in `ns` and return a handle once the
    /// initial list has populated. The reflector runs in a background task for the
    /// process lifetime.
    pub async fn spawn(client: Client, ns: String, aliases: Vec<LabelAlias>) -> anyhow::Result<Self> {
        let api: Api<Service> = Api::namespaced(client, &ns);
        let (store, writer) = reflector::store();
        let (tx, _rx) = broadcast::channel(64);

        // Watch ALL Services (no label filter): a Service is discoverable via the
        // finder.fastverk.dev/capability label OR a legacy alias label (e.g.
        // fastverk.dev/plugin), and one selector can't OR across keys — so filter
        // in the resolver, not the watch. Services are lightweight; a namespace's
        // worth is a small cache. Backoff wraps the watcher (re-list re-syncs the
        // store after an API blip), then the reflector writes as events pass through.
        let cfg = watcher::Config::default();
        let tx_events = tx.clone();
        let watch_stream = watcher(api, cfg).default_backoff();
        let stream = reflector(writer, watch_stream).touched_objects();

        tokio::spawn(async move {
            futures::pin_mut!(stream);
            loop {
                match stream.next().await {
                    Some(Ok(svc)) => {
                        tracing::debug!(service = svc.metadata.name, "cache updated");
                        // A dropped ping only means a Watch subscriber resyncs late.
                        let _ = tx_events.send(());
                    }
                    Some(Err(e)) => tracing::warn!(error = %e, "service watcher error"),
                    None => {
                        tracing::warn!("service watcher stream ended; discovery cache is now static");
                        break;
                    }
                }
            }
        });

        store.wait_until_ready().await?;
        tracing::info!(namespace = %ns, aliases = aliases.len(), "registry ready");
        Ok(Self {
            store,
            ns,
            aliases,
            tx,
        })
    }

    /// Resolve the current matching endpoints (pure over the cache snapshot).
    pub fn resolve(
        &self,
        capability: &str,
        selector: &BTreeMap<String, String>,
        port_name: &str,
    ) -> Vec<Endpoint> {
        let state = self.store.state();
        resolver::resolve(
            state.iter().map(|arc| arc.as_ref()),
            &self.ns,
            &self.aliases,
            capability,
            selector,
            port_name,
        )
    }

    /// Resolve a named EndpointGroup: match on its capability + selector, then
    /// apply its ordering policy (priority read from each backing Service's
    /// annotation). Nothing is materialized — this is a live lookup like `resolve`.
    pub fn resolve_group(&self, spec: &GroupSpec) -> Vec<Endpoint> {
        let state = self.store.state();
        let endpoints = resolver::resolve(
            state.iter().map(|arc| arc.as_ref()),
            &self.ns,
            &self.aliases,
            &spec.capability,
            &spec.selector,
            &spec.port_name,
        );
        if spec.ordering == resolver::Ordering::Unordered {
            return endpoints; // resolve already returns a stable url sort
        }
        // Priority ordering: look each endpoint's backing Service up in the store
        // snapshot to read its priority annotation.
        let by_name: HashMap<&str, &Service> = state
            .iter()
            .filter_map(|arc| arc.metadata.name.as_deref().map(|n| (n, arc.as_ref())))
            .collect();
        resolver::order_endpoints(endpoints, &spec.ordering, |ep| {
            by_name
                .get(ep.service.as_str())
                .map(|svc| resolver::priority_of_service(svc))
                .unwrap_or(i64::MAX)
        })
    }

    /// Subscribe to cache-change notifications (one ping per watch event).
    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.tx.subscribe()
    }
}
