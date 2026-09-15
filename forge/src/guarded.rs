//! Guarded provisioning never falls back to unconditional mutations.
//!
//! Requests and receipts use the canonical protobuf contract. Status codes are
//! preserved so callers distinguish stale confirmation, denied access, and an
//! unsupported server. Authorization belongs to each caller-bound adapter.
use async_trait::async_trait;
use tonic::{
    metadata::{Ascii, MetadataValue},
    transport::Channel,
    Request, Status,
};

use crate::pb::guarded_forge_provision_service_client::GuardedForgeProvisionServiceClient;
pub use crate::pb::{
    GetProvisionMutationRequest, GetProvisionMutationResponse, GetProvisionSnapshotRequest,
    GetProvisionSnapshotResponse, GuardedArchiveRepoRequest, GuardedArchiveRepoResponse,
    GuardedDeleteRepoRequest, GuardedDeleteRepoResponse, GuardedEnsureProtectionRequest,
    GuardedEnsureProtectionResponse,
};

/// Optional guarded capability. Defaults reject explicitly, never emulate a
/// precondition using a separate read followed by an unconditional write.
#[async_trait]
pub trait GuardedProvisioner: Send + Sync {
    async fn snapshot(
        &self,
        _request: GetProvisionSnapshotRequest,
    ) -> Result<GetProvisionSnapshotResponse, Status> {
        Err(Status::unimplemented(
            "guarded provisioning snapshots are unsupported",
        ))
    }
    async fn archive(
        &self,
        _request: GuardedArchiveRepoRequest,
    ) -> Result<GuardedArchiveRepoResponse, Status> {
        Err(Status::unimplemented("guarded archive is unsupported"))
    }
    async fn delete(
        &self,
        _request: GuardedDeleteRepoRequest,
    ) -> Result<GuardedDeleteRepoResponse, Status> {
        Err(Status::unimplemented("guarded deletion is unsupported"))
    }
    async fn ensure_protection(
        &self,
        _request: GuardedEnsureProtectionRequest,
    ) -> Result<GuardedEnsureProtectionResponse, Status> {
        Err(Status::unimplemented("guarded protection is unsupported"))
    }
    async fn mutation(
        &self,
        _request: GetProvisionMutationRequest,
    ) -> Result<GetProvisionMutationResponse, Status> {
        Err(Status::unimplemented(
            "guarded operation lookup is unsupported",
        ))
    }
}

/// Native Geetch passthrough bound to one caller's Authorization metadata.
/// Create another instance when the caller changes; credentials are never part
/// of request protobufs or receipt identity supplied by the client.
#[derive(Clone)]
pub struct GuardedGeetchProvisioner {
    channel: Channel,
    authorization: MetadataValue<Ascii>,
}

impl GuardedGeetchProvisioner {
    pub fn new(channel: Channel, mut authorization: MetadataValue<Ascii>) -> Self {
        authorization.set_sensitive(true);
        Self {
            channel,
            authorization,
        }
    }
    fn request<T>(&self, message: T) -> Request<T> {
        let mut request = Request::new(message);
        request
            .metadata_mut()
            .insert("authorization", self.authorization.clone());
        request
    }
    fn client(&self) -> GuardedForgeProvisionServiceClient<Channel> {
        GuardedForgeProvisionServiceClient::new(self.channel.clone())
    }
}

