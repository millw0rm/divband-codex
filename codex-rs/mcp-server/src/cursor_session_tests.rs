use std::ffi::OsString;
use std::io::Cursor;

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::*;

#[test]
fn cursor_session_tool_schema_is_model_visible_contract() {
    let tool = create_tool_for_cursor_session_tool_call_param();
    let tool_json = serde_json::to_value(&tool).expect("tool serializes");

    assert_eq!(
        "cursor-session",
        tool_json
            .get("name")
            .and_then(serde_json::Value::as_str)
            .expect("name")
    );
    assert_eq!(
        Some(&serde_json::json!(["prompt"])),
        tool_json.pointer("/inputSchema/required")
    );
    assert_eq!(
        Some(&serde_json::json!(false)),
        tool_json.pointer("/inputSchema/additionalProperties")
    );
    assert_eq!(
        Some(&serde_json::json!("object")),
        tool_json.pointer("/outputSchema/type")
    );
}

#[test]
fn prepare_cursor_session_command_uses_login_home_without_api_key() -> anyhow::Result<()> {
    let root = TempDir::new()?;
    let cursor_home = root.path().join("cursor-home");
    let auth_dir = cursor_home.join(".config").join("cursor");
    std::fs::create_dir_all(&auth_dir)?;
    std::fs::write(auth_dir.join("auth.json"), "{}")?;
    let cwd = root.path().join("workspace");
    std::fs::create_dir(&cwd)?;

    let prepared = prepare_cursor_session_command(
        CursorSessionToolCallParam {
            prompt: "inspect the code".to_string(),
            cwd: Some(cwd.to_string_lossy().to_string()),
            command: Some("cursor-agent --extra".to_string()),
            cursor_home: Some(cursor_home.to_string_lossy().to_string()),
            mode: Some("ask".to_string()),
            model: Some("auto".to_string()),
            timeout_seconds: Some(5),
            output_max_bytes: Some(64),
        },
        root.path(),
    )?;

    assert_eq!("cursor-agent", prepared.program);
    assert_eq!(
        vec![
            "--extra",
            "-p",
            "--trust",
            "--mode",
            "ask",
            "--model",
            "auto",
            "--output-format",
            "text",
            "inspect the code",
        ],
        prepared.args
    );
    assert_eq!(cwd, prepared.cwd);
    assert_eq!(Duration::from_secs(5), prepared.timeout);
    assert_eq!(64, prepared.output_max_bytes);
    assert_eq!(
        Some(&cursor_home.as_os_str().to_os_string()),
        prepared.env.get(&OsString::from("HOME"))
    );
    assert_eq!(
        Some(&cursor_home.join(".config").into_os_string()),
        prepared.env.get(&OsString::from("XDG_CONFIG_HOME"))
    );
    assert_eq!(
        Some(&cursor_home.join(".cache").into_os_string()),
        prepared.env.get(&OsString::from("XDG_CACHE_HOME"))
    );
    assert_eq!(
        Some(&cursor_home.join(".npm").into_os_string()),
        prepared.env.get(&OsString::from("NPM_CONFIG_CACHE"))
    );
    assert!(!prepared.env.contains_key(&OsString::from("CURSOR_API_KEY")));
    assert!(!prepared.env.contains_key(&OsString::from("APP_SECRET_KEY")));

    Ok(())
}

#[test]
fn prepare_cursor_session_command_defaults_to_configured_workspace() -> anyhow::Result<()> {
    let root = TempDir::new()?;
    let cursor_home = root.path().join("cursor-home");
    let auth_dir = cursor_home.join(".config").join("cursor");
    std::fs::create_dir_all(&auth_dir)?;
    std::fs::write(auth_dir.join("auth.json"), "{}")?;
    let default_cwd = root.path().join("configured-workspace");
    std::fs::create_dir(&default_cwd)?;

    let prepared = prepare_cursor_session_command(
        CursorSessionToolCallParam {
            prompt: "inspect the code".to_string(),
            cwd: None,
            command: Some("cursor-agent".to_string()),
            cursor_home: Some(cursor_home.to_string_lossy().to_string()),
            mode: None,
            model: None,
            timeout_seconds: None,
            output_max_bytes: None,
        },
        &default_cwd,
    )?;

    assert_eq!(default_cwd, prepared.cwd);

    Ok(())
}

#[test]
fn prepare_cursor_session_command_requires_mounted_auth_file() {
    let root = TempDir::new().expect("tempdir");

    let err = prepare_cursor_session_command(
        CursorSessionToolCallParam {
            prompt: "inspect the code".to_string(),
            cwd: None,
            command: Some("cursor-agent".to_string()),
            cursor_home: Some(root.path().to_string_lossy().to_string()),
            mode: None,
            model: None,
            timeout_seconds: None,
            output_max_bytes: None,
        },
        root.path(),
    )
    .expect_err("missing auth profile should fail");

    assert!(
        err.to_string()
            .contains("cursor-session auth file is not available"),
        "unexpected error: {err}"
    );
}

#[test]
fn prepare_cursor_session_command_caps_output_limit() -> anyhow::Result<()> {
    let root = TempDir::new()?;
    let cursor_home = root.path().join("cursor-home");
    let auth_dir = cursor_home.join(".config").join("cursor");
    std::fs::create_dir_all(&auth_dir)?;
    std::fs::write(auth_dir.join("auth.json"), "{}")?;

    let prepared = prepare_cursor_session_command(
        CursorSessionToolCallParam {
            prompt: "inspect the code".to_string(),
            cwd: None,
            command: Some("cursor-agent".to_string()),
            cursor_home: Some(cursor_home.to_string_lossy().to_string()),
            mode: None,
            model: None,
            timeout_seconds: None,
            output_max_bytes: Some(MAX_OUTPUT_MAX_BYTES + 1),
        },
        root.path(),
    )?;

    assert_eq!(MAX_OUTPUT_MAX_BYTES, prepared.output_max_bytes);

    Ok(())
}

#[test]
fn read_limited_caps_retained_output_and_drains_reader() -> anyhow::Result<()> {
    let output = read_limited(Cursor::new(b"abcdef"), 3)?;

    assert_eq!(
        LimitedOutput {
            bytes: b"abc".to_vec(),
            truncated: true,
        },
        output
    );

    Ok(())
}

#[test]
fn process_output_preserves_structured_metadata() {
    let result = CursorSessionProcessOutput {
        stdout: LimitedOutput {
            bytes: b"cursor answer".to_vec(),
            truncated: false,
        },
        stderr: LimitedOutput {
            bytes: Vec::new(),
            truncated: false,
        },
        exit_code: Some(0),
        timed_out: false,
    }
    .into_call_tool_result();

    assert_eq!(Some(false), result.is_error);
    assert_eq!(
        Some(serde_json::json!({
            "content": "cursor answer",
            "stderr": "",
            "exitCode": 0,
            "timedOut": false,
            "stdoutTruncated": false,
            "stderrTruncated": false,
        })),
        result.structured_content
    );
}
