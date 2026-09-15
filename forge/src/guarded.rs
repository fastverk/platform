//! Guarded provisioning never falls back to unconditional mutations.
//!
//! Requests and receipts use the canonical protobuf contract. Status codes are
//! preserved so callers distinguish stale confirmation, denied access, and an
//! unsupported server. Authorization belongs to each caller-bound adapter.
use async_trait::async_trait;
use tonic::{
    metadata::{Ascii, MetadataValue},
    transport::Channel,
    Request,
};
pub use tonic::{Code, Status};

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
        let response = self
            .client()
            .get_provision_snapshot(self.request(request.clone()))
            .await?
            .into_inner();
        if !snapshot_matches(&response, &request) {
            return Err(Status::data_loss("invalid guarded repository snapshot"));
        }
        Ok(response)
    }
    async fn archive(
        &self,
        request: GuardedArchiveRepoRequest,
    ) -> Result<GuardedArchiveRepoResponse, Status> {
        let response = self
            .client()
            .guarded_archive_repo(self.request(request.clone()))
            .await?
            .into_inner();
        let expected = crate::pb::provision_mutation_receipt::Request::Archive(request);
        if !response
            .receipt
            .as_ref()
            .is_some_and(|r| receipt_valid(r) && r.request.as_ref() == Some(&expected))
        {
            return Err(Status::data_loss("invalid guarded mutation receipt"));
        }
        Ok(response)
    }
    async fn delete(
        &self,
        request: GuardedDeleteRepoRequest,
    ) -> Result<GuardedDeleteRepoResponse, Status> {
        let response = self
            .client()
            .guarded_delete_repo(self.request(request.clone()))
            .await?
            .into_inner();
        let expected = crate::pb::provision_mutation_receipt::Request::Delete(request);
        if !response
            .receipt
            .as_ref()
            .is_some_and(|r| receipt_valid(r) && r.request.as_ref() == Some(&expected))
        {
            return Err(Status::data_loss("invalid guarded mutation receipt"));
        }
        Ok(response)
    }
    async fn ensure_protection(
        &self,
        request: GuardedEnsureProtectionRequest,
    ) -> Result<GuardedEnsureProtectionResponse, Status> {
        let response = self
            .client()
            .guarded_ensure_protection(self.request(request.clone()))
            .await?
            .into_inner();
        let expected = crate::pb::provision_mutation_receipt::Request::Protection(request);
        if !response
            .receipt
            .as_ref()
            .is_some_and(|r| receipt_valid(r) && r.request.as_ref() == Some(&expected))
        {
            return Err(Status::data_loss("invalid guarded mutation receipt"));
        }
        Ok(response)
    }
    async fn mutation(
        &self,
        request: GetProvisionMutationRequest,
    ) -> Result<GetProvisionMutationResponse, Status> {
        let response = self
            .client()
            .get_provision_mutation(self.request(request.clone()))
            .await?
            .into_inner();
        if !response.receipt.as_ref().is_some_and(|r| {
            receipt_valid(r)
                && r.request.as_ref().is_some_and(|input| {
                    let (repo, revision, key) = receipt_identity(input);
                    repo == &request.repo
                        && revision
                            .as_ref()
                            .is_some_and(|v| v.incarnation == request.incarnation)
                        && key == request.idempotency_key
                })
        }) {
            return Err(Status::data_loss(
                "guarded lookup returned an unrelated or invalid receipt",
            ));
        }
        Ok(response)
    }
}

