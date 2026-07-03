use anyhow::Result;
use app_test_support::TestAppServer;
use app_test_support::to_response;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ThreadProfileRefreshResponse;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::WarningNotification;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;
use tokio::time::Duration;
use tokio::time::timeout;

const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::test]
async fn thread_profile_refresh_returns_response_and_warning_without_turn() -> Result<()> {
    let codex_home = TempDir::new()?;
    let mut mcp = TestAppServer::new_with_auto_env(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let thread_start_id = mcp
        .send_thread_start_request_with_auto_env(ThreadStartParams::default())
        .await?;
    let thread_start_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(thread_start_id)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response(thread_start_resp)?;

    let refresh_id = mcp
        .send_raw_request(
            "thread/profile/refresh",
            Some(json!({ "threadId": thread.id.clone() })),
        )
        .await?;
    let refresh_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(refresh_id)),
    )
    .await??;
    let _: ThreadProfileRefreshResponse = to_response(refresh_resp)?;

    let warning = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("warning"),
    )
    .await??;
    let warning: WarningNotification =
        serde_json::from_value(warning.params.expect("warning params should be present"))?;
    assert_eq!(warning.thread_id.as_deref(), Some(thread.id.as_str()));
    assert!(
        warning
            .message
            .contains("No managed profile failover is configured"),
        "unexpected warning: {}",
        warning.message
    );

    let turn_started = timeout(
        Duration::from_millis(250),
        mcp.read_stream_until_notification_message("turn/started"),
    )
    .await;
    assert!(
        turn_started.is_err(),
        "profile refresh should not start a turn"
    );

    Ok(())
}
