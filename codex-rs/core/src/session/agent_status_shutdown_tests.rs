use super::tests::make_session_and_context;
use codex_protocol::protocol::AgentStatus;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn coalesced_shutdown_retains_the_real_turn_error() {
    let (session, _) = make_session_and_context().await;
    let status = session.agent_status.subscribe();
    let error = AgentStatus::Errored("injected invalid prompt".to_owned());

    // No consumer poll occurs between publication of the terminal result and
    // shutdown, just as when the memory consumer is awaiting a DB heartbeat.
    session.publish_agent_status(error.clone()).await;
    session.publish_agent_status(AgentStatus::Shutdown).await;

    assert_eq!(*status.borrow(), AgentStatus::Shutdown);
    assert_eq!(
        *session.last_terminal_agent_status.lock().await,
        Some(error)
    );
}

#[tokio::test]
async fn successful_completion_replaces_an_earlier_turn_error_before_shutdown() {
    let (session, _) = make_session_and_context().await;
    session
        .publish_agent_status(AgentStatus::Errored("transient".to_owned()))
        .await;
    let completed = AgentStatus::Completed(Some("recovered".to_owned()));
    session.publish_agent_status(completed.clone()).await;
    session.publish_agent_status(AgentStatus::Shutdown).await;

    assert_eq!(
        *session.last_terminal_agent_status.lock().await,
        Some(completed)
    );
}

#[tokio::test]
async fn shutdown_of_a_new_turn_does_not_inherit_the_previous_turn_error() {
    let (session, _) = make_session_and_context().await;
    session
        .publish_agent_status(AgentStatus::Errored("previous turn".to_owned()))
        .await;
    session.publish_agent_status(AgentStatus::Running).await;
    session.publish_agent_status(AgentStatus::Shutdown).await;

    assert_eq!(*session.last_terminal_agent_status.lock().await, None);
}
