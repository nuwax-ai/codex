//! Shared support for the live integration tests of the chat bridges.
//!
//! Everything here talks to REAL providers and is vendor-agnostic: configure
//! one via `LIVE_VENDOR_*` (environment, or a gitignored `.env.local` /
//! `.env` at the repository root; Xiaomi MiMo is the default and the
//! `MIMO_*` variables remain as fallbacks). Adding a vendor is pure
//! configuration — no code changes. Two suites live in this crate:
//!
//! - `tests/bridge_live.rs` — bridge level: `ResponsesApiRequest` → bridge →
//!   provider SSE → codex `ResponseEvent`s, with protocol invariants asserted
//!   on real streams; defaults to the rig bridge, with genai variants opt-in
//!   via `LIVE_INCLUDE_GENAI=1` (bridge shelved; A/B comparison).
//! - `tests/exec_live.rs` — binary level: the compiled `codex-exec` driven
//!   end to end with the unforgeable-marker technique, covering both
//!   `wire_api` values: chat (always via a bridge) and responses (fork
//!   default: third-party providers also go through the rig bridge;
//!   `experimental_bridge = "native"` opts back into the upstream
//!   transport).
//!
//! Every run persists artifacts under `<repo>/logs/live-<vendor>/`
//! (gitignored): per-turn bridge event logs in `bridge/`, and per-run
//! `events.jsonl` / `final_message.txt` / `stderr.log` for the binary suite.
//!
//! NOTE: rebuild the binary after changing core/provider code, or the
//! binary-level suite tests the stale executable:
//! `cargo build -p codex-exec --bin codex-exec`.
//!
//! The crate is organized by layer: [`config`]/[`env`] (vendor setup),
//! [`bridge_turns`] (L1), [`binary_turns`] (L3), [`assertions`] /
//! [`error_probe`] (verification), [`artifacts`] / [`cassette`] (evidence),
//! [`responses_replay`] (offline Responses replay). `lib.rs` re-exports the
//! public surface so the integration tests see one namespace.

// This crate is test infrastructure only (consumed exclusively by its own
// integration tests under tests/); panicking helpers are the intended
// fail-fast behavior inside a test runner. Mirrors the lmstudio precedent.
#![allow(clippy::expect_used)]
#![allow(clippy::unwrap_used)]

pub(crate) mod artifacts;
mod assertions;
mod binary_turns;
mod bridge_turns;
mod cassette;
mod config;
mod env;
mod error_probe;
mod responses_replay;

pub use cassette::CassetteMode;
pub use cassette::TurnFixture;
pub use cassette::cassette_mode;
pub use cassette::fixture_path;
pub use cassette::load_fixture;
pub use cassette::load_responses_sse_fixture;
pub use cassette::load_rig_event_fixture;
use cassette::record_turn;
pub use cassette::save_final_request_fixture;
pub use cassette::save_responses_sse_fixture;
pub use cassette::save_rig_event_fixture;
pub use cassette::store_fixture;
pub use config::LiveConfig;
pub use config::load_env_files;
pub use config::vendor;
pub use config::vendors;

pub use assertions::assert_completed_with_usage;
pub use assertions::assert_reasoning_before_message;
pub use assertions::assert_responses_tool_arguments_complete;
pub use assertions::assert_tool_deltas_reassemble;
pub use assertions::end_turn_of;
pub use assertions::event_kind;
pub use assertions::reasoning_len;
pub use assertions::text_len;
pub use binary_turns::codex_exec_binary;
pub use binary_turns::run_capped_marker_turn;
pub use binary_turns::run_compact_turn;
pub use binary_turns::run_marker_turn;
pub use binary_turns::run_websearch_turns;
pub use binary_turns::write_config_toml;
pub use bridge_turns::Bridge;
pub use bridge_turns::LiveWire;
pub use bridge_turns::drain_stream;
pub use bridge_turns::genai_bridge_enabled;
pub use bridge_turns::run_responses_turn_rig;
pub use bridge_turns::run_turn;
pub use bridge_turns::run_turn_rig;
pub use env::StaticBearerAuth;
pub use env::anthropic_url_or_skip;
pub use env::repo_root;
pub use env::responses_url_or_skip;
pub use env::shared_auth;
pub use env::user_message;
pub use env::vendor_provider;
pub use error_probe::turn_start_error;
// Cross-module plumbing that used to be file-private in the single-module
// layout; the sibling layers still share them.
pub(crate) use artifacts::persist_lines;
pub(crate) use artifacts::prune_artifacts;
pub(crate) use artifacts::write_manifest;

use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use anyhow::Result;
use anyhow::anyhow;
use codex_api::AuthProvider;
use codex_api::Provider;
use codex_api::ResponseEvent;
use codex_api::ResponseStream;
use codex_api::ResponsesApiRequest;
use codex_api::RetryConfig;
use codex_api::SharedAuthProvider;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use http::HeaderMap;
use http::header::AUTHORIZATION;
use serde_json::Value;
use tokio::time::timeout;

const TURN_TIMEOUT: Duration = Duration::from_secs(180);
const EXEC_RUN_TIMEOUT: Duration = Duration::from_secs(300);
