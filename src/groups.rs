//! groups — the EndpointGroup CR cache (the finder's policy layer).
//!
//! An `EndpointGroup` (finder.fastverk.dev/v1) is a NAMED, declarative match +
//! ordering policy: `Resolve("rust-parsers")` instead of encoding capability +
//! selector + ordering in every consumer. Crucially it holds POLICY, not endpoints
//! — the finder still resolves live against its Service cache and applies the
//! ordering at request time (nothing is materialized into etcd; that would just
//! re-create the render-a-registry anti-pattern the finder exists to remove).
//!
//! Groups are read via the kube dynamic API (no CRD codegen) and refreshed on a
//! short poll — they are policy objects edited by humans, so a few seconds of
//! latency is fine and a poll is far simpler/robuster than a dynamic reflector.
//! Absent CRD ⇒ empty cache ⇒ ResolveGroup just returns NotFound.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use kube::api::{Api, ApiResource, DynamicObject, GroupVersionKind, ListParams};
use kube::Client;
use tokio::sync::broadcast;

use crate::resolver::Ordering;

/// The resolved policy of one EndpointGroup CR.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupSpec {
    pub capability: String,
    pub selector: BTreeMap<String, String>,
    pub port_name: String,
    pub ordering: Ordering,
}

/// A live (polled) cache of EndpointGroup policies by name.
#[derive(Clone)]
pub struct GroupCache {
    groups: Arc<RwLock<HashMap<String, GroupSpec>>>,
    tx: broadcast::Sender<()>,
}

const POLL: Duration = Duration::from_secs(15);

fn group_api(client: Client, ns: &str) -> Api<DynamicObject> {
    let gvk = GroupVersionKind::gvk("finder.fastverk.dev", "v1", "EndpointGroup");
    let ar = ApiResource::from_gvk(&gvk);
    Api::namespaced_with(client, ns, &ar)
}

/// Parse one EndpointGroup CR into (name, spec). None if it's missing required
/// fields (a malformed group is skipped, never fatal).
fn parse_group(obj: &DynamicObject) -> Option<(String, GroupSpec)> {
    let name = obj.metadata.name.clone()?;
    let spec = obj.data.get("spec")?;
    let capability = spec.get("capability")?.as_str()?.to_string();
    let selector = spec
        .get("selector")
        .and_then(|v| v.as_object())
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default();
    let port_name = spec
        .get("portName")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let ordering = match spec
        .get("ordering")
        .and_then(|o| o.get("strategy"))
        .and_then(|v| v.as_str())
    {
        Some("Priority") => Ordering::Priority,
        _ => Ordering::Unordered,
    };
    Some((
        name,
        GroupSpec {
            capability,
            selector,
            port_name,
            ordering,
        },
    ))
}

async fn list_groups(api: &Api<DynamicObject>) -> HashMap<String, GroupSpec> {
    match api.list(&ListParams::default()).await {
        Ok(list) => list.items.iter().filter_map(parse_group).collect(),
        // CRD not installed / RBAC / transient ⇒ no groups (ResolveGroup 404s).
        Err(e) => {
            tracing::debug!(error = %e, "EndpointGroup list unavailable (CRD absent?)");
            HashMap::new()
        }
    }
}

impl GroupCache {
    /// Start the group poller and return once the initial list has loaded.
    pub async fn spawn(client: Client, ns: String) -> Self {
        let api = group_api(client, &ns);
        let groups = Arc::new(RwLock::new(list_groups(&api).await));
        let (tx, _rx) = broadcast::channel(16);
        {
            let n = groups.read().unwrap().len();
            tracing::info!(groups = n, "endpoint-group cache ready");
        }

        let groups_bg = groups.clone();
        let tx_bg = tx.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(POLL).await;
                let next = list_groups(&api).await;
                let changed = {
                    let cur = groups_bg.read().unwrap();
                    *cur != next
                };
                if changed {
                    *groups_bg.write().unwrap() = next;
                    let _ = tx_bg.send(());
                    tracing::info!("endpoint-group cache updated");
                }
            }
        });

        Self { groups, tx }
    }

    /// The policy for a named group, if it exists.
    pub fn get(&self, name: &str) -> Option<GroupSpec> {
        self.groups.read().unwrap().get(name).cloned()
    }

    /// Notified whenever the set of group policies changes.
    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.tx.subscribe()
    }
}
