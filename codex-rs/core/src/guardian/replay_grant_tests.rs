//! Unit coverage for the guardian checkpoint replay grant bindings.

use codex_history::ModelOutputProvenance;
use pretty_assertions::assert_eq;

use super::OpaqueReplayGrant;
use super::authorize_opaque_replay;
use crate::model_output_projection::OpaqueReplayAuthorization;

fn provenance(model: &str, auth_domain: &str) -> ModelOutputProvenance {
    ModelOutputProvenance {
        wire_protocol: "responses".to_string(),
        bridge: None,
        provider: Some("openai".to_string()),
        model: Some(model.to_string()),
        endpoint_identity: Some("https://api.openai.com/v1".to_string()),
        auth_domain: Some(auth_domain.to_string()),
        auth_domain_kind: Some("account".to_string()),
    }
}

fn grant(checkpoint: &str, producer_model: &str, reviewer_model: &str) -> OpaqueReplayGrant {
    OpaqueReplayGrant::for_checkpoint(&envelope(checkpoint, producer_model), reviewer_model)
        .expect("checkpoint with id and provenance should produce a grant")
}

fn envelope(checkpoint: &str, producer_model: &str) -> codex_history::ResponseItemEnvelope {
    codex_history::ResponseItemEnvelope {
        item: serde_json::from_value(serde_json::json!({
            "type": "compaction",
            "id": checkpoint,
            "encrypted_content": "opaque-checkpoint-payload"
        }))
        .expect("compaction fixture"),
        metadata: Some(codex_history::CodexHarnessMetadata {
            model_output_provenance: Some(provenance(producer_model, "account-v1:producer")),
            ..Default::default()
        }),
    }
}

/// A review request: the reviewer model under the guardian header (whose
/// auth-domain shift the grant exists to tolerate), and the same request with
/// the header stripped (its basis).
fn review_request(
    producer: &ModelOutputProvenance,
    reviewer_model: &str,
) -> (ModelOutputProvenance, ModelOutputProvenance) {
    let mut target = producer.clone();
    target.model = Some(reviewer_model.to_string());
    target.auth_domain = Some("credential-instance:shifted-by-header".to_string());
    target.auth_domain_kind = Some("credentialInstance".to_string());
    let basis = producer.clone();
    (target, basis)
}

#[test]
fn grant_requires_item_identity_and_captured_provenance() {
    let model = "codex-auto-review";
    assert!(
        OpaqueReplayGrant::for_checkpoint(
            &codex_history::ResponseItemEnvelope {
                item: serde_json::from_value(serde_json::json!({
                    "type": "compaction", "encrypted_content": "payload"
                }))
                .unwrap(),
                metadata: Some(codex_history::CodexHarnessMetadata::default()),
            },
            model,
        )
        .is_none(),
        "an envelope without item identity cannot be granted"
    );
    assert!(
        OpaqueReplayGrant::for_checkpoint(
            &codex_history::ResponseItemEnvelope {
                item: serde_json::from_value(serde_json::json!({
                    "type": "compaction", "id": "item_1", "encrypted_content": "payload"
                }))
                .unwrap(),
                metadata: Some(codex_history::CodexHarnessMetadata::default()),
            },
            model,
        )
        .is_none(),
        "an envelope without captured provenance cannot be granted"
    );
    assert!(
        grant("item_1", "gpt-5.2-codex", model)
            .producer
            .model
            .is_some()
    );
}

#[test]
fn authorized_review_request_replays_exactly_the_bound_checkpoint() {
    let grant = grant("item_1", "gpt-5.2-codex", "codex-auto-review");
    let producer = provenance("gpt-5.2-codex", "account-v1:producer");
    let (target, basis) = review_request(&producer, "codex-auto-review");

    let authorization = authorize_opaque_replay(&grant, &target, &basis)
        .expect("the approved review request should authorize its checkpoint");
    let mut input = vec![
        serde_json::from_value(serde_json::json!({
            "type": "message", "id": "msg_1", "role": "user",
            "content": [{"type": "input_text", "text": "review"}]
        }))
        .unwrap(),
        serde_json::from_value(serde_json::json!({
            "type": "compaction", "id": "item_1", "encrypted_content": "payload"
        }))
        .unwrap(),
    ];
    let sources =
        crate::model_output_projection::sources_for_input(&[envelope("item_1", "gpt-5.2-codex")]);
    let saved = input.clone();
    crate::model_output_projection::project_input(
        &mut input,
        &sources,
        &target,
        Some(&authorization),
    );
    assert_eq!(
        input, saved,
        "the granted checkpoint must survive projection"
    );
}

