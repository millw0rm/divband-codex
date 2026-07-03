use std::time::Duration;

use pretty_assertions::assert_eq;
use rmcp::model::JsonObject;
use rmcp::model::RequestId;
use serde_json::json;
use tempfile::TempDir;
use tokio::time::timeout;

use mcp_test_support::McpProcess;

const READ_TIMEOUT: Duration = Duration::from_secs(20);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_tools_exposes_cursor_session_tool() -> anyhow::Result<()> {
    let codex_home = TempDir::new()?;
    let mut process = McpProcess::new(codex_home.path()).await?;
    process.initialize().await?;

    let request_id = process.send_list_tools().await?;
    let response = timeout(
        READ_TIMEOUT,
        process.read_stream_until_response_message(RequestId::Number(request_id)),
    )
    .await??;

    let tools = response
        .result
        .get("tools")
        .and_then(serde_json::Value::as_array)
        .expect("tools/list response should contain tools");
    let cursor_session = tools
        .iter()
        .find(|tool| tool.get("name").and_then(serde_json::Value::as_str) == Some("cursor-session"))
        .expect("cursor-session tool should be listed");

    assert_eq!(Some(&json!("Cursor Session")), cursor_session.get("title"));
    assert_eq!(
        Some(&json!(["prompt"])),
        cursor_session.pointer("/inputSchema/required")
    );
    assert_eq!(
        Some(&json!("object")),
        cursor_session.pointer("/outputSchema/type")
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cursor_session_tool_call_routes_to_handler() -> anyhow::Result<()> {
    let codex_home = TempDir::new()?;
    let mut process = McpProcess::new(codex_home.path()).await?;
    process.initialize().await?;

    let request_id = process.send_tool_call("cursor-session", None).await?;
    let response = timeout(
        READ_TIMEOUT,
        process.read_stream_until_response_message(RequestId::Number(request_id)),
    )
    .await??;

    assert_eq!(
        Some(true),
        response
            .result
            .get("isError")
            .and_then(serde_json::Value::as_bool)
    );
    assert_eq!(
        Some("Missing arguments for cursor-session tool-call; the `prompt` field is required."),
        response
            .result
            .pointer("/content/0/text")
            .and_then(serde_json::Value::as_str)
    );

    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cursor_session_tool_call_uses_configured_workspace_by_default() -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = TempDir::new()?;
    let codex_home = temp_dir.path().join("codex-home");
    let cursor_home = temp_dir.path().join("cursor-home");
    let auth_dir = cursor_home.join(".config").join("cursor");
    let workspace = temp_dir.path().join("workspace");
    std::fs::create_dir(&codex_home)?;
    std::fs::create_dir_all(&auth_dir)?;
    std::fs::create_dir(&workspace)?;
    std::fs::write(auth_dir.join("auth.json"), "{}")?;
    std::fs::write(workspace.join("target.txt"), "workspace evidence")?;

    let fake_agent = temp_dir.path().join("fake-cursor-agent");
    std::fs::write(
        &fake_agent,
        r#"#!/bin/sh
set -eu
last=
for arg in "$@"; do
  last=$arg
done
printf 'cwd=%s\n' "$PWD"
printf 'file=%s\n' "$(cat target.txt)"
printf 'prompt=%s\n' "$last"
"#,
    )?;
    let mut permissions = std::fs::metadata(&fake_agent)?.permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&fake_agent, permissions)?;

    let mut process = McpProcess::new_with_cwd(&codex_home, &workspace).await?;
    process.initialize().await?;

    let arguments = JsonObject::from_iter([
        (
            "prompt".to_string(),
            json!("inspect target.txt and report what it contains"),
        ),
        (
            "command".to_string(),
            json!(fake_agent.to_string_lossy().to_string()),
        ),
        (
            "cursor-home".to_string(),
            json!(cursor_home.to_string_lossy().to_string()),
        ),
    ]);
    let request_id = process
        .send_tool_call("cursor-session", Some(arguments))
        .await?;
    let response = timeout(
        READ_TIMEOUT,
        process.read_stream_until_response_message(RequestId::Number(request_id)),
    )
    .await??;

    assert_eq!(
        Some(false),
        response
            .result
            .get("isError")
            .and_then(serde_json::Value::as_bool)
    );
    assert_eq!(
        Some(&json!(false)),
        response.result.pointer("/structuredContent/timedOut")
    );
    assert_eq!(
        Some(&json!(0)),
        response.result.pointer("/structuredContent/exitCode")
    );

    let content = response
        .result
        .pointer("/structuredContent/content")
        .and_then(serde_json::Value::as_str)
        .expect("structured content should include stdout");
    assert!(
        content.contains(&format!("cwd={}", workspace.display())),
        "unexpected content: {content}"
    );
    assert!(
        content.contains("workspace evidence"),
        "unexpected content: {content}"
    );
    assert!(
        content.contains("inspect target.txt"),
        "unexpected content: {content}"
    );

    Ok(())
}
