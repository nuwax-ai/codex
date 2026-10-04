//! Queue user messages through a local or remote app server.

use crate::app_server_session::AppServerSession;
use crate::named_session_lookup::SessionCollection;
use crate::named_session_lookup::lookup;
use crate::session_archive_commands::SessionArchiveCommandOptions;
use crate::session_archive_commands::start_app_server_for_session_command;
use codex_app_server_client::TypedRequestError;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::ThreadQueueAddParams;
use codex_app_server_protocol::ThreadQueueAddResponse;
use codex_app_server_protocol::UserInput;
use codex_app_server_protocol::experimental_required_message;
use codex_protocol::ThreadId;
use codex_utils_home_dir::find_codex_home;
use color_eyre::Report;
use color_eyre::eyre::Result;
use color_eyre::eyre::WrapErr;
use color_eyre::eyre::eyre;
use std::path::Path;
use uuid::Uuid;

const JSONRPC_INVALID_REQUEST: i64 = -32600;
const JSONRPC_METHOD_NOT_FOUND: i64 = -32601;
const THREAD_QUEUE_ADD_METHOD: &str = "thread/queue/add";

/// Where a queued message will execute. The owner server must be able to
/// resolve the thread's provider when the turn eventually runs.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueueOwner {
    SharedDaemon,
    Embedded,
    Remote,
}

/// Reads the thread's persisted provider from its rollout metadata. `None`
/// (missing/compressed/unparsable rollout) skips the preflight honestly.
async fn rollout_model_provider(codex_home: &Path, thread_id: &ThreadId) -> Option<String> {
    let path = codex_rollout::find_thread_path_by_id_str(
        codex_home,
        &thread_id.to_string(),
        /*state_db_ctx*/ None,
    )
    .await
    .ok()??;
    let contents = tokio::fs::read_to_string(&path).await.ok()?;
    let first_line = contents.lines().next()?;
    match codex_rollout::parse_rollout_line(first_line).ok()?.item {
        codex_history::RolloutItem::SessionMeta(meta) => meta.meta.model_provider,
        _ => None,
    }
}

/// Enqueue success does not mean the model turn can run: a temporary
/// per-process provider cannot be resolved by a shared owner server.
pub(crate) async fn ensure_owner_can_resolve_thread_provider(
    codex_home: &Path,
    thread_id: &ThreadId,
    owner: QueueOwner,
) -> Result<()> {
    if owner != QueueOwner::SharedDaemon {
        return Ok(());
    }
    if rollout_model_provider(codex_home, thread_id)
        .await
        .as_deref()
        == Some(codex_protocol::config_types::NUWAX_ENV_PROVIDER_ID)
    {
        return Err(eyre!(
            "thread {thread_id} uses the temporary NUWAX environment provider, which the shared app-server daemon cannot resolve (its credentials are per-process). Queue this message from a client whose environment created the session, or configure a named provider the daemon can load."
        ));
    }
    Ok(())
}

pub async fn run_session_queue_command(
    target: String,
    message: String,
    options: SessionArchiveCommandOptions,
) -> Result<String> {
    if options.cli.no_daemon && options.explicit_remote_endpoint.is_none() {
        return Err(eyre!(
            "--no-daemon cannot be used with codex queue. Queuing must discover the shared server to avoid writing through a separate server."
        ));
    }
    let codex_home = find_codex_home().wrap_err("failed to find Codex home")?;
    let explicit_remote = options.explicit_remote_endpoint.is_some();
    let mut app_server = start_app_server_for_session_command(
        options,
        codex_home.to_path_buf(),
        crate::session_startup_policy::SessionCommandPurpose::Queue,
    )
    .await?;
    if !explicit_remote
        && app_server.uses_embedded_app_server()
        && super::maybe_probe_default_daemon_socket(codex_home.as_path())
            .await
            .is_some()
    {
        return Err(eyre!(
            "cannot queue through an embedded app server while a local app-server daemon is running; remove configuration overrides or use --remote"
        ));
    }
    let implicit_local_daemon = !explicit_remote && !app_server.uses_embedded_app_server();
    let owner = if explicit_remote {
        QueueOwner::Remote
    } else if app_server.uses_embedded_app_server() {
        QueueOwner::Embedded
    } else {
        QueueOwner::SharedDaemon
    };
    let client_message_id = Uuid::now_v7().to_string();

    let (thread_id, response) = match run_session_queue_action_with_app_server(
        &mut app_server,
        codex_home.as_path(),
        &target,
        &message,
        &client_message_id,
        owner,
    )
    .await
    {
        Err(error)
            if (implicit_local_daemon || explicit_remote) && is_unsupported_queue_error(&error) =>
        {
            let server = if explicit_remote {
                "remote app server"
            } else {
                "local app-server daemon"
            };
            return Err(error.wrap_err(format!(
                "the {server} does not support thread/queue/add; update or restart the {server}"
            )));
        }
        result => result?,
    };

    Ok(format!(
        "Queued message {} for thread {}.",
        response.queued_submission.id, thread_id
    ))
}

pub(super) async fn run_session_queue_action_with_app_server(
    app_server: &mut AppServerSession,
    codex_home: &Path,
    target: &str,
    message: &str,
    client_message_id: &str,
    owner: QueueOwner,
) -> Result<(ThreadId, ThreadQueueAddResponse)> {
    let thread_id = if let Ok(thread_id) = ThreadId::from_string(target) {
        thread_id
    } else {
        let thread = lookup(
            app_server,
            codex_home,
            target,
            &[SessionCollection::Active],
            &[
                super::resume_source_kinds(/*include_non_interactive*/ true),
                // An empty filter includes Atlas/ChatGPT sessions.
                Vec::new(),
            ],
            // Queue resolves the target across providers; enqueueing goes
            // through the owner server either way.
            super::named_session_lookup::ProviderFilter::All,
        )
        .await?
        .ok_or_else(|| eyre!("No active session found matching '{target}'."))?;
        ThreadId::from_string(&thread.id)
            .wrap_err_with(|| format!("app server returned invalid session id `{}`", thread.id))?
    };
    ensure_owner_can_resolve_thread_provider(codex_home, &thread_id, owner).await?;
    let request_id = app_server.next_request_id();
    let response = app_server
        .request_handle()
        .request_typed(ClientRequest::ThreadQueueAdd {
            request_id,
            params: ThreadQueueAddParams {
                thread_id: thread_id.to_string(),
                input: vec![UserInput::Text {
                    text: message.to_string(),
                    text_elements: Vec::new(),
                }],
                client_user_message_id: client_message_id.to_string(),
            },
        })
        .await
        .wrap_err("failed to queue session message")?;
    Ok((thread_id, response))
}

fn is_unsupported_queue_error(error: &Report) -> bool {
    matches!(
        error.downcast_ref::<TypedRequestError>(),
        Some(TypedRequestError::Server { method, source })
            if method == THREAD_QUEUE_ADD_METHOD
                && (source.code == JSONRPC_METHOD_NOT_FOUND
                    || (source.code == JSONRPC_INVALID_REQUEST
                        && (source.message == experimental_required_message(THREAD_QUEUE_ADD_METHOD)
                            || source.message.starts_with(
                                "Invalid request: unknown variant `thread/queue/add`",
                            ))))
    )
}

#[cfg(test)]
#[path = "session_queue_commands_tests.rs"]
mod tests;
