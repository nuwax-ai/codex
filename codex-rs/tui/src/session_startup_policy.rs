//! Shared launch policy for session-scoped CLI commands. CLI key/value
//! overrides and the `NUWAX_*` environment group flow through the same
//! resolver as the interactive TUI startup, so administrative commands and
//! queue resolve the identical seed set instead of each re-deriving it.

use crate::Cli;
use codex_config::TomlValue;

/// Why a session command is starting an app server. Administrative actions
/// never write turns, so they may run embedded with environment seeds like
/// the interactive TUI; queue must reach the thread's owner server.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionCommandPurpose {
    Administrative,
    Queue,
}

/// The launch channel for session commands: parsed `-c` pairs plus the
/// separately-attributed NUWAX environment seeds, using the interactive
/// startup's exact precedence (typed `-m`/`--oss` selections suppress the
/// group; an explicit other-provider `-c` makes it irrelevant).
pub(crate) fn resolve_session_launch_overrides(
    cli: &Cli,
    cli_kv_overrides: Vec<(String, TomlValue)>,
) -> Result<codex_config::LaunchOverrides, String> {
    let env_seed_overrides = codex_utils_cli::nuwax_env_overrides(
        codex_utils_cli::nuwax_env_from_process(),
        cli.shared.model.as_deref(),
        cli.shared
            .oss
            .then_some("oss")
            .or_else(|| cli.shared.oss_provider.as_deref()),
        &cli_kv_overrides,
    )?;
    Ok(codex_config::LaunchOverrides {
        cli_overrides: cli_kv_overrides,
        env_seed_overrides,
    })
}
