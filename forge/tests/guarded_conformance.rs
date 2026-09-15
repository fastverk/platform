//! Exercise the shared lifecycle contract against an atomic in-memory model.
//! This validates the suite, not Geetch's persistence or restart recovery.
use async_trait::async_trait;
use forge::{guarded::*, guarded_conformance::Fixture, pb};
use pb::provision_mutation_receipt::{Request as Input, Result as Output};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};
use tonic::Status;

struct Model {
    pending_first: AtomicBool,
    repo: pb::RepoRef,
    state: Mutex<State>,
}
struct State {
    generation: u64,
    incarnation: u64,
    snapshot: pb::GetProvisionSnapshotResponse,
    receipts: HashMap<String, pb::ProvisionMutationReceipt>,
}
impl Model {
    fn new() -> Self {
        let repo = pb::RepoRef {
            owner: "conformance".into(),
            name: "repository".into(),
            ..Default::default()
        };
        Self {
            pending_first: AtomicBool::new(false),
            repo: repo.clone(),
            state: Mutex::new(State {
                generation: 1,
                incarnation: 1,
                snapshot: pb::GetProvisionSnapshotResponse {
                    found: true,
                    repo: Some(pb::ProvisionedRepo {
                        repo: Some(repo),
                        lifecycle: pb::RepoLifecycle::Active as i32,
                        ..Default::default()
                    }),
                    revision: Some(pb::ProvisionRevision {
                        incarnation: "incarnation-1".into(),
                        configuration: "configuration-1".into(),
                    }),
                    ..Default::default()
                },
                receipts: HashMap::new(),
            }),
        }
    }
    fn initially_pending() -> Self {
        let model = Self::new();
        model.pending_first.store(true, Ordering::SeqCst);
        model
    }
    fn response(&self, mut receipt: pb::ProvisionMutationReceipt) -> pb::ProvisionMutationReceipt {
        if self.pending_first.swap(false, Ordering::SeqCst) {
            receipt.phase = pb::ProvisionMutationPhase::Pending as i32;
            receipt.result = None;
            receipt.resulting_revision = None;
        }
        receipt
    }
    fn execute(&self, input: Input) -> Result<pb::ProvisionMutationReceipt, Status> {
        let (repo, expected, key) = match &input {
            Input::Archive(r) => (&r.repo, &r.expected, &r.idempotency_key),
            Input::Delete(r) => {
                if r.confirm_name != self.repo.name {
                    return Err(Status::invalid_argument("exact name required"));
                }
                (&r.repo, &r.expected, &r.idempotency_key)
            }
            Input::Protection(_) => {
                return Err(Status::unimplemented("protection model is not implemented"))
            }
        };
        if repo.as_ref() != Some(&self.repo) {
            return Err(Status::permission_denied("repository not granted"));
        }
        if key.is_empty()
            || !expected
                .as_ref()
                .is_some_and(|v| !v.incarnation.is_empty() && !v.configuration.is_empty())
        {
            return Err(Status::invalid_argument("complete preconditions required"));
        }
        let mut state = self.state.lock().unwrap();
        if let Some(receipt) = state.receipts.get(key) {
            return if receipt.request.as_ref() == Some(&input) {
                Ok(receipt.clone())
            } else {
                Err(Status::already_exists("key already used"))
            };
        }
        if state.snapshot.revision != *expected {
            return Err(Status::failed_precondition("revision changed"));
        }
        let (result, resulting_revision) = match &input {
            Input::Archive(_) => {
                state.snapshot.repo.as_mut().unwrap().lifecycle =
                    pb::RepoLifecycle::Archived as i32;
                state.generation += 1;
                let n = state.generation;
                state.snapshot.revision.as_mut().unwrap().configuration =
                    format!("configuration-{n}");
                (
                    Output::Archived(pb::ArchivedProvisionResult {
                        repo: state.snapshot.repo.clone(),
                    }),
                    state.snapshot.revision.clone(),
                )
            }
            Input::Delete(_) => {
                state.snapshot = pb::GetProvisionSnapshotResponse::default();
                (
                    Output::Deleted(pb::DeletedProvisionResult { confirmed: true }),
                    None,
                )
            }
            Input::Protection(_) => unreachable!(),
        };
        let key = key.clone();
        let receipt = pb::ProvisionMutationReceipt {
            operation_id: format!("operation-{}", state.receipts.len() + 1),
            actor: Some(pb::ProvisionActor {
                issuer: "fixture".into(),
                subject: "caller".into(),
            }),
            request: Some(input),
            phase: pb::ProvisionMutationPhase::Succeeded as i32,
            resulting_revision,
            result: Some(result),
            created_at_unix_seconds: 1,
            updated_at_unix_seconds: 1,
            ..Default::default()
        };
        state.receipts.insert(key, receipt.clone());
        Ok(receipt)
    }
}
#[async_trait]
impl GuardedProvisioner for Model {
    async fn snapshot(
        &self,
        request: GetProvisionSnapshotRequest,
    ) -> Result<GetProvisionSnapshotResponse, Status> {
        if request.repo.as_ref() != Some(&self.repo) {
            return Err(Status::permission_denied("repository not granted"));
        }
        let mut snapshot = self.state.lock().unwrap().snapshot.clone();
        snapshot.branch = request.branch;
        if snapshot.branch != "main" {
            snapshot.protection = None;
            snapshot.protection_found = false;
        }
        Ok(snapshot)
    }
    async fn archive(
        &self,
        request: GuardedArchiveRepoRequest,
    ) -> Result<GuardedArchiveRepoResponse, Status> {
        // Allow competing futures to overlap before the serialization boundary.
        tokio::task::yield_now().await;
        Ok(GuardedArchiveRepoResponse {
            receipt: Some(self.response(self.execute(Input::Archive(request))?)),
        })
    }
    async fn delete(
        &self,
        request: GuardedDeleteRepoRequest,
    ) -> Result<GuardedDeleteRepoResponse, Status> {
        Ok(GuardedDeleteRepoResponse {
            receipt: Some(self.response(self.execute(Input::Delete(request))?)),
        })
    }
    async fn mutation(
        &self,
        request: GetProvisionMutationRequest,
    ) -> Result<GetProvisionMutationResponse, Status> {
        if request.repo.as_ref() != Some(&self.repo) {
            return Err(Status::permission_denied("repository not granted"));
        }
        let state = self.state.lock().unwrap();
        let receipt = state
            .receipts
            .get(&request.idempotency_key)
            .ok_or_else(|| Status::not_found("unknown operation"))?;
        let expected = match receipt.request.as_ref().unwrap() {
            Input::Archive(r) => &r.expected,
            Input::Delete(r) => &r.expected,
            Input::Protection(r) => &r.expected,
        };
        if expected.as_ref().unwrap().incarnation != request.incarnation {
            return Err(Status::not_found("unknown incarnation"));
        }
        Ok(GetProvisionMutationResponse {
            receipt: Some(receipt.clone()),
        })
    }
}
impl Fixture for Model {
    fn provisioner(&self) -> &dyn GuardedProvisioner {
        self
    }
    fn repo(&self) -> pb::RepoRef {
        self.repo.clone()
    }
}
forge::guarded_conformance_suite!(atomic_model, Model::new());

