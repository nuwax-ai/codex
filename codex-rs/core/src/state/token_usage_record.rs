use codex_protocol::protocol::TokenUsageRecord;

/// Reject oversized identities whole so distinct provider IDs never alias
/// through a truncated prefix.
pub(super) const MAX_RESPONSE_IDENTITY_BYTES: usize = 256;

/// Copies a persisted usage snapshot into restored state without cloning
/// rejected response identities. Counters and turn ownership stay intact,
/// while the original rollout record remains untouched.
pub(crate) fn clone_usage_record_for_resume(record: &TokenUsageRecord) -> TokenUsageRecord {
    let invalid_host_key = record
        .response_key
        .as_ref()
        .is_some_and(|key| key.len() > MAX_RESPONSE_IDENTITY_BYTES);
    TokenUsageRecord {
        thread_id: record.thread_id,
        turn_id: record.turn_id.clone(),
        session_id: record.session_id,
        root_turn_id: record.root_turn_id.clone(),
        // An invalid host key also clears the advisory provider ID, keeping
        // this record keyless if a later checkpoint is resumed again.
        response_id: if invalid_host_key || record.response_id.len() > MAX_RESPONSE_IDENTITY_BYTES {
            String::new()
        } else {
            record.response_id.clone()
        },
        response_key: record
            .response_key
            .as_ref()
            .filter(|key| key.len() <= MAX_RESPONSE_IDENTITY_BYTES)
            .cloned(),
        usage: record.usage.clone(),
        turn_token_usage: record.turn_token_usage.clone(),
        thread_token_usage: record.thread_token_usage.clone(),
    }
}
