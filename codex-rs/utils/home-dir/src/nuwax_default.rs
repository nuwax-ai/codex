// Fork: the default config home is isolated from upstream Codex.
//
// `CODEX_HOME` is handled by `find_codex_home_from_env` and always wins before
// this module runs. Otherwise the default is always `~/.nuwax-harness`, even
// when a legacy Codex home exists. A missing default may be initialized by the
// caller. An existing file or unusable symlink is an error; a valid directory
// symlink is returned as the link path without canonicalization.

use codex_utils_absolute_path::AbsolutePathBuf;
use std::io;
use std::path::Path;

/// Resolves the default (no `CODEX_HOME`) config home under `user_home`.
/// The selected `user_home/.nuwax-harness` path is returned as-is, never
/// canonicalized. It need not exist, but an existing entry must resolve to a
/// directory.
pub(crate) fn find_default_codex_home(user_home: &Path) -> io::Result<AbsolutePathBuf> {
    let nuwax_home = user_home.join(".nuwax-harness");
    match std::fs::symlink_metadata(&nuwax_home) {
        // A fresh install may initialize the fork-specific default.
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            AbsolutePathBuf::from_absolute_path(nuwax_home)
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
                     use the nuwax-harness config home",
                    nuwax_home.display()
                ),
            )),
            Err(err) => Err(io::Error::new(
                err.kind(),
                format!(
                    "{} is not a usable directory: {err}; fix or remove it to use the \
                     nuwax-harness config home",
                    nuwax_home.display()
                ),
            )),
        },
    }
}

#[cfg(test)]
#[path = "nuwax_default_tests.rs"]
mod tests;
