//! Shared guarded-provisioning behavior, reusable by real server fixtures.
//!
//! Each case requires a fresh, active repository and an authorized caller.
//! These initial cases cover lifecycle preconditions and receipt replay. They
//! do not establish restart recovery, retention, protection, or access revocation.
use crate::{guarded::*, pb};
use tonic::Code;

pub trait Fixture {
    fn provisioner(&self) -> &dyn GuardedProvisioner;
    fn repo(&self) -> pb::RepoRef;
}

async fn snapshot(fx: &dyn Fixture) -> pb::GetProvisionSnapshotResponse {
    fx.provisioner()
        .snapshot(GetProvisionSnapshotRequest {
            repo: Some(fx.repo()),
            branch: String::new(),
        })
        .await
        .unwrap()
}

fn archive(
    fx: &dyn Fixture,
    revision: Option<pb::ProvisionRevision>,
    key: &str,
) -> GuardedArchiveRepoRequest {
    GuardedArchiveRepoRequest {
        repo: Some(fx.repo()),
        expected: revision,
        idempotency_key: key.into(),
    }
}

async fn succeeded(
    fx: &dyn Fixture,
    mut receipt: pb::ProvisionMutationReceipt,
) -> pb::ProvisionMutationReceipt {
    // Pending intent is valid. Poll the original operation without replaying it.
    let initial_id = receipt.operation_id.clone();
    let initial_request = receipt.request.clone();
    let initial_actor = receipt.actor.clone();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            assert_eq!(receipt.operation_id, initial_id);
            assert_eq!(receipt.request, initial_request);
            assert_eq!(receipt.actor, initial_actor);
            match pb::ProvisionMutationPhase::try_from(receipt.phase).unwrap() {
                pb::ProvisionMutationPhase::Succeeded => break,
                pb::ProvisionMutationPhase::Pending
                | pb::ProvisionMutationPhase::ReconciliationRequired => {
                    assert!(receipt.result.is_none() && receipt.resulting_revision.is_none());
                    let (repo, revision, key) = match receipt.request.as_ref().unwrap() {
                        pb::provision_mutation_receipt::Request::Archive(r) => {
                            (&r.repo, &r.expected, &r.idempotency_key)
                        }
                        pb::provision_mutation_receipt::Request::Delete(r) => {
                            (&r.repo, &r.expected, &r.idempotency_key)
                        }
                        pb::provision_mutation_receipt::Request::Protection(r) => {
                            (&r.repo, &r.expected, &r.idempotency_key)
                        }
                    };
                    let query = GetProvisionMutationRequest {
                        repo: repo.clone(),
                        incarnation: revision.as_ref().unwrap().incarnation.clone(),
                        idempotency_key: key.clone(),
                    };
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    receipt = fx
                        .provisioner()
                        .mutation(query)
                        .await
                        .unwrap()
                        .receipt
                        .unwrap();
                }
                phase => panic!("operation did not succeed: {phase:?}"),
            }
        }
    })
    .await
    .expect("healthy fixture must complete within ten seconds");
    assert!(!receipt.operation_id.is_empty());
    let actor = receipt.actor.as_ref().expect("verified provenance");
    assert!(!actor.issuer.is_empty() && !actor.subject.is_empty());
    receipt
}

/// The outcome and the subsequent consistent snapshot agree on the new version.
pub async fn archive_updates_snapshot(fx: &dyn Fixture) {
    let before = snapshot(fx).await;
    assert!(before.found);
    let old = before.revision.expect("initial revision");
    assert!(!old.incarnation.is_empty() && !old.configuration.is_empty());
    let request = archive(fx, Some(old.clone()), "archive");
    let receipt = fx
        .provisioner()
        .archive(request.clone())
        .await
        .unwrap()
        .receipt
        .unwrap();
    let receipt = succeeded(fx, receipt).await;
    assert_eq!(
        receipt.request,
        Some(pb::provision_mutation_receipt::Request::Archive(request))
    );
    let after = snapshot(fx).await;
    assert!(after.found);
    let repo = after.repo.unwrap();
    assert_eq!(repo.repo, Some(fx.repo()));
    assert_eq!(repo.lifecycle, pb::RepoLifecycle::Archived as i32);
    let new = after.revision.unwrap();
    assert_eq!(new.incarnation, old.incarnation);
    assert_ne!(new.configuration, old.configuration);
    assert!(!new.configuration.is_empty());
    assert_eq!(receipt.resulting_revision, Some(new));
    assert_eq!(
        receipt.result,
        Some(pb::provision_mutation_receipt::Result::Archived(
            pb::ArchivedProvisionResult { repo: Some(repo) }
        ))
    );
}

