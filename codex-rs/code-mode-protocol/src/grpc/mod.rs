#[cfg(codex_bazel)]
pub use code_mode_proto::codex::code_mode::v1::*;

#[cfg(not(codex_bazel))]
// 1.99 clippy flags tonic's generated `#[must_use]`-without-message traits
// (async_trait expansion). Wrapping the include in a module scopes an
// outer-`mod` allow to everything the macro expands — a plain attribute on
// the include does not reach those items.
#[allow(
    clippy::must_use_candidate,
    clippy::let_unit_value,
    clippy::double_must_use
)]
mod code_mode_v1 {
    tonic::include_proto!("codex.code_mode.v1");
}
#[cfg(not(codex_bazel))]
pub use code_mode_v1::*;

pub const MAX_IDENTIFIER_BYTES: usize = 256;

/// Decimal nanoseconds a completed result was buffered before a wait or terminate retrieved it.
/// Zero means immediate delivery. Hosts omit this metadata if no completed result was retrieved.
pub const RESULT_BUFFERED_FOR_NS_METADATA: &str = "codex-code-mode-result-buffered-for-ns";