fn revision_valid(value: &Option<crate::pb::ProvisionRevision>) -> bool {
    value
        .as_ref()
        .is_some_and(|v| !v.incarnation.is_empty() && !v.configuration.is_empty())
}
fn snapshot_matches(
    response: &GetProvisionSnapshotResponse,
    request: &GetProvisionSnapshotRequest,
) -> bool {
    if response.branch != request.branch {
        return false;
    }
    if !response.found {
        return response.repo.is_none()
            && response.revision.is_none()
            && !response.protection_found
            && response.protection.is_none();
    }
    request.repo.is_some()
        && response
            .repo
            .as_ref()
            .is_some_and(|r| r.repo == request.repo)
        && revision_valid(&response.revision)
        && response.protection_found == response.protection.is_some()
        && response
            .protection
            .as_ref()
            .is_none_or(|p| !request.branch.is_empty() && p.branch == request.branch)
}
fn receipt_identity(
    input: &crate::pb::provision_mutation_receipt::Request,
) -> (
    &Option<crate::pb::RepoRef>,
    &Option<crate::pb::ProvisionRevision>,
    &str,
) {
    use crate::pb::provision_mutation_receipt::Request::*;
    match input {
        Archive(r) => (&r.repo, &r.expected, &r.idempotency_key),
        Delete(r) => (&r.repo, &r.expected, &r.idempotency_key),
        Protection(r) => (&r.repo, &r.expected, &r.idempotency_key),
    }
}
fn receipt_valid(receipt: &crate::pb::ProvisionMutationReceipt) -> bool {
    use crate::pb::{
        provision_mutation_receipt::{Request as Input, Result as Output},
        ProvisionMutationPhase as Phase,
    };
    let Some(input) = receipt.request.as_ref() else {
        return false;
    };
    let (repo, expected, key) = receipt_identity(input);
    if receipt.operation_id.is_empty()
        || !receipt
            .actor
            .as_ref()
            .is_some_and(|a| !a.issuer.is_empty() && !a.subject.is_empty())
        || repo.is_none()
        || !revision_valid(expected)
        || key.is_empty()
    {
        return false;
    }
    match Phase::try_from(receipt.phase) {
        Ok(Phase::Pending | Phase::Failed | Phase::ReconciliationRequired) => {
            receipt.result.is_none() && receipt.resulting_revision.is_none()
        }
        Ok(Phase::Succeeded) => {
            let surviving_revision = revision_valid(&receipt.resulting_revision)
                && receipt.resulting_revision.as_ref().map(|v| &v.incarnation)
                    == expected.as_ref().map(|v| &v.incarnation);
            match (input, receipt.result.as_ref()) {
                (Input::Archive(_), Some(Output::Archived(r))) => {
                    surviving_revision
                        && r.repo.as_ref().is_some_and(|r| {
                            &r.repo == repo
                                && r.lifecycle == crate::pb::RepoLifecycle::Archived as i32
                        })
                }
                (Input::Delete(r), Some(Output::Deleted(result))) => {
                    result.confirmed
                        && receipt.resulting_revision.is_none()
                        && repo
                            .as_ref()
                            .is_some_and(|repo| r.confirm_name == repo.name)
                }
                (Input::Protection(r), Some(Output::ProtectionSaved(result))) => {
                    surviving_revision
                        && result
                            .protection
                            .as_ref()
                            .is_some_and(|p| p.branch == r.branch)
                }
                _ => false,
            }
        }
        _ => false,
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
    #[test]
    fn snapshots_reject_wrong_identity_or_incomplete_versions() {
        use crate::pb::{Protection, ProvisionRevision, ProvisionedRepo, RepoRef};
        let request = GetProvisionSnapshotRequest {
            repo: Some(RepoRef {
                forge: 3,
                owner: "acme".into(),
                name: "widgets".into(),
                host: String::new(),
            }),
            branch: "main".into(),
        };
        let valid = GetProvisionSnapshotResponse {
            found: true,
            repo: Some(ProvisionedRepo {
                repo: request.repo.clone(),
                ..Default::default()
            }),
            revision: Some(ProvisionRevision {
                incarnation: "original".into(),
                configuration: "7".into(),
            }),
            branch: "main".into(),
            protection_found: false,
            protection: None,
        };
        assert!(snapshot_matches(&valid, &request));
        let mut wrong = valid.clone();
        wrong.repo.as_mut().unwrap().repo.as_mut().unwrap().name = "other".into();
        assert!(!snapshot_matches(&wrong, &request));
        wrong = valid.clone();
        wrong.revision = None;
        assert!(!snapshot_matches(&wrong, &request));
        wrong = valid.clone();
        wrong.protection_found = true;
        assert!(!snapshot_matches(&wrong, &request));
        wrong.protection = Some(Protection {
            branch: "other".into(),
            ..Default::default()
        });
        assert!(!snapshot_matches(&wrong, &request));
        wrong = valid.clone();
        wrong.found = false;
        assert!(!snapshot_matches(&wrong, &request));
        assert!(snapshot_matches(
            &GetProvisionSnapshotResponse {
                branch: "main".into(),
                ..Default::default()
            },
            &request
        ));
    }

    #[test]
    fn successful_delete_receipts_require_confirmed_matching_outcomes() {
        use crate::pb::provision_mutation_receipt::{Request as Input, Result as Output};
        use crate::pb::{
            DeletedProvisionResult, ProvisionActor, ProvisionMutationPhase,
            ProvisionMutationReceipt, ProvisionRevision, RepoRef,
        };
        let valid = ProvisionMutationReceipt {
            operation_id: "operation-1".into(),
            actor: Some(ProvisionActor {
                issuer: "issuer".into(),
                subject: "caller".into(),
            }),
            request: Some(Input::Delete(GuardedDeleteRepoRequest {
                repo: Some(RepoRef {
                    forge: 3,
                    owner: "acme".into(),
                    name: "widgets".into(),
                    host: String::new(),
                }),
                expected: Some(ProvisionRevision {
                    incarnation: "original".into(),
                    configuration: "7".into(),
                }),
                confirm_name: "widgets".into(),
                idempotency_key: "delete-once".into(),
            })),
            phase: ProvisionMutationPhase::Succeeded as i32,
            result: Some(Output::Deleted(DeletedProvisionResult { confirmed: true })),
            ..Default::default()
        };
        assert!(receipt_valid(&valid));
        let mut wrong = valid.clone();
        wrong.result = None;
        assert!(!receipt_valid(&wrong));
        wrong = valid.clone();
        wrong.phase = 999;
        assert!(!receipt_valid(&wrong));
        wrong = valid.clone();
        wrong.actor = None;
        assert!(!receipt_valid(&wrong));
        wrong = valid.clone();
        wrong.result = Some(Output::Deleted(DeletedProvisionResult { confirmed: false }));
        assert!(!receipt_valid(&wrong));
        wrong = valid.clone();
        wrong.resulting_revision = Some(ProvisionRevision {
            incarnation: "replacement".into(),
            configuration: "1".into(),
        });
        assert!(!receipt_valid(&wrong));
        wrong = valid.clone();
        wrong.phase = ProvisionMutationPhase::Pending as i32;
        assert!(
            !receipt_valid(&wrong),
            "pending must not carry a confirmed outcome"
        );
        wrong.result = None;
        assert!(
            receipt_valid(&wrong),
            "pending remains an explicit non-success state"
        );
        wrong = valid;
        if let Some(Input::Delete(r)) = &mut wrong.request {
            r.confirm_name = "different".into();
        }
        assert!(!receipt_valid(&wrong));
    }
}