/// A stale request cannot delete the repository after another change commits.
pub async fn stale_delete_has_no_effect(fx: &dyn Fixture) {
    let old = snapshot(fx).await.revision;
    let receipt = fx
        .provisioner()
        .archive(archive(fx, old.clone(), "archive"))
        .await
        .unwrap()
        .receipt
        .unwrap();
    succeeded(fx, receipt).await;
    let before = snapshot(fx).await;
    let error = fx
        .provisioner()
        .delete(GuardedDeleteRepoRequest {
            repo: Some(fx.repo()),
            expected: old,
            confirm_name: fx.repo().name,
            idempotency_key: "stale-delete".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(error.code(), Code::FailedPrecondition);
    assert_eq!(
        snapshot(fx).await,
        before,
        "stale deletion changed repository state"
    );
}

/// Distinct requests sharing a revision cannot both commit.
pub async fn competing_archives_have_one_winner(fx: &dyn Fixture) {
    let old = snapshot(fx).await.revision;
    let (a, b) = tokio::join!(
        fx.provisioner().archive(archive(fx, old.clone(), "first")),
        fx.provisioner().archive(archive(fx, old, "second"))
    );
    let (winner, loser) = match (a, b) {
        (Ok(w), Err(e)) | (Err(e), Ok(w)) => (w.receipt.unwrap(), e),
        other => panic!("expected one committed operation and one stale rejection: {other:?}"),
    };
    let winner = succeeded(fx, winner).await;
    assert_eq!(loser.code(), Code::FailedPrecondition);
    assert_eq!(snapshot(fx).await.revision, winner.resulting_revision);
}

/// A lost response is recoverable by lookup and an identical retry; a reused
/// key with different input is rejected even when its revision is now stale.
pub async fn archive_retry_preserves_receipt(fx: &dyn Fixture) {
    let old = snapshot(fx).await.revision.unwrap();
    let request = archive(fx, Some(old.clone()), "retry");
    let first = fx
        .provisioner()
        .archive(request.clone())
        .await
        .unwrap()
        .receipt
        .unwrap();
    let first = succeeded(fx, first).await;
    let after = snapshot(fx).await;
    let lookup = GetProvisionMutationRequest {
        repo: Some(fx.repo()),
        incarnation: old.incarnation,
        idempotency_key: "retry".into(),
    };
    assert_eq!(
        fx.provisioner().mutation(lookup).await.unwrap().receipt,
        Some(first.clone())
    );
    assert_eq!(
        fx.provisioner()
            .archive(request.clone())
            .await
            .unwrap()
            .receipt,
        Some(first)
    );
    let mut changed = request;
    changed
        .expected
        .as_mut()
        .unwrap()
        .configuration
        .push_str("-different");
    assert_eq!(
        fx.provisioner().archive(changed).await.unwrap_err().code(),
        Code::AlreadyExists
    );
    assert_eq!(
        snapshot(fx).await,
        after,
        "retry must not create another revision"
    );
}

/// Deletion requires exact name confirmation and preserves its receipt after
/// repository removal, allowing recovery without guessing from absence.
pub async fn deletion_receipt_survives_removal(fx: &dyn Fixture) {
    let before = snapshot(fx).await;
    let old = before.revision.clone().unwrap();
    let mut request = GuardedDeleteRepoRequest {
        repo: Some(fx.repo()),
        expected: Some(old.clone()),
        confirm_name: format!("{}-wrong", fx.repo().name),
        idempotency_key: "delete".into(),
    };
    assert_eq!(
        fx.provisioner()
            .delete(request.clone())
            .await
            .unwrap_err()
            .code(),
        Code::InvalidArgument
    );
    assert_eq!(snapshot(fx).await, before);
    request.confirm_name = fx.repo().name;
    let receipt = fx
        .provisioner()
        .delete(request.clone())
        .await
        .unwrap()
        .receipt
        .unwrap();
    let receipt = succeeded(fx, receipt).await;
    assert_eq!(
        receipt.request,
        Some(pb::provision_mutation_receipt::Request::Delete(
            request.clone()
        ))
    );
    assert_eq!(receipt.resulting_revision, None);
    assert_eq!(
        receipt.result,
        Some(pb::provision_mutation_receipt::Result::Deleted(
            pb::DeletedProvisionResult { confirmed: true }
        ))
    );
    let after = snapshot(fx).await;
    assert!(!after.found);
    assert!(after.repo.is_none() && after.revision.is_none());
    assert_eq!(
        fx.provisioner().delete(request).await.unwrap().receipt,
        Some(receipt.clone())
    );
    assert_eq!(
        fx.provisioner()
            .mutation(GetProvisionMutationRequest {
                repo: Some(fx.repo()),
                incarnation: old.incarnation,
                idempotency_key: "delete".into()
            })
            .await
            .unwrap()
            .receipt,
        Some(receipt)
    );
    assert_eq!(snapshot(fx).await, after);
}

/// Empty keys and incomplete versions are invalid, never unconditional writes.
pub async fn missing_preconditions_have_no_effect(fx: &dyn Fixture) {
    let before = snapshot(fx).await;
    let mut requests = vec![
        archive(fx, None, "missing"),
        archive(fx, Some(pb::ProvisionRevision::default()), "empty"),
        archive(fx, before.revision.clone(), ""),
    ];
    for missing_incarnation in [true, false] {
        let mut revision = before.revision.clone().unwrap();
        if missing_incarnation {
            revision.incarnation.clear();
        } else {
            revision.configuration.clear();
        }
        requests.push(archive(fx, Some(revision), "partial"));
    }
    for request in requests {
        assert_eq!(
            fx.provisioner().archive(request).await.unwrap_err().code(),
            Code::InvalidArgument
        );
        assert_eq!(snapshot(fx).await, before);
    }
}

/// Run identical behavioral cases against a fresh fixture per test. The fixture
/// expression may await daemon startup in the consumer's test runtime.
#[macro_export]
macro_rules! guarded_conformance_suite {
    ($name:ident, $fixture:expr) => {
        mod $name {
            use super::*;
            #[tokio::test]
            async fn archive_updates_snapshot() {
                $crate::guarded_conformance::archive_updates_snapshot(&$fixture).await;
            }
            #[tokio::test]
            async fn stale_delete_has_no_effect() {
                $crate::guarded_conformance::stale_delete_has_no_effect(&$fixture).await;
            }
            #[tokio::test]
            async fn competing_archives_have_one_winner() {
                $crate::guarded_conformance::competing_archives_have_one_winner(&$fixture).await;
            }
            #[tokio::test]
            async fn archive_retry_preserves_receipt() {
                $crate::guarded_conformance::archive_retry_preserves_receipt(&$fixture).await;
            }
            #[tokio::test]
            async fn deletion_receipt_survives_removal() {
                $crate::guarded_conformance::deletion_receipt_survives_removal(&$fixture).await;
            }
            #[tokio::test]
            async fn missing_preconditions_have_no_effect() {
                $crate::guarded_conformance::missing_preconditions_have_no_effect(&$fixture).await;
            }
        }
    };
}

/// Required controls for testing changes made through the legacy provisioning
/// surface. Implement these with the real legacy service in backend fixtures;
/// directly changing the guarded service's revision would not test coordination.
#[async_trait::async_trait]
pub trait RevisionFixture: Fixture + Sync {
    async fn legacy_protection(&self, spec: pb::ProtectionSpec) -> Result<(), tonic::Status>;
    /// Delete and recreate the same repository through its supported lifecycle.
    async fn recreate(&self) -> Result<(), tonic::Status>;
}

async fn protection_snapshot(fx: &dyn RevisionFixture) -> GetProvisionSnapshotResponse {
    fx.provisioner()
        .snapshot(GetProvisionSnapshotRequest {
            repo: Some(fx.repo()),
            branch: "main".into(),
        })
        .await
        .unwrap()
}
async fn rejects_old_delete(fx: &dyn RevisionFixture, old: Option<pb::ProvisionRevision>) {
    let before = protection_snapshot(fx).await;
    let result = fx
        .provisioner()
        .delete(GuardedDeleteRepoRequest {
            repo: Some(fx.repo()),
            expected: old,
            confirm_name: fx.repo().name,
            idempotency_key: "stale-confirmation".into(),
        })
        .await;
    assert_eq!(result.unwrap_err().code(), Code::FailedPrecondition);
    assert_eq!(protection_snapshot(fx).await, before);
}

pub async fn legacy_settings_invalidate_confirmation(fx: &dyn RevisionFixture) {
    let before = protection_snapshot(fx).await;
    // Toggle a supported field so this is a real change even on a protected repo.
    let mut spec = before
        .protection
        .as_ref()
        .and_then(|p| p.effective.clone())
        .unwrap_or_default();
    spec.block_force_push = !spec.block_force_push;
    fx.legacy_protection(spec.clone()).await.unwrap();
    let after = protection_snapshot(fx).await;
    assert!(after.found && after.protection_found);
    assert_eq!(
        after
            .protection
            .unwrap()
            .effective
            .unwrap()
            .block_force_push,
        spec.block_force_push
    );
    let old = before.revision.as_ref().unwrap();
    let new = after.revision.as_ref().unwrap();
    assert_eq!(old.incarnation, new.incarnation);
    assert_ne!(old.configuration, new.configuration);
    rejects_old_delete(fx, before.revision).await;
}

pub async fn settings_reversion_keeps_confirmation_stale(fx: &dyn RevisionFixture) {
    let spec = pb::ProtectionSpec {
        block_force_push: true,
        ..Default::default()
    };
    fx.legacy_protection(spec.clone()).await.unwrap();
    let before = protection_snapshot(fx).await;
    fx.legacy_protection(pb::ProtectionSpec {
        block_force_push: false,
        ..Default::default()
    })
    .await
    .unwrap();
    fx.legacy_protection(spec).await.unwrap();
    let after = protection_snapshot(fx).await;
    assert_eq!(
        before.protection, after.protection,
        "the visible settings are restored"
    );
    assert_eq!(
        before.revision.as_ref().unwrap().incarnation,
        after.revision.as_ref().unwrap().incarnation
    );
    assert_ne!(
        before.revision, after.revision,
        "a reverted value must not revive an old confirmation"
    );
    rejects_old_delete(fx, before.revision).await;
}

pub async fn recreated_repository_rejects_original_confirmation(fx: &dyn RevisionFixture) {
    let before = protection_snapshot(fx).await;
    fx.recreate().await.unwrap();
    let after = protection_snapshot(fx).await;
    assert!(after.found);
    assert_eq!(
        before.repo.as_ref().unwrap().repo,
        after.repo.as_ref().unwrap().repo
    );
    assert_ne!(
        before.revision.as_ref().unwrap().incarnation,
        after.revision.as_ref().unwrap().incarnation
    );
    rejects_old_delete(fx, before.revision).await;
}

#[macro_export]
macro_rules! guarded_revision_conformance_suite {
    ($name:ident, $fixture:expr) => {
        mod $name {
            use super::*;
            #[tokio::test]
            async fn legacy_settings_invalidate_confirmation() {
                $crate::guarded_conformance::legacy_settings_invalidate_confirmation(&$fixture)
                    .await;
            }
            #[tokio::test]
            async fn settings_reversion_keeps_confirmation_stale() {
                $crate::guarded_conformance::settings_reversion_keeps_confirmation_stale(&$fixture)
                    .await;
            }
            #[tokio::test]
            async fn recreated_repository_rejects_original_confirmation() {
                $crate::guarded_conformance::recreated_repository_rejects_original_confirmation(
                    &$fixture,
                )
                .await;
            }
        }
    };
}
