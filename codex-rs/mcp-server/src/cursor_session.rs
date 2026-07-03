//! Cursor Agent login-profile MCP tool.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use rmcp::model::CallToolResult;
use rmcp::model::Content;
use rmcp::model::JsonObject;
use rmcp::model::Tool;
use schemars::JsonSchema;
use schemars::r#gen::SchemaSettings;
use serde::Deserialize;
use serde::Serialize;
use serde_json::json;

use crate::codex_tool_config::create_tool_input_schema;

const TOOL_NAME: &str = "cursor-session";
const DEFAULT_CURSOR_AGENT_COMMAND: &str = "cursor-agent";
const DEFAULT_CURSOR_SESSION_HOME: &str = "/cursor-home";
const DEFAULT_CURSOR_SESSION_MODE: &str = "ask";
const DEFAULT_CURSOR_SESSION_MODEL: &str = "auto";
const DEFAULT_TIMEOUT_SECONDS: u64 = 900;
const DEFAULT_OUTPUT_MAX_BYTES: usize = 16_000;
const MAX_OUTPUT_MAX_BYTES: usize = 16_000;
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Client-supplied configuration for the `cursor-session` tool-call.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct CursorSessionToolCallParam {
    /// The task or question to send to Cursor Agent.
    pub prompt: String,

    /// Working directory for Cursor Agent. Defaults to the Codex MCP server's
    /// resolved working directory. If relative, it is resolved by the child process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,

    /// Cursor Agent command to execute. Defaults to CURSOR_SESSION_AGENT_COMMAND or cursor-agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,

    /// Cursor login-profile home directory. Defaults to CURSOR_SESSION_HOME or /cursor-home.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_home: Option<String>,

    /// Cursor mode. Defaults to CURSOR_SESSION_MODE or ask.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,

    /// Cursor model. Defaults to CURSOR_SESSION_MODEL or auto.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Maximum runtime in seconds. Defaults to CURSOR_SESSION_TIMEOUT_SECONDS or 900.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,

    /// Maximum stdout/stderr bytes retained from the child process, capped at 16000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_max_bytes: Option<usize>,
}

/// Builds a `Tool` definition for the `cursor-session` tool-call.
pub(crate) fn create_tool_for_cursor_session_tool_call_param() -> Tool {
    let schema = SchemaSettings::draft2019_09()
        .with(|s| {
            s.inline_subschemas = true;
            s.option_add_null_type = false;
        })
        .into_generator()
        .into_root_schema_for::<CursorSessionToolCallParam>();

    let input_schema =
        create_tool_input_schema(schema, "Cursor session tool schema should serialize");

    Tool::new(
        TOOL_NAME,
        "Ask Cursor Agent using a mounted login profile. Use this for bounded research or analysis; Codex remains responsible for planning, edits, and coordination.",
        input_schema,
    )
    .with_title("Cursor Session")
    .with_raw_output_schema(cursor_session_tool_output_schema())
}

fn cursor_session_tool_output_schema() -> Arc<JsonObject> {
    let schema = json!({
        "type": "object",
        "properties": {
            "content": { "type": "string" },
            "stderr": { "type": "string" },
            "exitCode": { "type": ["integer", "null"] },
            "timedOut": { "type": "boolean" },
            "stdoutTruncated": { "type": "boolean" },
            "stderrTruncated": { "type": "boolean" }
        },
        "required": [
            "content",
            "stderr",
            "exitCode",
            "timedOut",
            "stdoutTruncated",
            "stderrTruncated"
        ],
    });
    match schema {
        serde_json::Value::Object(map) => Arc::new(map),
        _ => unreachable!("json literal must be an object"),
    }
}

pub(crate) async fn handle_cursor_session_tool_call(
    arguments: Option<JsonObject>,
    default_cwd: PathBuf,
) -> CallToolResult {
    let arguments = arguments.map(serde_json::Value::Object);
    let params = match arguments {
        Some(value) => match serde_json::from_value::<CursorSessionToolCallParam>(value) {
            Ok(params) => params,
            Err(err) => {
                return CallToolResult::error(vec![Content::text(format!(
                    "Failed to parse configuration for cursor-session tool: {err}"
                ))]);
            }
        },
        None => {
            return CallToolResult::error(vec![Content::text(
                "Missing arguments for cursor-session tool-call; the `prompt` field is required.",
            )]);
        }
    };

    match run_cursor_session_tool_call(params, default_cwd).await {
        Ok(output) => output.into_call_tool_result(),
        Err(err) => CallToolResult::error(vec![Content::text(format!(
            "Failed to run cursor-session tool: {err}"
        ))]),
    }
}

