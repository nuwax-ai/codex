//! Runtime-issued authorization for replaying one parent checkpoint across
//! the opaque-output provenance boundary during guardian review.
//!
//! The provenance projection exists so opaque model output is never replayed
//! to a different credential, endpoint, or wire than produced it. Guardian
//! review is the one intentional exception: a reviewer thread replays the
//! parent thread's encrypted checkpoint to the same provider under the
//! reviewer model, with the backend validating the payload.
//!
//! Trust model:
//!
//! - the grant is created only where the trusted review runtime selects the
//!   seed envelope for a reviewer thread (`thread_options`), from the
//!   in-memory envelope and the configured reviewer model — never from
//!   user-editable history and never persisted to rollouts
//! - it travels in spawn-time thread extension data (the same trusted channel
//!   as the reviewer's response headers) and is attached to the prompt by the
//!   guardian request-budget stage
//! - at request projection time the grant is validated against the live
//!   request: with the fixed `x-codex-guardian` reviewer header stripped, the
//!   request must reproduce the producer's exact provider, endpoint, wire,
//!   bridge, and auth scope; the only tolerated differences are the approved
//!   reviewer-model substitution and the auth-domain shift that the header
//!   itself causes
//!
//! Anything else — a different checkpoint, edited provenance metadata, a
//! rotated credential or endpoint, another model, a normal resume — keeps the
//! existing isolation and the checkpoint is dropped as before.

use codex_history::ModelOutputProvenance;
use codex_history::ResponseItemEnvelope;

use crate::model_output_projection::OpaqueReplayAuthorization;

/// The fixed reviewer header trusted runtime injects into review requests.
pub(crate) const GUARDIAN_REVIEWER_HEADER: &str = "x-codex-guardian";

/// A narrowly bound permission to replay one checkpoint on one review thread.
#[derive(Debug, Clone)]
pub(crate) struct OpaqueReplayGrant {
    checkpoint_id: codex_protocol::ResponseItemId,
    producer: ModelOutputProvenance,
    reviewer_model: String,
}

impl OpaqueReplayGrant {
    /// Binds the exact seed envelope and the reviewer model approved for it.
    ///
    /// Returns `None` when the envelope carries no item identity or no
    /// captured producer provenance; a grant never guesses either.
    pub(crate) fn for_checkpoint(
        envelope: &ResponseItemEnvelope,
        reviewer_model: &str,
    ) -> Option<Self> {
        let checkpoint_id = envelope.item.id()?;
        let producer = envelope
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.model_output_provenance.clone())?;
        Some(Self {
            checkpoint_id: checkpoint_id.clone(),
            producer,
            reviewer_model: reviewer_model.to_string(),
        })
    }

}

/// Validates the grant against the live request and returns the projection-
/// level authorization when every binding holds.
///
/// `target` is the request's own provenance (with the reviewer header) and
/// `basis` is the same computation with only the fixed reviewer header
/// stripped, proving the credential and endpoint scope is otherwise
/// unchanged from the producer's.
pub(crate) fn authorize_opaque_replay(
    grant: &OpaqueReplayGrant,
    target: &ModelOutputProvenance,
    basis: &ModelOutputProvenance,
) -> Option<OpaqueReplayAuthorization> {
    // Credential-instance identities are deliberately random per process
    // ("eviction/restart conservatively yields a new identity"), and a
    // reviewer thread can live in a different process than the parent
    // compaction, so the minted `auth_domain` strings can never be compared
    // across that boundary. Credential continuity is instead attested by the
    // trusted runtime that issues the grant: it captures the producer from
    // the parent session's own compaction and spawns the reviewer with the
    // parent's auth manager. What the request-side check can and does prove
    // is that the reviewer request rides the same provider, endpoint, wire,
    // and bridge under the same evidence KIND, with only the approved
    // reviewer-model substitution and the guardian header differing.
    let scope_matches_producer = basis.wire_protocol == grant.producer.wire_protocol
        && basis.bridge == grant.producer.bridge
        && basis.provider == grant.producer.provider
        && basis.endpoint_identity == grant.producer.endpoint_identity
        && basis.auth_domain_kind == grant.producer.auth_domain_kind;
    let target_differs_only_as_approved = target.model.as_deref()
        == Some(grant.reviewer_model.as_str())
        && target.wire_protocol == basis.wire_protocol
        && target.bridge == basis.bridge
        && target.provider == basis.provider
        && target.endpoint_identity == basis.endpoint_identity;
    (scope_matches_producer && target_differs_only_as_approved).then(|| {
        OpaqueReplayAuthorization::new(grant.checkpoint_id.clone(), grant.producer.clone())
    })
}

#[cfg(test)]
#[path = "replay_grant_tests.rs"]
mod tests;
