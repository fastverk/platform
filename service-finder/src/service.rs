//! service — the `fastverk.finder.v1.Finder` gRPC surface over the Registry.

use std::collections::BTreeMap;
use std::pin::Pin;

use futures::Stream;
use tokio::sync::broadcast::error::RecvError;
use tonic::{Request, Response, Status};

use crate::groups::GroupCache;
use crate::pb::finder_server::Finder;
use crate::pb::{
    watch_event, ResolveGroupRequest, ResolveGroupResponse, ResolveRequest, ResolveResponse,
    WatchEvent, WatchGroupRequest, WatchRequest,
};
use crate::registry::Registry;

pub struct FinderService {
    reg: Registry,
    groups: GroupCache,
}

impl FinderService {
    pub fn new(reg: Registry, groups: GroupCache) -> Self {
        Self { reg, groups }
    }
}

type WatchResult = Result<WatchEvent, Status>;

#[tonic::async_trait]
impl Finder for FinderService {
    async fn resolve(
        &self,
        request: Request<ResolveRequest>,
    ) -> Result<Response<ResolveResponse>, Status> {
        let req = request.into_inner();
        if req.capability.is_empty() {
            return Err(Status::invalid_argument("capability is required"));
        }
        let selector: BTreeMap<String, String> = req.selector.into_iter().collect();
        let endpoints = self.reg.resolve(&req.capability, &selector, &req.port_name);
        tracing::debug!(
            capability = %req.capability,
            selector = ?selector,
            port = %req.port_name,
            matched = endpoints.len(),
            "resolve",
        );
        Ok(Response::new(ResolveResponse { endpoints }))
    }

    type WatchStream = Pin<Box<dyn Stream<Item = WatchResult> + Send + 'static>>;

    async fn watch(
        &self,
        request: Request<WatchRequest>,
    ) -> Result<Response<Self::WatchStream>, Status> {
        let req = request.into_inner();
        if req.capability.is_empty() {
            return Err(Status::invalid_argument("capability is required"));
        }
        let selector: BTreeMap<String, String> = req.selector.into_iter().collect();
        let reg = self.reg.clone();
        let mut rx = reg.subscribe();

        let stream = async_stream::stream! {
            // Initial full snapshot. The explicit `Ok::<_, Status>` pins the
            // stream's error type (nothing inside uses `?`, so it can't infer it).
            let mut last = reg.resolve(&req.capability, &selector, &req.port_name);
            yield Ok::<WatchEvent, Status>(WatchEvent {
                r#type: watch_event::Type::Snapshot as i32,
                endpoints: last.clone(),
            });
            // Then push the new full set on every change (skip no-op events so a
            // subscriber to capability A isn't woken by an unrelated capability B).
            loop {
                match rx.recv().await {
                    Ok(()) | Err(RecvError::Lagged(_)) => {
                        let current = reg.resolve(&req.capability, &selector, &req.port_name);
                        if current != last {
                            last = current.clone();
                            yield Ok(WatchEvent {
                                r#type: watch_event::Type::Changed as i32,
                                endpoints: current,
                            });
                        }
                    }
                    Err(RecvError::Closed) => break,
                }
            }
        };

        Ok(Response::new(Box::pin(stream)))
    }

    async fn resolve_group(
        &self,
        request: Request<ResolveGroupRequest>,
    ) -> Result<Response<ResolveGroupResponse>, Status> {
        let name = request.into_inner().name;
        if name.is_empty() {
            return Err(Status::invalid_argument("name is required"));
        }
        let Some(spec) = self.groups.get(&name) else {
            return Err(Status::not_found(format!("no EndpointGroup {name:?}")));
        };
        let endpoints = self.reg.resolve_group(&spec);
        tracing::debug!(group = %name, ordering = ?spec.ordering, matched = endpoints.len(), "resolve_group");
        Ok(Response::new(ResolveGroupResponse { endpoints }))
    }

    type WatchGroupStream = Pin<Box<dyn Stream<Item = WatchResult> + Send + 'static>>;

    async fn watch_group(
        &self,
        request: Request<WatchGroupRequest>,
    ) -> Result<Response<Self::WatchGroupStream>, Status> {
        let name = request.into_inner().name;
        if name.is_empty() {
            return Err(Status::invalid_argument("name is required"));
        }
        let reg = self.reg.clone();
        let groups = self.groups.clone();
        // Wake on EITHER the Service cache changing OR the group's own policy
        // changing (a group can appear/disappear/retune after subscribe).
        let mut svc_rx = reg.subscribe();
        let mut grp_rx = groups.subscribe();

        let stream = async_stream::stream! {
            let snapshot = groups.get(&name).map(|s| reg.resolve_group(&s)).unwrap_or_default();
            let mut last = snapshot.clone();
            yield Ok::<WatchEvent, Status>(WatchEvent {
                r#type: watch_event::Type::Snapshot as i32,
                endpoints: snapshot,
            });
            loop {
                tokio::select! {
                    r = svc_rx.recv() => if matches!(r, Err(RecvError::Closed)) { break; },
                    r = grp_rx.recv() => if matches!(r, Err(RecvError::Closed)) { break; },
                }
                let current = groups.get(&name).map(|s| reg.resolve_group(&s)).unwrap_or_default();
                if current != last {
                    last = current.clone();
                    yield Ok(WatchEvent {
                        r#type: watch_event::Type::Changed as i32,
                        endpoints: current,
                    });
                }
            }
        };

        Ok(Response::new(Box::pin(stream)))
    }
}