async fn run_cursor_session_tool_call(
    params: CursorSessionToolCallParam,
    default_cwd: PathBuf,
) -> Result<CursorSessionProcessOutput> {
    let command = prepare_cursor_session_command(params, &default_cwd)?;
    tokio::task::spawn_blocking(move || run_prepared_command(command))
        .await
        .map_err(|err| anyhow!("cursor-session task failed: {err}"))?
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreparedCursorSessionCommand {
    program: String,
    args: Vec<String>,
    cwd: PathBuf,
    env: HashMap<OsString, OsString>,
    timeout: Duration,
    output_max_bytes: usize,
}

fn prepare_cursor_session_command(
    params: CursorSessionToolCallParam,
    default_cwd: &Path,
) -> Result<PreparedCursorSessionCommand> {
    let CursorSessionToolCallParam {
        prompt,
        cwd,
        command,
        cursor_home,
        mode,
        model,
        timeout_seconds,
        output_max_bytes,
    } = params;

    let command = non_empty_or_env(
        command,
        "CURSOR_SESSION_AGENT_COMMAND",
        DEFAULT_CURSOR_AGENT_COMMAND,
    );
    let mut command_parts = shlex::split(&command)
        .filter(|parts| !parts.is_empty())
        .ok_or_else(|| anyhow!("CURSOR_SESSION_AGENT_COMMAND is empty or invalid"))?;
    let program = command_parts.remove(0);
    let cursor_home = PathBuf::from(non_empty_or_env(
        cursor_home,
        "CURSOR_SESSION_HOME",
        DEFAULT_CURSOR_SESSION_HOME,
    ));
    let auth_file = cursor_home.join(".config").join("cursor").join("auth.json");
    if !auth_file.is_file() {
        return Err(anyhow!(
            "cursor-session auth file is not available at {}",
            auth_file.display()
        ));
    }

    let mode = non_empty_or_env(mode, "CURSOR_SESSION_MODE", DEFAULT_CURSOR_SESSION_MODE);
    let model = non_empty_or_env(model, "CURSOR_SESSION_MODEL", DEFAULT_CURSOR_SESSION_MODEL);
    let timeout_seconds = timeout_seconds
        .or_else(|| env_parsed("CURSOR_SESSION_TIMEOUT_SECONDS"))
        .unwrap_or(DEFAULT_TIMEOUT_SECONDS);
    if timeout_seconds == 0 {
        return Err(anyhow!("timeout-seconds must be greater than zero"));
    }
    let output_max_bytes = output_max_bytes
        .or_else(|| env_parsed("CURSOR_SESSION_OUTPUT_MAX_BYTES"))
        .unwrap_or(DEFAULT_OUTPUT_MAX_BYTES);
    if output_max_bytes == 0 {
        return Err(anyhow!("output-max-bytes must be greater than zero"));
    }
    let output_max_bytes = output_max_bytes.min(MAX_OUTPUT_MAX_BYTES);

    let mut args = command_parts;
    args.extend([
        "-p".to_string(),
        "--trust".to_string(),
        "--mode".to_string(),
        mode,
        "--model".to_string(),
        model,
        "--output-format".to_string(),
        "text".to_string(),
        prompt,
    ]);

    Ok(PreparedCursorSessionCommand {
        program,
        args,
        cwd: cwd
            .map(PathBuf::from)
            .unwrap_or_else(|| default_cwd.to_path_buf()),
        env: cursor_session_child_env(&cursor_home),
        timeout: Duration::from_secs(timeout_seconds),
        output_max_bytes,
    })
}

fn non_empty_or_env(value: Option<String>, env_key: &str, default: &str) -> String {
    value
        .and_then(|value| {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        })
        .or_else(|| {
            std::env::var(env_key)
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_else(|| default.to_string())
}

fn env_parsed<T: FromStr>(key: &str) -> Option<T> {
    std::env::var(key).ok()?.parse().ok()
}

fn cursor_session_child_env(cursor_home: &std::path::Path) -> HashMap<OsString, OsString> {
    let mut env = HashMap::new();
    for key in [
        "ALL_PROXY",
        "PATH",
        "LANG",
        "LC_ALL",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "NO_PROXY",
        "all_proxy",
        "http_proxy",
        "https_proxy",
        "no_proxy",
    ] {
        if let Some(value) = std::env::var_os(key) {
            env.insert(OsString::from(key), value);
        }
    }

    env.insert(
        OsString::from("HOME"),
        cursor_home.as_os_str().to_os_string(),
    );
    env.insert(
        OsString::from("XDG_CONFIG_HOME"),
        cursor_home.join(".config").into_os_string(),
    );
    env.insert(
        OsString::from("XDG_CACHE_HOME"),
        cursor_home.join(".cache").into_os_string(),
    );
    env.insert(
        OsString::from("NPM_CONFIG_CACHE"),
        cursor_home.join(".npm").into_os_string(),
    );
    env
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CursorSessionProcessOutput {
    stdout: LimitedOutput,
    stderr: LimitedOutput,
    exit_code: Option<i32>,
    timed_out: bool,
}

impl CursorSessionProcessOutput {
    fn success(&self) -> bool {
        !self.timed_out && self.exit_code == Some(0)
    }

    fn into_call_tool_result(self) -> CallToolResult {
        let stdout = self.stdout.text();
        let stderr = self.stderr.text();
        let model_visible_text = if stdout.trim().is_empty() && !stderr.trim().is_empty() {
            stderr.clone()
        } else {
            stdout.clone()
        };
        let structured_content = json!({
            "content": stdout,
            "stderr": stderr,
            "exitCode": self.exit_code,
            "timedOut": self.timed_out,
            "stdoutTruncated": self.stdout.truncated,
            "stderrTruncated": self.stderr.truncated,
        });
        let mut result = if self.success() {
            CallToolResult::success(vec![Content::text(model_visible_text)])
        } else {
            CallToolResult::error(vec![Content::text(model_visible_text)])
        };
        result.structured_content = Some(structured_content);
        result
    }
}

fn run_prepared_command(
    prepared: PreparedCursorSessionCommand,
) -> Result<CursorSessionProcessOutput> {
    let PreparedCursorSessionCommand {
        program,
        args,
        cwd,
        env,
        timeout,
        output_max_bytes,
    } = prepared;

    let mut child = Command::new(&program)
        .args(args)
        .current_dir(&cwd)
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn cursor-session command `{program}`"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("cursor-session stdout pipe is unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("cursor-session stderr pipe is unavailable"))?;
    let stdout_reader = std::thread::spawn(move || read_limited(stdout, output_max_bytes));
    let stderr_reader = std::thread::spawn(move || read_limited(stderr, output_max_bytes));

    let started = Instant::now();
    let (status, timed_out) = loop {
        if let Some(status) = child.try_wait()? {
            break (Some(status), false);
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            break (None, true);
        }
        std::thread::sleep(PROCESS_POLL_INTERVAL);
    };

    let stdout = join_limited_reader(stdout_reader, "stdout")?;
    let stderr = join_limited_reader(stderr_reader, "stderr")?;

    Ok(CursorSessionProcessOutput {
        stdout,
        stderr,
        exit_code: status.as_ref().and_then(std::process::ExitStatus::code),
        timed_out,
    })
}

fn join_limited_reader(
    handle: std::thread::JoinHandle<std::io::Result<LimitedOutput>>,
    stream_name: &str,
) -> Result<LimitedOutput> {
    handle
        .join()
        .map_err(|_| anyhow!("cursor-session {stream_name} reader panicked"))?
        .with_context(|| format!("failed to read cursor-session {stream_name}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LimitedOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

impl LimitedOutput {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).trim().to_string()
    }
}

fn read_limited(mut reader: impl Read, max_bytes: usize) -> std::io::Result<LimitedOutput> {
    let mut bytes = Vec::with_capacity(max_bytes.min(8192));
    let mut truncated = false;
    let mut buffer = [0_u8; 8192];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let remaining = max_bytes.saturating_sub(bytes.len());
        if remaining > 0 {
            bytes.extend_from_slice(&buffer[..read.min(remaining)]);
        }
        if read > remaining {
            truncated = true;
        }
    }
    Ok(LimitedOutput { bytes, truncated })
}

#[cfg(test)]
#[path = "cursor_session_tests.rs"]
mod tests;
