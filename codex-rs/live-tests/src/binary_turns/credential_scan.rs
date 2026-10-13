//! Byte-level verification of temporary homes and retained live-test evidence.

use anyhow::Context;
use anyhow::Result;
use std::io::Read;
use std::path::Path;

pub(super) fn verify(roots: &[&Path], api_key: &str) -> Result<()> {
    anyhow::ensure!(
        !api_key.is_empty(),
        "credential scan requires a nonempty key"
    );
    let json_key = serde_json::to_string(api_key)?;
    let escaped_key = &json_key.as_bytes()[1..json_key.len() - 1];
    let mut needles = vec![
        api_key.as_bytes(),
        b"experimental_bearer_token".as_slice(),
        b"model_providers.nuwax_env".as_slice(),
    ];
    // JSON artifacts escape quotes/backslashes inside keys, including keys
    // embedded in larger strings such as an Authorization value.
    if escaped_key != api_key.as_bytes() {
        needles.push(escaped_key);
    }
    let mut scanned = 0usize;
    let mut directories = roots
        .iter()
        .map(|root| root.to_path_buf())
        .collect::<Vec<_>>();
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            if kind.is_dir() {
                directories.push(path);
                continue;
            }
            // Following a directory link can escape the scene or loop. Such
            // evidence cannot be certified; never silently count it as clean.
            anyhow::ensure!(
                kind.is_file(),
                "cannot verify non-regular artifact {path:?}"
            );
            let mut file = std::fs::File::open(&path)
                .with_context(|| format!("cannot open credential scan input {path:?}"))?;
            let found = contains_secret(&mut file, &needles)
                .with_context(|| format!("cannot read credential scan input {path:?}"))?;
            anyhow::ensure!(
                !found,
                "credential or ephemeral provider definition leaked into {path:?}"
            );
            scanned += 1;
        }
    }
    anyhow::ensure!(scanned > 0, "expected at least a config.toml to scan");
    Ok(())
}

fn contains_secret(reader: &mut impl Read, needles: &[&[u8]]) -> std::io::Result<bool> {
    let overlap = needles
        .iter()
        .map(|needle| needle.len())
        .max()
        .unwrap_or_default()
        .saturating_sub(1);
    let mut window = Vec::with_capacity(8192 + overlap);
    let mut chunk = [0u8; 8192];
    loop {
        let count = match reader.read(&mut chunk) {
            Ok(0) => return Ok(false),
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        window.extend_from_slice(&chunk[..count]);
        if needles
            .iter()
            .any(|needle| window.windows(needle.len()).any(|bytes| bytes == *needle))
        {
            return Ok(true);
        }
        window.drain(..window.len().saturating_sub(overlap));
    }
}

#[cfg(test)]
#[path = "credential_scan_tests.rs"]
mod tests;
