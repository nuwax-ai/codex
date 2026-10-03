//! Shared, credential-free source identity for the executable and its verifier.
//! This identifies git-listed build inputs; it is not a signature or a record
//! of Cargo's resolved dependency feature graph.

use sha2::Digest;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use std::time::Duration;
use std::time::Instant;

pub const SOURCE_ALGORITHM: &str = "git-build-input-content-sha256-v1";
const PROCESS_TIMEOUT: Duration = Duration::from_secs(5);
const GIT_OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
const MAX_PIPE_READERS: usize = 16;
static PIPE_READERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

struct PipeReaderLease;

impl PipeReaderLease {
    fn reserve() -> Result<Self, String> {
        use std::sync::atomic::Ordering;
        PIPE_READERS
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < MAX_PIPE_READERS).then_some(count + 1)
            })
            .map(|_| Self)
            .map_err(|_| "identity command pipe reader budget exhausted".into())
    }
}

impl Drop for PipeReaderLease {
    fn drop(&mut self) {
        PIPE_READERS.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

pub struct SourceIdentity {
    pub git_sha: String,
    pub fingerprint: String,
    pub dirty: bool,
    pub inputs: Vec<PathBuf>,
}

/// Executes a child with bounded time and bounded stdout/stderr. Readers send
/// an error as soon as their cap is exceeded, allowing the owner to stop it.
pub fn bounded_output(
    command: &mut Command,
    timeout: Duration,
    stdout_limit: usize,
    stderr_limit: usize,
) -> Result<Vec<u8>, String> {
    // Safe std cannot cancel a blocking pipe owned by a descendant. Reserve
    // before spawning, so timed-out readers and further probes stay bounded.
    let leases = [PipeReaderLease::reserve()?, PipeReaderLease::reserve()?];
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot spawn identity command: {error}"))?;
    let stdout = child.stdout.take().ok_or("missing child stdout")?;
    let stderr = child.stderr.take().ok_or("missing child stderr")?;
    let (sender, receiver) = std::sync::mpsc::channel();
    for ((mut reader, limit, is_stdout), lease) in [
        (Box::new(stdout) as Box<dyn Read + Send>, stdout_limit, true),
        (
            Box::new(stderr) as Box<dyn Read + Send>,
            stderr_limit,
            false,
        ),
    ]
    .into_iter()
    .zip(leases)
    {
        let sender = sender.clone();
        let spawned = std::thread::Builder::new().spawn(move || {
            let _lease = lease;
            let mut bytes = Vec::new();
            let mut buffer = [0u8; 4096];
            let result = loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break Ok(bytes),
                    Ok(count) if count <= limit.saturating_sub(bytes.len()) => {
                        bytes.extend_from_slice(&buffer[..count]);
                    }
                    Ok(_) => break Err("identity command output exceeds limit".to_string()),
                    Err(error) => break Err(format!("identity command output failed: {error}")),
                }
            };
            let _ = sender.send((is_stdout, result));
        });
        if let Err(error) = spawned {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("cannot start identity pipe reader: {error}"));
        }
    }
    drop(sender);
    let deadline = Instant::now() + timeout;
    let mut output = None;
    let mut readers_done = 0;
    let result = 'process: loop {
        while let Ok((is_stdout, read)) = receiver.try_recv() {
            match read {
                Ok(bytes) => {
                    if is_stdout {
                        output = Some(bytes);
                    }
                    readers_done += 1;
                }
                Err(error) => break 'process Err(error),
            }
        }
        if Instant::now() >= deadline {
            break Err("identity command timed out or output could not be drained".to_string());
        }
        match child.try_wait() {
            Ok(Some(status)) if !status.success() => {
                break Err(format!("identity command exited with {status}"));
            }
            Ok(Some(_)) if readers_done == 2 => {
                break output.ok_or("missing identity output".into());
            }
            Ok(_) => {}
            Err(error) => break Err(format!("cannot wait for identity command: {error}")),
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

#[cfg(test)]
#[path = "build_source_identity_tests.rs"]
mod tests;

pub fn git_output(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    bounded_output(
        Command::new("git").arg("-C").arg(root).args(args),
        PROCESS_TIMEOUT,
        GIT_OUTPUT_LIMIT,
        4096,
    )
}

pub fn source_identity(root: &Path) -> Result<SourceIdentity, String> {
    let revision = git_output(root, &["rev-parse", "HEAD"])?;
    let git_sha = std::str::from_utf8(&revision)
        .map_err(|_| "git revision is not UTF-8")?
        .trim()
        .to_string();
    if !matches!(git_sha.len(), 40 | 64) || !git_sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("git did not provide a full revision".into());
    }
    let listed = git_output(
        root,
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ],
    )?;
    let mut paths = listed
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| std::str::from_utf8(path).map(str::to_owned))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "source path is not UTF-8")?;
    paths.retain(|path| is_build_input(path));
    paths.sort();
    paths.dedup();
    if paths.is_empty() {
        return Err("git did not provide any build inputs".into());
    }
    let mut hasher = sha2::Sha256::new();
    hasher.update(SOURCE_ALGORITHM.as_bytes());
    let mut inputs = Vec::new();
    for relative in paths {
        let path = root.join(&relative);
        hasher.update((relative.len() as u64).to_le_bytes());
        hasher.update(relative.as_bytes());
        match std::fs::symlink_metadata(&path) {
            Ok(mut metadata) => {
                if metadata.file_type().is_symlink() {
                    let target = std::fs::read_link(&path).map_err(|error| {
                        format!("cannot read build input link {relative}: {error}")
                    })?;
                    let target_text = target.to_str().ok_or("build input link is not UTF-8")?;
                    let resolved = std::fs::canonicalize(&path).map_err(|error| {
                        format!("cannot resolve build input link {relative}: {error}")
                    })?;
                    if !resolved.starts_with(root) {
                        return Err(format!("build input link leaves source tree: {relative}"));
                    }
                    hasher.update(b"symlink");
                    hasher.update((target_text.len() as u64).to_le_bytes());
                    hasher.update(target_text.as_bytes());
                    metadata = std::fs::metadata(&path).map_err(|error| {
                        format!("cannot inspect build input link {relative}: {error}")
                    })?;
                }
                if !metadata.is_file() {
                    return Err(format!(
                        "non-file build input cannot be identified: {relative}"
                    ));
                }
                hasher.update(b"file");
                hasher.update(metadata.len().to_le_bytes());
                let mut file = std::fs::File::open(&path)
                    .map_err(|error| format!("cannot read build input {relative}: {error}"))?;
                let mut buffer = [0u8; 64 * 1024];
                loop {
                    let count = file
                        .read(&mut buffer)
                        .map_err(|error| format!("cannot hash build input {relative}: {error}"))?;
                    if count == 0 {
                        break;
                    }
                    hasher.update(&buffer[..count]);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => hasher.update(b"deleted"),
            Err(error) => return Err(format!("cannot inspect build input {relative}: {error}")),
        }
        inputs.push(path);
    }
    let status = git_output(
        root,
        &["status", "--porcelain", "-z", "--untracked-files=all"],
    )?;
    Ok(SourceIdentity {
        git_sha,
        fingerprint: format!("{:x}", hasher.finalize()),
        dirty: !status.is_empty(),
        inputs,
    })
}

fn is_build_input(path: &str) -> bool {
    let components: Vec<_> = path.split('/').collect();
    if components.iter().any(|part| {
        matches!(
            *part,
            "target" | "tmp" | "logs" | ".git" | ".aws" | ".codex" | "node_modules"
        ) || part.starts_with(".env")
            || part.ends_with(".sqlite")
            || part.contains(".sqlite-")
            || matches!(
                *part,
                "credentials" | "credentials.json" | "auth.json" | "secrets.json" | ".DS_Store"
            )
    }) {
        return false;
    }
    let source_scope = path.starts_with("codex-rs/")
        || path.starts_with(".cargo/")
        || path.starts_with("scripts/")
        || path.starts_with("vendor/");
    let root_build_input = matches!(
        path,
        "MODULE.bazel"
            | "MODULE.bazel.lock"
            | "BUILD.bazel"
            | "defs.bzl"
            | "Cargo.toml"
            | "Cargo.lock"
            | ".bazelrc"
            | ".gitignore"
            | "justfile"
    );
    source_scope || root_build_input
}
