//! Responses SSE decoding. The event mapping and error classification are
//! shared by the native transport and by fork bridges that speak the
//! Responses wire themselves (same-protocol passthrough); the strictness of
//! terminal handling stays with each caller.
mod responses_error;

pub mod responses;
mod responses_policy;

pub use responses::ResponsesStreamEvent;
pub use responses::process_responses_event;
pub use responses::spawn_response_stream;
pub use responses_policy::spawn_strict_response_stream;
