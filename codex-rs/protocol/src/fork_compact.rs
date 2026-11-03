//! Fork (nuwax-codex): the auto-compaction threshold policy.
//!
//! Kept in a fork-owned file so the merge surface in upstream's
//! `openai_models.rs` is a one-line call. Semantics (see FORK.md §10):
//!
//! * The derived default is **80%** of the resolved window, not upstream's
//!   90%. Third-party gateways reserve part of the window for output (probe
//!   2026-11-02: a 1M-window gateway rejected at ~950-980k), and the
//!   compaction request itself carries the full history, so a 90% trigger
//!   leaves too little room for either — the probe's compaction request was
//!   rejected and had to shed history.
//! * An explicit limit — catalog or user configuration — is **authoritative
//!   and uncapped** by the derived ratio: silently clamping a configured
//!   950k to 900k contradicted fail-fast, and every catalog value is at or
//!   below 90%, so official models keep their behavior.

/// The fork's effective auto-compaction limit.
pub fn effective_auto_compact_limit(
    explicit: Option<i64>,
    resolved_context_window: Option<i64>,
) -> Option<i64> {
    match explicit {
        Some(explicit) => Some(explicit),
        None => resolved_context_window.map(|window| (window * 8) / 10),
    }
}
