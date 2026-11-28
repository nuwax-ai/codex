//! Debug-only capture of the model wire, for a developer's own machine.
//!
//! Enable with `NUWAX_RIG_WIRE_CAPTURE_DIR=<dir>` (the fork's namespace; the
//! older `CODEX_RIG_WIRE_CAPTURE_DIR` still works as an alias). Each process gets
//! its own `<dir>/<epoch>-<pid>/` run directory — two processes sharing one value
//! must not overwrite each other's evidence — and every model request leaves three
//! files there, named by that process's sequence number:
//!
//! ```text
//! 0001-anthropic.request.json   method, url, headers, body (parsed when JSON)
//! 0001-anthropic.request.body   the exact request bytes
//! 0001-anthropic.response.sse   the exact response bytes, chunk by chunk
//! ```
//!
//! Why this exists: neither of the two shipped captures can answer "did the
//! provider actually send a `text_delta`?". `CODEX_RIG_REQUEST_CAPTURE_FILE`
//! records requests only, and the tracing log holds rig's *parsed* view of a
//! reply. This tee sits on the transport, so it sees the bytes in both
//! directions before rig parses them.
//!
//! This is a local debugging aid: it writes prompts and completions verbatim
//! and applies no redaction, no rotation and no size cap. Do not enable it
//! where those bodies must not land on disk.

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use bytes::Bytes;
use futures::StreamExt;

use rig_core::http_client::BoxedStream;
use rig_core::http_client::LazyBody;

/// The fork's name for the capture directory.
const ENV_VAR: &str = "NUWAX_RIG_WIRE_CAPTURE_DIR";
/// Pre-rename spelling, still honored so an existing local workflow keeps working.
const LEGACY_ENV_VAR: &str = "CODEX_RIG_WIRE_CAPTURE_DIR";

static CAPTURE: OnceLock<Option<Arc<Capture>>> = OnceLock::new();

/// The process-wide capture, when the environment enables it.
pub(crate) fn enabled() -> Option<&'static Arc<Capture>> {
    CAPTURE
        .get_or_init(|| {
            // An empty value counts as unset, so a stray `export VAR=` cannot shadow the
            // other spelling.
            let dir = [ENV_VAR, LEGACY_ENV_VAR]
                .into_iter()
                .filter_map(std::env::var_os)
                .find(|dir| !dir.is_empty())?;
            match Capture::at(PathBuf::from(dir)) {
                Ok(capture) => Some(Arc::new(capture)),
                Err(error) => {
                    tracing::warn!(
                        %error,
                        "the wire capture directory is set but unusable; wire capture is off"
                    );
                    None
                }
            }
        })
        .as_ref()
}

pub(crate) struct Capture {
    dir: PathBuf,
    sequence: AtomicU64,
}

impl Capture {
    pub(crate) fn at(dir: PathBuf) -> std::io::Result<Self> {
        // One run directory per process: two harness processes pointed at the same
        // value would otherwise both start at 0001 and overwrite each other.
        // Second-resolution time alone is not unique: a test (or a host that restarts the
        // bridge quickly) can build two captures inside one second.
        static RUN_SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis())
            .unwrap_or_default();
        let run = RUN_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let run_dir = dir.join(format!("{millis}-{}-{run}", std::process::id()));
        create_private_dir_all(&run_dir)?;
        Ok(Self {
            dir: run_dir,
            sequence: AtomicU64::new(0),
        })
    }

    /// Opens one capture slot for a request: writes the request body and its
    /// summary, and returns the handle that tees the response into the slot.
    pub(crate) fn begin(
        self: &Arc<Self>,
        protocol: &str,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Option<Arc<Slot>> {
        let index = self.sequence.fetch_add(1, Ordering::Relaxed) + 1;
        let stem = self.dir.join(format!("{index:04}-{protocol}"));
        let body_path = stem.with_extension("request.body");
        let json_path = stem.with_extension("request.json");
        let response_path = stem.with_extension("response.sse");

        if let Err(error) = write_private(&body_path, body) {
            tracing::warn!(
                path = %body_path.display(),
                %error,
                "wire capture: cannot write the request body; capture is off for this call"
            );
            return None;
        }
        let parsed = serde_json::from_slice::<serde_json::Value>(body).ok();
        let summary = serde_json::json!({
            "method": method,
            "url": url,
            "headers": headers
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect::<std::collections::BTreeMap<_, _>>(),
            "bodyBytes": body.len(),
            "body": parsed,
            "files": {
                "request": body_path.display().to_string(),
                "response": response_path.display().to_string(),
            },
        });
        if let Err(error) = write_private(&json_path, format!("{summary:#}\n").as_bytes()) {
            tracing::warn!(
                path = %json_path.display(),
                %error,
                "wire capture: cannot write the request summary"
            );
        }

        // Keep the file open for the whole response: chunks arrive over time
        // and this slot is the only writer.
        let file = match create_private(&response_path) {
            Ok(file) => file,
            Err(error) => {
                tracing::warn!(
                    path = %response_path.display(),
                    %error,
                    "wire capture: cannot create the response file; capture is off for this call"
                );
                return None;
            }
        };
        Some(Arc::new(Slot {
            label: format!("{index:04}-{protocol}"),
            cap_bytes: MAX_RESPONSE_BYTES,
            state: Mutex::new(SlotState {
                file: Some(file),
                written: 0,
            }),
        }))
    }
}

