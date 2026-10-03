use super::AppServerTarget;
use super::RemoteAppServerEndpoint;
use super::app_server_target_for_launch;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_cli::NuwaxEnvInput;
use codex_utils_cli::nuwax_env_overrides;
use pretty_assertions::assert_eq;

#[test]
fn app_server_target_for_launch_rejects_active_nuwax_group_for_remote() -> color_eyre::Result<()> {
    for wire_api in ["responses", "chat", "anthropic"] {
        let seeds = nuwax_env_overrides(
            NuwaxEnvInput {
                model: Some("client-model".into()),
                base_url: Some("https://client.example/v1".into()),
                wire_api: Some(wire_api.into()),
                api_key: Some("client-secret".into()),
                ..Default::default()
            },
            /*cli_model*/ None,
            /*cli_provider*/ None,
            &[],
        )
        .expect("valid temporary provider");
        let endpoint = RemoteAppServerEndpoint::UnixSocket {
            socket_path: AbsolutePathBuf::relative_to_current_dir("server.sock")?,
        };
        let error = app_server_target_for_launch(
            Some(endpoint),
            /*default_daemon_socket*/ None,
            /*can_reuse_implicit_local_daemon*/ false,
            /*workload_identity_selected*/ false,
            /*exec_server_url*/ None,
            &seeds,
        )
        .expect_err("client credentials cannot configure a remote server");
        if wire_api == "chat" {
            insta::assert_snapshot!("remote_nuwax_provider_error", error.to_string());
        }
        assert_eq!(
            (error.kind(), error.to_string()),
            (
                std::io::ErrorKind::InvalidInput,
                "NUWAX environment provider must be configured on the remote app-server host; unset the local NUWAX provider group or omit --remote".to_string(),
            ),
            "wire {wire_api}",
        );
        assert_eq!(
            app_server_target_for_launch(
                /*explicit_remote_endpoint*/ None, /*default_daemon_socket*/ None,
                /*can_reuse_implicit_local_daemon*/ false,
                /*workload_identity_selected*/ false, /*exec_server_url*/ None, &seeds,
            )?,
            AppServerTarget::Embedded,
        );
    }
    Ok(())
}

#[test]
fn app_server_target_for_launch_allows_remote_when_nuwax_group_is_inactive()
-> color_eyre::Result<()> {
    let complete_group = NuwaxEnvInput {
        model: Some("client-model".into()),
        base_url: Some("https://client.example/v1".into()),
        wire_api: Some("chat".into()),
        api_key: Some("client-secret".into()),
        ..Default::default()
    };
    for (input, cli_provider, existing) in [
        (NuwaxEnvInput::default(), None, Vec::new()),
        (
            NuwaxEnvInput {
                model: Some("server-model".into()),
                ..Default::default()
            },
            None,
            Vec::new(),
        ),
        (complete_group.clone(), Some("server-provider"), Vec::new()),
        (
            complete_group,
            None,
            vec![(
                "model_provider".to_string(),
                toml::Value::String("server-provider".into()),
            )],
        ),
    ] {
        let seeds = nuwax_env_overrides(input, /*cli_model*/ None, cli_provider, &existing)
            .expect("inactive provider group");
        let endpoint = RemoteAppServerEndpoint::UnixSocket {
            socket_path: AbsolutePathBuf::relative_to_current_dir("server.sock")?,
        };
        assert_eq!(
            app_server_target_for_launch(
                Some(endpoint.clone()),
                /*default_daemon_socket*/ None,
                /*can_reuse_implicit_local_daemon*/ false,
                /*workload_identity_selected*/ false,
                /*exec_server_url*/ None,
                &seeds,
            )?,
            AppServerTarget::Remote { endpoint },
        );
    }
    Ok(())
}
