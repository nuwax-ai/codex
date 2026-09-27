//! Responses SSE decoding. The event mapping and error classification are
//! shared by the native transport and by fork bridges that speak the
//! Responses wire themselves (same-protocol passthrough); the strictness of
//! terminal handling stays with each caller.

pub mod responses;

pub use responses::ResponsesStreamEvent;
pub use responses::process_responses_event;
pub use responses::spawn_response_stream;