#[test]
fn a_different_checkpoint_under_the_same_grant_is_still_dropped() {
    let grant = grant("item_1", "gpt-5.2-codex", "codex-auto-review");
    let producer = provenance("gpt-5.2-codex", "account-v1:producer");
    let (target, basis) = review_request(&producer, "codex-auto-review");
    let authorization = authorize_opaque_replay(&grant, &target, &basis)
        .expect("the request itself is the approved review");

    // A different checkpoint (or one whose captured provenance was replaced)
    // is not the grant's subject and keeps the isolation.
    let sources =
        crate::model_output_projection::sources_for_input(&[envelope("item_2", "gpt-5.2-codex")]);
    let item: codex_protocol::models::ResponseItem = serde_json::from_value(
        serde_json::json!({"type": "compaction", "id": "item_2", "encrypted_content": "x"}),
    )
    .unwrap();
    let mut input = vec![item];
    crate::model_output_projection::project_input(
        &mut input,
        &sources,
        &target,
        Some(&authorization),
    );
    assert!(
        input.is_empty(),
        "only the bound checkpoint id may replay; other checkpoints stay isolated"
    );
}

#[test]
fn edited_producer_metadata_does_not_satisfy_the_grant() {
    let grant = grant("item_1", "gpt-5.2-codex", "codex-auto-review");
    let producer = provenance("gpt-5.2-codex", "account-v1:producer");
    let (target, basis) = review_request(&producer, "codex-auto-review");
    let authorization = authorize_opaque_replay(&grant, &target, &basis).unwrap();

    // The history now claims a different producer for the same id.
    let edited = envelope("item_1", "other-model");
    let sources = crate::model_output_projection::sources_for_input(&[edited.clone()]);
    let mut input = vec![edited.item.clone()];
    crate::model_output_projection::project_input(
        &mut input,
        &sources,
        &target,
        Some(&authorization),
    );
    assert!(
        input.is_empty(),
        "replaced producer provenance must not satisfy the grant"
    );
}

#[test]
fn rotated_credentials_endpoints_wires_or_models_reject_the_grant() {
    let grant = grant("item_1", "gpt-5.2-codex", "codex-auto-review");
    let producer = provenance("gpt-5.2-codex", "account-v1:producer");

    // Unapproved reviewer model on the live request.
    let (target, basis) = review_request(&producer, "some-other-model");
    assert!(
        authorize_opaque_replay(&grant, &target, &basis).is_none(),
        "a model outside the grant's approval must not replay"
    );

    // A different evidence KIND (e.g. an ambient account login instead of
    // the captured credential instance) must not replay. The random
    // per-process instance id itself is deliberately not comparable across
    // processes, so only kind-level drift is checkable here.
    let (target, mut basis) = review_request(&producer, "codex-auto-review");
    basis.auth_domain = Some("credential-instance-v2:ambient".to_string());
    basis.auth_domain_kind = Some("credentialInstance".to_string());
    assert!(
        authorize_opaque_replay(&grant, &target, &basis).is_none(),
        "a request under a different credential-evidence kind must not replay"
    );

    // Endpoint drift.
    let (target, mut basis) = review_request(&producer, "codex-auto-review");
    basis.endpoint_identity = Some("https://elsewhere.example/v1".to_string());
    assert!(
        authorize_opaque_replay(&grant, &target, &basis).is_none(),
        "a different endpoint must not replay"
    );

    // Wire drift on the live request.
    let (mut target, basis) = review_request(&producer, "codex-auto-review");
    target.wire_protocol = "chat".to_string();
    assert!(
        authorize_opaque_replay(&grant, &target, &basis).is_none(),
        "a different wire must not replay"
    );
}
