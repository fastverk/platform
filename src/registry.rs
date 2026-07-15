//! registry — the live cache of capability-labeled Services.
//!
//! A single kube `watcher` over Services carrying the `finder.fastverk.dev/
//! capability` label feeds a `reflector` Store, so `Resolve` is a pure in-memory
//! lookup (never a per-call k8s round-trip). Every watch event pings a broadcast
//! channel that `Watch` subscribers use to recompute their snapshot — this is the
//! `Watch` the in-process finders (discovery.rs polls only at boot) never had.

use std::collections::BTreeMap;

use futures::StreamExt;
use k8s_openapi::api::core::v1::Service;
use kube::runtime::{reflector, watcher, WatchStreamExt};
use kube::{Api, Client};
use tokio::sync::broadcast;

use crate::pb::Endpoint;
use crate::resolver;

/// A cloneable handle to the shared Service cache + change notifier.
#[derive(Clone)]
pub struct Registry {
    store: reflector::Store<Service>,
    ns: String,
    tx: broadcast::Sender<()>,
}

impl Registry {
    /// Start the reflector over capability-labeled Services in `ns` and return a
    /// handle once the initial list has populated. The reflector runs in a
    /// background task for the process lifetime.
    pub async fn spawn(client: Client, ns: String) -> anyhow::Result<Self> {
        let api: Api<Service> = Api::namespaced(client, &ns);
        let (store, writer) = reflector::store();
        let (tx, _rx) = broadcast::channel(64);

        // "label exists" selector — every Service that opts into discovery,
        // regardless of which capability value it carries. Backoff wraps the
        // watcher (so a re-list after an API blip re-syncs the store cleanly),
        // then the reflector writes the store as events pass through.
        let cfg = watcher::Config::default().labels(resolver::CAPABILITY_LABEL);
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
        tracing::info!(namespace = %ns, "registry ready");
        Ok(Self { store, ns, tx })
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
            capability,
            selector,
            port_name,
        )
    }

    /// Subscribe to cache-change notifications (one ping per watch event).
    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.tx.subscribe()
    }
}
