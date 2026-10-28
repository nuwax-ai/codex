//! Fork (nuwax-codex): NUWAX_-prefixed aliases for user-facing CODEX_*
//! environment variables (`NUWAX_HOME` for `CODEX_HOME`, the config-seed
//! pairs in `codex-utils-cli`). Resolution rules, shared by every consumer:
//!
//! - only `NUWAX_*` set → it wins;
//! - only `CODEX_*` set → it wins (permanent compatibility);
//! - both set to the same value → fine (idempotent migration);
//! - both set with differing values → hard error naming both variables: a
//!   split-brain configuration (two homes, two windows) must not be silently
//!   resolved, matching the fork's fail-fast env policy.
//!
//! Values flow through as raw `OsString`s: Unicode validation stays at each
//! consumer's pre-existing boundary, so an unreadable value behaves exactly
//! as it did before aliasing (the seeds' `-c`-precedence check still runs
//! before their Unicode error; the home resolver still ignores non-Unicode
//! values). A truly empty value counts as unset (the `CODEX_HOME`
//! precedent); blank-but-nonempty values are left to the consumer.

/// Resolves a `NUWAX_*`/`CODEX_*` alias pair from the process environment.
/// Returns the winning value plus whether it came from the `NUWAX_*` name
/// (callers use it to name the right variable in their own error messages).
pub fn resolve_env_alias(
    nuwax_name: &str,
    codex_name: &str,
) -> Result<Option<(std::ffi::OsString, bool)>, String> {
    resolve_alias_values(
        std::env::var_os(nuwax_name),
        std::env::var_os(codex_name),
        nuwax_name,
        codex_name,
    )
}

/// Pure core of [`resolve_env_alias`] for tests; `empty means unset` and the
/// conflict rules live here.
fn resolve_alias_values(
    nuwax: Option<std::ffi::OsString>,
    codex: Option<std::ffi::OsString>,
    nuwax_name: &str,
    codex_name: &str,
) -> Result<Option<(std::ffi::OsString, bool)>, String> {
    let nuwax = nuwax.filter(|value| !value.is_empty());
    let codex = codex.filter(|value| !value.is_empty());
    match (nuwax, codex) {
        (Some(nuwax), Some(codex)) if nuwax == codex => Ok(Some((nuwax, true))),
        (Some(nuwax), Some(codex)) => Err(format!(
            "{nuwax_name}={nuwax:?} and {codex_name}={codex:?} are both set but differ; \
             set only one (NUWAX_* is the preferred spelling)"
        )),
        (Some(nuwax), None) => Ok(Some((nuwax, true))),
        (None, Some(codex)) => Ok(Some((codex, false))),
        (None, None) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_alias_values;
    use std::ffi::OsString;

    const NUWAX: &str = "NUWAX_TEST_VAR";
    const CODEX: &str = "CODEX_TEST_VAR";

    fn value(raw: &str) -> OsString {
        OsString::from(raw)
    }

    #[test]
    fn unset_returns_none() {
        assert_eq!(
            resolve_alias_values(None, None, NUWAX, CODEX).unwrap(),
            None
        );
    }

    #[test]
    fn nuwax_only_wins() {
        assert_eq!(
            resolve_alias_values(Some(value("/a")), None, NUWAX, CODEX).unwrap(),
            Some((value("/a"), true))
        );
    }

    #[test]
    fn codex_only_wins_for_compatibility() {
        assert_eq!(
            resolve_alias_values(None, Some(value("/a")), NUWAX, CODEX).unwrap(),
            Some((value("/a"), false))
        );
    }

    #[test]
    fn both_set_same_value_is_idempotent() {
        assert_eq!(
            resolve_alias_values(Some(value("/a")), Some(value("/a")), NUWAX, CODEX).unwrap(),
            Some((value("/a"), true))
        );
    }

    #[test]
    fn both_set_differing_values_errors_naming_both() {
        let err = resolve_alias_values(Some(value("/a")), Some(value("/b")), NUWAX, CODEX)
            .expect_err("conflicting pair must error");
        assert!(err.contains(NUWAX) && err.contains(CODEX), "err: {err}");
        assert!(err.contains("/a") && err.contains("/b"), "err: {err}");
    }

    #[test]
    fn truly_empty_values_count_as_unset() {
        assert_eq!(
            resolve_alias_values(Some(value("")), None, NUWAX, CODEX).unwrap(),
            None
        );
        // An empty NUWAX_* must not shadow a real CODEX_* value.
        assert_eq!(
            resolve_alias_values(Some(value("")), Some(value("/a")), NUWAX, CODEX).unwrap(),
            Some((value("/a"), false))
        );
    }

    #[test]
    fn non_unicode_values_flow_through_for_consumer_boundaries() {
        use std::os::unix::ffi::OsStringExt;
        let raw = OsString::from_vec(vec![0xff]);
        assert_eq!(
            resolve_alias_values(Some(raw.clone()), None, NUWAX, CODEX).unwrap(),
            Some((raw, true))
        );
    }
}
