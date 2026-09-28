// Fork (nuwax-codex): layered default config home.
//
// nuwax-codex prefers `~/.codex-nuwax` over the upstream `~/.codex` default so
// fork-specific configuration (model providers with fork-only keys such as
// `experimental_bridge` or `wire_api = "anthropic"`) never has to be written
// into the shared `~/.codex/config.toml`, where it fails the official codex
// binary's strict config validation. The official binary keeps resolving its
// own `~/.codex` and is unaffected by the presence of `~/.codex-nuwax`.
//
// Resolution order (the `CODEX_HOME` environment variable is handled by
// `find_codex_home_from_env` and always wins before this module runs):
//   1. `~/.codex-nuwax` when it exists and is a directory — full isolation
//      (config, auth, sessions, logs, history all live there);
//   2. `~/.codex` — upstream default, shared with official codex.
//
// A `~/.codex-nuwax` that exists but is not a usable directory (regular file,
// dangling symlink) is a hard error: a half-configured isolation directory
// must be loud, not silently ignored.

use codex_utils_absolute_path::AbsolutePathBuf;
use std::io;
use std::path::Path;

/// Resolves the default (no `CODEX_HOME`) config home under `user_home`:
/// `user_home/.codex-nuwax` when it exists and is a directory, otherwise
/// `user_home/.codex` (unvalidated, matching the upstream default so a fresh
/// install still works out of the box). The selected path is returned as-is,
/// never canonicalized, mirroring the upstream default-home semantics for a
/// symlinked home directory.
pub(crate) fn find_default_codex_home(user_home: &Path) -> io::Result<AbsolutePathBuf> {
    let nuwax_home = user_home.join(".codex-nuwax");
    match std::fs::symlink_metadata(&nuwax_home) {
        // Nothing at the fork path: keep the upstream default.
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            AbsolutePathBuf::from_absolute_path(user_home.join(".codex"))
        }
        Err(err) => Err(io::Error::new(
            err.kind(),
            format!("failed to inspect {}: {err}", nuwax_home.display()),
        )),
        Ok(_) => match std::fs::metadata(&nuwax_home) {
            // Follows symlinks: a link to a real directory counts.
            Ok(metadata) if metadata.is_dir() => AbsolutePathBuf::from_absolute_path(nuwax_home),
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "{} exists but is not a directory; remove it or make it a directory to \
                     isolate the nuwax-codex config home",
                    nuwax_home.display()
                ),
            )),
            Err(err) => Err(io::Error::new(
                err.kind(),
                format!(
                    "{} is not a usable directory: {err}; fix or remove it to isolate the \
                     nuwax-codex config home",
                    nuwax_home.display()
                ),
            )),
        },
    }
}

#[cfg(test)]
#[path = "nuwax_default_tests.rs"]
mod tests;