#[async_trait]
impl GuardedProvisioner for GuardedGeetchProvisioner {
    async fn snapshot(
        &self,
        request: GetProvisionSnapshotRequest,
    ) -> Result<GetProvisionSnapshotResponse, Status> {
        Ok(self
            .client()
            .get_provision_snapshot(self.request(request))
            .await?
            .into_inner())
    }
    async fn archive(
        &self,
        request: GuardedArchiveRepoRequest,
    ) -> Result<GuardedArchiveRepoResponse, Status> {
        Ok(self
            .client()
            .guarded_archive_repo(self.request(request))
            .await?
            .into_inner())
    }
    async fn delete(
        &self,
        request: GuardedDeleteRepoRequest,
    ) -> Result<GuardedDeleteRepoResponse, Status> {
        Ok(self
            .client()
            .guarded_delete_repo(self.request(request))
            .await?
            .into_inner())
    }
    async fn ensure_protection(
        &self,
        request: GuardedEnsureProtectionRequest,
    ) -> Result<GuardedEnsureProtectionResponse, Status> {
        Ok(self
            .client()
            .guarded_ensure_protection(self.request(request))
            .await?
            .into_inner())
    }
    async fn mutation(
        &self,
        request: GetProvisionMutationRequest,
    ) -> Result<GetProvisionMutationResponse, Status> {
        Ok(self
            .client()
            .get_provision_mutation(self.request(request))
            .await?
            .into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Unsupported;
    impl GuardedProvisioner for Unsupported {}

    #[tokio::test]
    async fn optional_guarded_operations_fail_closed() {
        let adapter: &dyn GuardedProvisioner = &Unsupported;
        assert_eq!(
            adapter
                .snapshot(Default::default())
                .await
                .unwrap_err()
                .code(),
            tonic::Code::Unimplemented
        );
        assert_eq!(
            adapter
                .archive(Default::default())
                .await
                .unwrap_err()
                .code(),
            tonic::Code::Unimplemented
        );
        assert_eq!(
            adapter.delete(Default::default()).await.unwrap_err().code(),
            tonic::Code::Unimplemented
        );
        assert_eq!(
            adapter
                .ensure_protection(Default::default())
                .await
                .unwrap_err()
                .code(),
            tonic::Code::Unimplemented
        );
        assert_eq!(
            adapter
                .mutation(Default::default())
                .await
                .unwrap_err()
                .code(),
            tonic::Code::Unimplemented
        );
    }
    struct RejectingServer {
        expected: GuardedDeleteRepoRequest,
        checked: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }
    #[tonic::async_trait]
    impl crate::pb::guarded_forge_provision_service_server::GuardedForgeProvisionService
        for RejectingServer
    {
        async fn get_provision_snapshot(
            &self,
            _request: Request<GetProvisionSnapshotRequest>,
        ) -> Result<tonic::Response<GetProvisionSnapshotResponse>, Status> {
            Err(Status::unimplemented("not implemented by this server"))
        }
        async fn guarded_archive_repo(
            &self,
            _request: Request<GuardedArchiveRepoRequest>,
        ) -> Result<tonic::Response<GuardedArchiveRepoResponse>, Status> {
            Err(Status::unimplemented("not implemented by this server"))
        }
        async fn guarded_ensure_protection(
            &self,
            _request: Request<GuardedEnsureProtectionRequest>,
        ) -> Result<tonic::Response<GuardedEnsureProtectionResponse>, Status> {
            Err(Status::unimplemented("not implemented by this server"))
        }
        async fn get_provision_mutation(
            &self,
            _request: Request<GetProvisionMutationRequest>,
        ) -> Result<tonic::Response<GetProvisionMutationResponse>, Status> {
            Err(Status::unimplemented("not implemented by this server"))
        }
        async fn guarded_delete_repo(
            &self,
            request: Request<GuardedDeleteRepoRequest>,
        ) -> Result<tonic::Response<GuardedDeleteRepoResponse>, Status> {
            if request
                .metadata()
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                != Some("Bearer fixture-caller")
            {
                return Err(Status::permission_denied("caller lacks access"));
            }
            assert_eq!(
                request.into_inner(),
                self.expected,
                "adapter must preserve every confirmation field"
            );
            self.checked
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(Status::failed_precondition("repository revision changed"))
        }
    }

    #[tokio::test]
    async fn grpc_adapter_preserves_caller_preconditions_and_failure_codes() {
        use crate::pb::guarded_forge_provision_service_server::GuardedForgeProvisionServiceServer;
        use crate::pb::{ProvisionRevision, RepoRef};
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        let expected = GuardedDeleteRepoRequest {
            repo: Some(RepoRef {
                forge: 3,
                owner: "acme".into(),
                name: "widgets".into(),
                host: String::new(),
            }),
            expected: Some(ProvisionRevision {
                incarnation: "original".into(),
                configuration: "version-7".into(),
            }),
            confirm_name: "widgets".into(),
            idempotency_key: "delete-once".into(),
        };
        let checked = Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let service = RejectingServer {
            expected: expected.clone(),
            checked: Arc::clone(&checked),
        };
        let server = tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(GuardedForgeProvisionServiceServer::new(service))
                .serve_with_incoming_shutdown(
                    tokio_stream::wrappers::TcpListenerStream::new(listener),
                    async {
                        let _ = stopped.await;
                    },
                )
                .await
                .unwrap();
        });
        let channel = Channel::from_shared(format!("http://{addr}"))
            .unwrap()
            .connect()
            .await
            .unwrap();
        let adapter = GuardedGeetchProvisioner::new(
            channel.clone(),
            "Bearer fixture-caller".parse().unwrap(),
        );
        assert!(adapter.authorization.is_sensitive());
        let failure = adapter.delete(expected.clone()).await.unwrap_err();
        assert_eq!(failure.code(), tonic::Code::FailedPrecondition);
        assert_eq!(failure.message(), "repository revision changed");
        assert_eq!(
            checked.load(Ordering::SeqCst),
            1,
            "no automatic retry after a stale confirmation"
        );
        let denied =
            GuardedGeetchProvisioner::new(channel, "Bearer different-caller".parse().unwrap());
        assert_eq!(
            denied.delete(expected).await.unwrap_err().code(),
            tonic::Code::PermissionDenied
        );
        assert_eq!(
            checked.load(Ordering::SeqCst),
            1,
            "caller credentials must not leak between instances"
        );
        assert_eq!(
            adapter
                .archive(Default::default())
                .await
                .unwrap_err()
                .code(),
            tonic::Code::Unimplemented
        );
        stop.send(()).unwrap();
        server.await.unwrap();
    }
}
