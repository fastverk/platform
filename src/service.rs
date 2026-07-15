//! service — the `fastverk.finder.v1.Finder` gRPC surface over the Registry.

use std::collections::BTreeMap;
use std::pin::Pin;

use futures::Stream;
use tokio::sync::broadcast::error::RecvError;
use tonic::{Request, Response, Status};

use crate::pb::finder_server::Finder;
use crate::pb::{
    watch_event, ResolveRequest, ResolveResponse, WatchEvent, WatchRequest,
};
use crate::registry::Registry;

pub struct FinderService {
    reg: Registry,
}

impl FinderService {
    pub fn new(reg: Registry) -> Self {
        Self { reg }
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
}
