use std::fs;
use std::sync::Arc;

use anyhow::Result;
use codex_core::config::ProfileAuthCandidateConfig;
use codex_core::config::ProfileAuthFailoverConfig;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_response_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::sse_response;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;
use wiremock::ResponseTemplate;

fn write_api_auth(path: &std::path::Path, key: &str) -> Result<()> {
    fs::write(
        path,
        format!(r#"{{"auth_mode":"apikey","OPENAI_API_KEY":"{key}"}}"#),
    )?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn usage_limit_switches_profile_and_retries_turn() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let home = Arc::new(TempDir::new()?);
    let codex_home = home.path().to_path_buf();
    let profiles = home.path().join("profiles");
    let main = profiles.join("main");
    let backup = profiles.join("backup");
    fs::create_dir_all(&main)?;
    fs::create_dir_all(&backup)?;
    write_api_auth(&codex_home.join("auth.json"), "sk-main")?;
    write_api_auth(&main.join("auth.json"), "sk-main")?;
    write_api_auth(&backup.join("auth.json"), "sk-backup")?;
    let main_auth_file = main.join("auth.json");
    let backup_auth_file = backup.join("auth.json");
    let expected_auth_file = backup_auth_file.clone();

    let usage_limit_response = ResponseTemplate::new(429).set_body_json(json!({
        "error": {
            "type": "usage_limit_reached",
            "message": "limit reached"
        }
    }));
    let retry_response = sse_response(sse(vec![
        ev_response_created("resp-2"),
        ev_assistant_message("msg-1", "retried"),
        ev_completed("resp-2"),
    ]));
    let responses_mock =
        mount_response_sequence(&server, vec![usage_limit_response, retry_response]).await;

    let mut builder = test_codex()
        .with_home(Arc::clone(&home))
        .with_config(move |config| {
            config.profile_auth_failover = Some(ProfileAuthFailoverConfig {
                active_profile: "main".to_string(),
                candidates: vec![
                    ProfileAuthCandidateConfig {
                        name: "main".to_string(),
                        auth_file: main_auth_file,
                        limit_file: None,
                    },
                    ProfileAuthCandidateConfig {
                        name: "backup".to_string(),
                        auth_file: backup_auth_file,
                        limit_file: None,
                    },
                ],
            });
        });
    let test = builder.build(&server).await?;

    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "retry after usage limit".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await?;

    let warning = wait_for_event(&test.codex, |event| matches!(event, EventMsg::Warning(_))).await;
    let EventMsg::Warning(warning) = warning else {
        unreachable!();
    };
    assert_eq!(
        warning.message,
        "Usage limit reached; switched to profile `backup` and retrying."
    );
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert_eq!(responses_mock.requests().len(), 2);
    assert_eq!(
        fs::read_to_string(codex_home.join("auth.json"))?,
        fs::read_to_string(expected_auth_file)?
    );

    Ok(())
}