forge::guarded_conformance_suite!(pending_model, Model::initially_pending());

#[async_trait]
impl forge::guarded_conformance::RevisionFixture for Model {
    async fn legacy_protection(&self, spec: pb::ProtectionSpec) -> Result<(), Status> {
        let mut state = self.state.lock().unwrap();
        let protection = pb::Protection {
            branch: "main".into(),
            effective: Some(spec),
            unsupported: Vec::new(),
            ..Default::default()
        };
        if state.snapshot.protection.as_ref() != Some(&protection) {
            state.generation += 1;
            let generation = state.generation;
            state.snapshot.revision.as_mut().unwrap().configuration =
                format!("configuration-{generation}");
            state.snapshot.protection_found = true;
            state.snapshot.protection = Some(protection);
        }
        Ok(())
    }
    async fn recreate(&self) -> Result<(), Status> {
        let mut state = self.state.lock().unwrap();
        state.generation += 1;
        state.incarnation += 1;
        state.snapshot = pb::GetProvisionSnapshotResponse {
            found: true,
            repo: Some(pb::ProvisionedRepo {
                repo: Some(self.repo.clone()),
                lifecycle: pb::RepoLifecycle::Active as i32,
                ..Default::default()
            }),
            revision: Some(pb::ProvisionRevision {
                incarnation: format!("incarnation-{}", state.incarnation),
                configuration: format!("configuration-{}", state.generation),
            }),
            ..Default::default()
        };
        Ok(())
    }
}
forge::guarded_revision_conformance_suite!(revision_model, Model::new());