/// Creates a directory tree readable only by its owner (mode bits are ignored where the
/// platform has none).
fn create_private_dir_all(path: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

/// Creates a file readable only by its owner. Captures carry prompts and provider
/// credentials in headers, so the default `0644` under a `022` umask is too wide.
fn create_private(path: &Path) -> std::io::Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Writes a whole file with owner-only permissions, replacing any earlier copy.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = create_private(path)?;
    file.write_all(bytes)
}

/// Per-response byte cap. A runaway stream would otherwise fill the disk of the machine
/// this is meant to debug; the cap is reported, never silent.
const MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;

/// One model call's response sink.
pub(crate) struct Slot {
    label: String,
    cap_bytes: u64,
    /// `None` once the response is complete (or the cap was hit), so a late chunk cannot
    /// reopen it.
    state: Mutex<SlotState>,
}

#[derive(Default)]
struct SlotState {
    file: Option<File>,
    written: u64,
}

impl Slot {
    fn append(&self, bytes: &[u8]) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let Some(mut file) = state.file.take() else {
            return;
        };
        if state.written.saturating_add(bytes.len() as u64) > self.cap_bytes {
            tracing::warn!(
                label = %self.label,
                cap_bytes = self.cap_bytes,
                "wire capture: response cap reached; the rest of this response is not captured"
            );
            return;
        }
        match file.write_all(bytes) {
            Ok(()) => {
                state.written += bytes.len() as u64;
                state.file = Some(file);
            }
            Err(error) => {
                tracing::warn!(label = %self.label, %error, "wire capture: write failed");
            }
        }
    }

    fn finish(&self) {
        if let Ok(mut state) = self.state.lock()
            && let Some(file) = state.file.take()
        {
            let _ = file.sync_all();
        }
    }

    /// Records an error that arrived instead of a response body.
    pub(crate) fn record_error(&self, error: &str) {
        self.append(format!("\n[transport error] {error}\n").as_bytes());
        self.finish();
    }

    /// Copies a streaming body into the capture while passing it through.
    pub(crate) fn tee_stream(self: &Arc<Self>, body: BoxedStream) -> BoxedStream {
        let slot = Arc::clone(self);
        Box::pin(body.map(move |chunk| {
            match &chunk {
                Ok(bytes) => slot.append(bytes),
                Err(error) => slot.record_error(&error.to_string()),
            }
            chunk
        })) as BoxedStream
    }

    /// Copies a lazily-read body into the capture while passing it through.
    pub(crate) fn tee_lazy(self: &Arc<Self>, body: LazyBody) -> LazyBody {
        let slot = Arc::clone(self);
        Box::pin(async move {
            let result = body.await;
            match &result {
                Ok(bytes) => {
                    slot.append(bytes);
                    slot.finish();
                }
                Err(error) => slot.record_error(&error.to_string()),
            }
            result
        }) as LazyBody
    }
}

/// Starts a capture slot for a request about to be sent.
pub(crate) fn begin_for(
    request: &rig_core::http_client::Request<Bytes>,
    protocol: &str,
) -> Option<Arc<Slot>> {
    let capture = enabled()?;
    let headers: Vec<(String, String)> = request
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().unwrap_or("<non-utf8>").to_string(),
            )
        })
        .collect();
    capture.begin(
        protocol,
        request.method().as_str(),
        &request.uri().to_string(),
        &headers,
        request.body(),
    )
}

#[cfg(test)]
#[path = "wire_capture_tests.rs"]
mod tests;
