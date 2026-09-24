//! Bridge crate that converts between Codex types (`ResponseItem`, `ResponseEvent`)
//! and rust-genai types (`ChatRequest`, `ChatStreamEvent`).
//!
//! The core entry point is [`stream_via_genai`], which takes a Codex
//! `ResponsesApiRequest`, streams through rust-genai, and emits Codex
//! `ResponseEvent`s — preserving the existing Codex type system for all
//! upstream consumers.

mod bridge_impl;
mod convert_request;
mod convert_response;
mod resolver;
mod stream;
mod types;

pub use bridge_impl::GenaiChatBridge;
pub use stream::stream_via_genai;
