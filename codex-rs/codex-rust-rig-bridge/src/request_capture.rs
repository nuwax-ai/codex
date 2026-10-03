//! Explicit local audit capture. Bodies may contain sensitive prompts and tool
//! output; only authentication headers and URL credentials are excluded.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use http::Request;
use rig_core::http_client::Error;
use serde::Serialize;

const MAX_ATTEMPTS: usize = 64;
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_CAPTURE_BYTES: usize = 16 * 1024 * 1024;
static FILE_WRITE_LOCK: Mutex<()> = Mutex::new(());

/// One HTTP attempt after all protocol rewrites. `body_raw` preserves the
/// exact UTF-8 bytes; `body` is a convenience view, not serialization evidence.
#[derive(Clone, Debug, Serialize)]
pub struct RecordedRequest {
    pub method: String,
    pub url: String,
    pub body_raw: String,
    pub body: serde_json::Value,
}

/// Bounded attempt history. Read only after the response stream has drained.
#[derive(Clone, Debug, Default, Serialize)]
pub struct FinalRequestCapture {
    pub requests: Vec<RecordedRequest>,
    #[serde(skip)]
    bytes: usize,
    #[serde(skip)]
    file: Option<PathBuf>,
}

pub type FinalRequestRecorder = Option<Arc<Mutex<FinalRequestCapture>>>;

pub(crate) fn from_env() -> Result<FinalRequestRecorder, codex_api::ApiError> {
    match std::env::var_os("CODEX_RIG_REQUEST_CAPTURE_FILE") {
        None => Ok(None),
        Some(path) if path.is_empty() => Err(codex_api::ApiError::InvalidRequest {
            message: "CODEX_RIG_REQUEST_CAPTURE_FILE must be a nonempty path".into(),
        }),
        Some(path) => Ok(Some(Arc::new(Mutex::new(FinalRequestCapture {
            file: Some(PathBuf::from(path)),
            ..Default::default()
        })))),
    }
}

#[derive(Debug)]
struct RequestCaptureError(String);

impl std::fmt::Display for RequestCaptureError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for RequestCaptureError {}

fn capture_error(message: impl Into<String>) -> Error {
    Error::Instance(Box::new(RequestCaptureError(message.into())))
}

pub(crate) fn api_error(
    mut error: &(dyn std::error::Error + 'static),
) -> Option<codex_api::ApiError> {
    loop {
        if let Some(error) = error.downcast_ref::<RequestCaptureError>() {
            return Some(codex_api::ApiError::InvalidRequest {
                message: error.to_string(),
            });
        }
        let source = error.source()?;
        error = source;
    }
}

pub(crate) fn record(
    recorder: &FinalRequestRecorder,
    request: &Request<bytes::Bytes>,
) -> Result<(), Error> {
    let Some(recorder) = recorder else {
        return Ok(());
    };
    if request.body().len() > MAX_BODY_BYTES {
        return Err(capture_error("request capture body exceeds 2 MiB limit"));
    }
    let mut url = reqwest_rig::Url::parse(&request.uri().to_string())
        .map_err(|_| capture_error("request capture URL is invalid"))?;
    url.set_username("")
        .map_err(|()| capture_error("cannot redact URL username"))?;
    url.set_password(None)
        .map_err(|()| capture_error("cannot redact URL password"))?;
    let names: Vec<String> = url
        .query_pairs()
        .map(|(name, _)| name.into_owned())
        .collect();
    if !names.is_empty() {
        url.query_pairs_mut()
            .clear()
            .extend_pairs(names.iter().map(|name| (name.as_str(), "REDACTED")));
    }
    let body_raw = std::str::from_utf8(request.body())
        .map_err(|_| capture_error("request capture body is not UTF-8"))?
        .to_owned();
    // Value cannot represent every numeric literal accepted by RawValue. The
    // convenience view must never reject or re-encode a valid raw request.
    let body = serde_json::from_str(&body_raw).unwrap_or(serde_json::Value::Null);
    let entry = RecordedRequest {
        method: request.method().to_string(),
        url: url.to_string(),
        body_raw,
        body,
    };
    let mut encoded = serde_json::to_vec(&entry)
        .map_err(|_| capture_error("cannot serialize request capture"))?;
    encoded.push(b'\n');
    let mut capture = recorder
        .lock()
        .map_err(|_| capture_error("request capture lock poisoned"))?;
    if capture.requests.len() >= MAX_ATTEMPTS
        || capture.bytes.saturating_add(encoded.len()) > MAX_CAPTURE_BYTES
    {
        return Err(capture_error(
            "request capture exceeds attempt or total byte limit",
        ));
    }
    if let Some(file) = &capture.file {
        append_file(file, &encoded)?;
    }
    capture.bytes += encoded.len();
    capture.requests.push(entry);
    Ok(())
}

fn append_file(path: &std::path::Path, encoded: &[u8]) -> Result<(), Error> {
    use std::io::Write;
    let _guard = FILE_WRITE_LOCK
        .lock()
        .map_err(|_| capture_error("request capture file lock poisoned"))?;
    let existing = match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => {
            if metadata.len() > MAX_CAPTURE_BYTES as u64 {
                return Err(capture_error("request capture file exceeds byte limit"));
            }
            std::fs::read(path).map_err(|_| capture_error("cannot read request capture file"))?
        }
        Ok(_) => return Err(capture_error("request capture path must be a regular file")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(_) => return Err(capture_error("cannot inspect request capture file")),
    };
    if existing.iter().filter(|byte| **byte == b'\n').count() >= MAX_ATTEMPTS
        || existing.len().saturating_add(encoded.len()) > MAX_CAPTURE_BYTES
    {
        return Err(capture_error(
            "request capture file exceeds attempt or total byte limit",
        ));
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|_| capture_error("cannot open request capture file"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|_| capture_error("cannot restrict request capture file permissions"))?;
    }
    file.write_all(encoded)
        .map_err(|_| capture_error("cannot append request capture file"))
}

#[cfg(test)]
#[path = "request_capture_tests.rs"]
mod tests;
