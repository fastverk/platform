//! service-finder-client — dial the fastverk finder without a kube client.
//!
//! ```ignore
//! let finder = Finder::connect("http://service-finder.fastverk.svc.cluster.local:50060");
//! // one-shot:
//! let eps = finder.resolve("ast-parser", &[("ext", ".rs")], "grpc").await?;
//! // live cache (discovery off the hot path, survives finder downtime):
//! let parsers = finder.watched("ast-parser", &[], "grpc");
//! let now = parsers.current(); // Arc<Vec<Endpoint>>, always the last-known-good set
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio_stream::StreamExt;

/// Generated `fastverk.finder.v1` client + message types.
pub mod pb {
    tonic::include_proto!("fastverk.finder.v1");
}

pub use pb::Endpoint;
use pb::{finder_client::FinderClient, ResolveRequest, WatchRequest};

fn selector_map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// A lazily-connecting finder handle (cheap to clone). The channel connects on
/// first use (`connect_lazy`), so construction never fails and a finder that is
/// briefly unavailable just yields empty resolves until it returns.
#[derive(Clone)]
pub struct Finder {
    endpoint: String,
}

impl Finder {
    /// Create a handle for the finder at `endpoint` (e.g.
    /// `http://service-finder.fastverk.svc.cluster.local:50060`).
    pub fn connect(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
        }
    }

    async fn client(&self) -> anyhow::Result<FinderClient<tonic::transport::Channel>> {
        let channel = tonic::transport::Channel::from_shared(self.endpoint.clone())?
            .connect_timeout(Duration::from_secs(5))
            .connect_lazy();
        Ok(FinderClient::new(channel))
    }

    /// One-shot resolve. Returns an empty Vec (not an error) when nothing is
    /// registered — mirror the finder's semantics: fall back to your own default.
    pub async fn resolve(
        &self,
        capability: &str,
        selector: &[(&str, &str)],
        port_name: &str,
    ) -> anyhow::Result<Vec<Endpoint>> {
        let mut client = self.client().await?;
        let resp = client
            .resolve(ResolveRequest {
                capability: capability.to_string(),
                selector: selector_map(selector),
                port_name: port_name.to_string(),
            })
            .await?
            .into_inner();
        Ok(resp.endpoints)
    }

    /// Convenience: resolve and return the first endpoint's URL, if any.
    pub async fn resolve_one(
        &self,
        capability: &str,
        selector: &[(&str, &str)],
        port_name: &str,
    ) -> anyhow::Result<Option<String>> {
        Ok(self
            .resolve(capability, selector, port_name)
            .await?
            .into_iter()
            .next()
            .map(|e| e.url))
    }

    /// A live-updating, last-known-good cache for `(capability, selector)`. Spawns
    /// a background task that Watches the finder and keeps the latest snapshot;
    /// `Watched::current()` is a synchronous, non-failing read. On finder
    /// disconnect the last snapshot is retained and the task reconnects with
    /// backoff — so consumers never block on discovery.
    pub fn watched(&self, capability: &str, selector: &[(&str, &str)], port_name: &str) -> Watched {
        let (tx, rx) = watch::channel(Arc::new(Vec::<Endpoint>::new()));
        let this = self.clone();
        let capability = capability.to_string();
        let selector = selector_map(selector);
        let port_name = port_name.to_string();
        tokio::spawn(async move {
            let mut backoff = Duration::from_secs(1);
            loop {
                match this.run_watch(&capability, &selector, &port_name, &tx).await {
                    Ok(()) => backoff = Duration::from_secs(1),
                    Err(e) => {
                        tracing::warn!(error = %e, capability, "finder watch dropped; retaining last snapshot");
                    }
                }
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        });
        Watched { rx }
    }

    async fn run_watch(
        &self,
        capability: &str,
        selector: &HashMap<String, String>,
        port_name: &str,
        tx: &watch::Sender<Arc<Vec<Endpoint>>>,
    ) -> anyhow::Result<()> {
        let mut client = self.client().await?;
        let mut stream = client
            .watch(WatchRequest {
                capability: capability.to_string(),
                selector: selector.clone(),
                port_name: port_name.to_string(),
            })
            .await?
            .into_inner();
        while let Some(event) = stream.next().await {
            let event = event?;
            let _ = tx.send(Arc::new(event.endpoints));
        }
        Ok(())
    }
}

/// A synchronously-readable, always-populated view of a resolved capability.
#[derive(Clone)]
pub struct Watched {
    rx: watch::Receiver<Arc<Vec<Endpoint>>>,
}

impl Watched {
    /// The last-known-good endpoint set (empty until the first snapshot arrives).
    pub fn current(&self) -> Arc<Vec<Endpoint>> {
        self.rx.borrow().clone()
    }

    /// The first endpoint's URL, if any.
    pub fn first_url(&self) -> Option<String> {
        self.rx.borrow().first().map(|e| e.url.clone())
    }

    /// Await the next change to the endpoint set.
    pub async fn changed(&mut self) -> anyhow::Result<()> {
        self.rx.changed().await?;
        Ok(())
    }
}
