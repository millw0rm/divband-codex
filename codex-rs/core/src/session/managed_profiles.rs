use anyhow::Context;
use codex_login::AuthDotJson;
use codex_protocol::auth::AuthMode;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

const NEAR_LIMIT_PERCENT: f64 = 90.0;
const FIVE_HOUR_WINDOW_MINUTES: i64 = 5 * 60;
const WEEKLY_WINDOW_MINUTES: i64 = 7 * 24 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum LimitStatus {
    Ok,
    NearLimit,
    Limited,
    Unknown,
}

#[derive(Debug, Clone)]
pub(crate) struct ManagedProfileLimitReport {
    pub(crate) name: String,
    status: LimitStatus,
    five_hour_percent: Option<f64>,
    weekly_percent: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LimitCache {
    version: u32,
    account_id: String,
    observed_at: u64,
    status: LimitStatus,
    payload: Value,
}

#[derive(Debug, Clone)]
struct AuthSummary {
    mode: AuthMode,
    access_token: Option<String>,
    account_id: Option<String>,
    valid: bool,
}

#[derive(Debug, Clone, Copy)]
struct UsageWindow {
    used_percent: f64,
    window_minutes: Option<i64>,
    is_secondary: bool,
}

pub(crate) async fn refresh_managed_profile_limits(
    root_dir: Option<PathBuf>,
) -> anyhow::Result<Vec<ManagedProfileLimitReport>> {
    let root = resolve_root(root_dir)?;
    let names = list_profiles(&root)?;
    let mut reports = Vec::with_capacity(names.len());
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()?;
    for name in names {
        reports.push(refresh_profile_limit(&client, &root, &name).await);
    }
    Ok(reports)
}

pub(crate) fn format_refresh_message(reports: &[ManagedProfileLimitReport]) -> String {
    if reports.is_empty() {
        return "No managed profiles found. Add one with `codex-profiles add NAME`.".to_string();
    }

    let usable = reports
        .iter()
        .filter(|report| matches!(report.status, LimitStatus::Ok | LimitStatus::NearLimit))
        .min_by(|left, right| {
            limit_score(left)
                .total_cmp(&limit_score(right))
                .then_with(|| left.name.cmp(&right.name))
        });
    let limited_count = reports
        .iter()
        .filter(|report| report.status == LimitStatus::Limited)
        .count();
    let unknown_count = reports
        .iter()
        .filter(|report| report.status == LimitStatus::Unknown)
        .count();

    let availability = usable.map_or_else(
        || "No usable managed profile is currently available.".to_string(),
        |report| {
            format!(
                "Best available: `{}` (5h {}, weekly {}).",
                report.name,
                percent_text(report.five_hour_percent),
                percent_text(report.weekly_percent)
            )
        },
    );
    let mut suffixes = Vec::new();
    if limited_count > 0 {
        suffixes.push(format!("{limited_count} limited"));
    }
    if unknown_count > 0 {
        suffixes.push(format!("{unknown_count} unknown"));
    }
    if suffixes.is_empty() {
        format!(
            "Refreshed {} managed profile{}. {availability}",
            reports.len(),
            plural_suffix(reports.len())
        )
    } else {
        format!(
            "Refreshed {} managed profile{}. {availability} ({})",
            reports.len(),
            plural_suffix(reports.len()),
            suffixes.join(", ")
        )
    }
}

async fn refresh_profile_limit(
    client: &reqwest::Client,
    root: &Path,
    name: &str,
) -> ManagedProfileLimitReport {
    let home = profile_home(root, name);
    let Ok(auth) = read_auth_summary(&home) else {
        return ManagedProfileLimitReport {
            name: name.to_string(),
            status: LimitStatus::Unknown,
            five_hour_percent: None,
            weekly_percent: None,
        };
    };

    if !auth.valid {
        return ManagedProfileLimitReport {
            name: name.to_string(),
            status: LimitStatus::Unknown,
            five_hour_percent: None,
            weekly_percent: None,
        };
    }

    if !matches!(auth.mode, AuthMode::Chatgpt | AuthMode::ChatgptAuthTokens) {
        return ManagedProfileLimitReport {
            name: name.to_string(),
            status: LimitStatus::Ok,
            five_hour_percent: Some(0.0),
            weekly_percent: Some(0.0),
        };
    }

    match fetch_limits(client, &auth).await {
        Ok(cache) => {
            let _ = write_limit_cache(root, name, &cache);
            report_from_cache(name, cache)
        }
        Err(_) => ManagedProfileLimitReport {
            name: name.to_string(),
            status: LimitStatus::Unknown,
            five_hour_percent: None,
            weekly_percent: None,
        },
    }
}

fn resolve_root(root_dir: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    if let Some(root) = root_dir {
        return Ok(root);
    }
    if let Some(root) = env::var_os("CODEX_PROFILES_DIR") {
        return Ok(PathBuf::from(root));
    }
    let home = dirs::home_dir().context("could not resolve home directory")?;
    Ok(home.join(".config").join("codex-switch"))
}

fn list_profiles(root: &Path) -> anyhow::Result<Vec<String>> {
    let homes = homes_dir(root);
    if !homes.is_dir() {
        return Ok(Vec::new());
    }

    let mut names = HashSet::new();
    for entry in fs::read_dir(homes)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            if is_valid_name(&name) {
                names.insert(name);
            }
        }
    }

    let mut names = names.into_iter().collect::<Vec<_>>();
    names.sort();
    Ok(names)
}

fn read_auth_summary(codex_home: &Path) -> anyhow::Result<AuthSummary> {
    let path = codex_home.join("auth.json");
    let contents =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let auth: AuthDotJson = serde_json::from_str(&contents)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(auth_summary(&auth))
}

fn auth_summary(auth: &AuthDotJson) -> AuthSummary {
    let mode = resolved_auth_mode(auth);
    let valid = match mode {
        AuthMode::ApiKey => match auth.openai_api_key.as_deref() {
            Some(key) if !key.trim().is_empty() => true,
            Some(_) | None => false,
        },
        AuthMode::Chatgpt | AuthMode::ChatgptAuthTokens => {
            match auth
                .tokens
                .as_ref()
                .map(|tokens| tokens.access_token.trim())
            {
                Some(access_token) if !access_token.is_empty() => true,
                Some(_) | None => false,
            }
        }
        AuthMode::AgentIdentity => auth
            .agent_identity
            .as_ref()
            .is_some_and(codex_login::auth::AgentIdentityStorage::has_auth_material),
        AuthMode::PersonalAccessToken => auth
            .personal_access_token
            .as_deref()
            .is_some_and(|token| !token.trim().is_empty()),
        AuthMode::BedrockApiKey => auth.bedrock_api_key.is_some(),
    };

    AuthSummary {
        mode,
        access_token: auth
            .tokens
            .as_ref()
            .map(|tokens| tokens.access_token.clone()),
        account_id: auth.tokens.as_ref().and_then(|tokens| {
            tokens
                .account_id
                .clone()
                .or_else(|| tokens.id_token.chatgpt_account_id.clone())
        }),
        valid,
    }
}

fn resolved_auth_mode(auth: &AuthDotJson) -> AuthMode {
    if let Some(mode) = auth.auth_mode {
        return mode;
    }
    if auth.personal_access_token.is_some() {
        return AuthMode::PersonalAccessToken;
    }
    if auth.bedrock_api_key.is_some() {
        return AuthMode::BedrockApiKey;
    }
    if auth.openai_api_key.is_some() {
        return AuthMode::ApiKey;
    }
    AuthMode::Chatgpt
}

async fn fetch_limits(client: &reqwest::Client, auth: &AuthSummary) -> anyhow::Result<LimitCache> {
    let access_token = auth
        .access_token
        .as_deref()
        .context("missing ChatGPT access token")?;
    let account_id = auth
        .account_id
        .as_deref()
        .context("missing ChatGPT account id")?;
    let url = env::var("CODEX_PROFILES_LIMITS_URL")
        .unwrap_or_else(|_| "https://chatgpt.com/backend-api/wham/usage".to_string());
    let payload: Value = client
        .get(url)
        .bearer_auth(access_token)
        .header("ChatGPT-Account-ID", account_id)
        .header("User-Agent", "codex-profiles")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let status = classify_limit_status(&payload);
    Ok(LimitCache {
        version: 1,
        account_id: account_id.to_string(),
        observed_at: unix_now(),
        status,
        payload,
    })
}

fn write_limit_cache(root: &Path, name: &str, cache: &LimitCache) -> anyhow::Result<()> {
    let limits = limits_dir(root);
    fs::create_dir_all(&limits)?;
    let path = limits.join(format!("{name}.json"));
    fs::write(&path, serde_json::to_vec_pretty(cache)?)?;
    restrict_file(&path)
}

#[cfg(unix)]
fn restrict_file(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_file(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

fn report_from_cache(name: &str, cache: LimitCache) -> ManagedProfileLimitReport {
    ManagedProfileLimitReport {
        name: name.to_string(),
        status: cache.status,
        five_hour_percent: labeled_window_percent(&cache.payload, WindowLabel::FiveHour),
        weekly_percent: labeled_window_percent(&cache.payload, WindowLabel::Weekly),
    }
}

fn classify_limit_status(payload: &Value) -> LimitStatus {
    if payload
        .pointer("/rate_limit_reached_type/type")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty())
    {
        return LimitStatus::Limited;
    }
    let max_percent = usage_windows(payload)
        .into_iter()
        .map(|window| window.used_percent)
        .fold(0.0, f64::max);
    if max_percent >= 100.0 {
        LimitStatus::Limited
    } else if max_percent >= NEAR_LIMIT_PERCENT {
        LimitStatus::NearLimit
    } else {
        LimitStatus::Ok
    }
}

#[derive(Clone, Copy)]
enum WindowLabel {
    FiveHour,
    Weekly,
}

fn labeled_window_percent(payload: &Value, label: WindowLabel) -> Option<f64> {
    let windows = usage_windows(payload);
    windows
        .iter()
        .find(|window| matches_window_label(window.window_minutes, label))
        .or_else(|| fallback_window(&windows, label))
        .map(|window| window.used_percent)
}

fn usage_windows(payload: &Value) -> Vec<UsageWindow> {
    let mut windows = Vec::new();
    if let Some(window) = usage_window(payload, "primary_window", /*is_secondary*/ false) {
        windows.push(window);
    }
    if let Some(window) = usage_window(payload, "secondary_window", /*is_secondary*/ true) {
        windows.push(window);
    }
    windows
}

fn usage_window(payload: &Value, name: &str, is_secondary: bool) -> Option<UsageWindow> {
    let used_percent = payload
        .pointer(&format!("/rate_limit/{name}/used_percent"))
        .and_then(Value::as_f64)?;
    let window_minutes = payload
        .pointer(&format!("/rate_limit/{name}/window_minutes"))
        .and_then(Value::as_i64)
        .or_else(|| {
            payload
                .pointer(&format!("/rate_limit/{name}/limit_window_seconds"))
                .and_then(Value::as_i64)
                .map(|seconds| seconds / 60)
        });
    Some(UsageWindow {
        used_percent,
        window_minutes,
        is_secondary,
    })
}

fn matches_window_label(window_minutes: Option<i64>, label: WindowLabel) -> bool {
    let Some(window_minutes) = window_minutes else {
        return false;
    };
    match label {
        WindowLabel::FiveHour => is_approximate_window(window_minutes, FIVE_HOUR_WINDOW_MINUTES),
        WindowLabel::Weekly => is_approximate_window(window_minutes, WEEKLY_WINDOW_MINUTES),
    }
}

fn fallback_window(windows: &[UsageWindow], label: WindowLabel) -> Option<&UsageWindow> {
    windows.iter().find(|window| match label {
        WindowLabel::FiveHour => !window.is_secondary,
        WindowLabel::Weekly => window.is_secondary,
    })
}

fn is_approximate_window(minutes: i64, expected_minutes: i64) -> bool {
    let minutes = minutes.max(0) as f64;
    let expected_minutes = expected_minutes as f64;
    minutes >= expected_minutes * 0.95 && minutes <= expected_minutes * 1.05
}

fn limit_score(report: &ManagedProfileLimitReport) -> f64 {
    report
        .five_hour_percent
        .unwrap_or(100.0)
        .max(report.weekly_percent.unwrap_or(100.0))
}

fn percent_text(value: Option<f64>) -> String {
    value.map_or_else(|| "-".to_string(), |value| format!("{value:.0}%"))
}

fn plural_suffix(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

fn homes_dir(root: &Path) -> PathBuf {
    root.join("homes")
}

fn profile_home(root: &Path, name: &str) -> PathBuf {
    homes_dir(root).join(name)
}

fn limits_dir(root: &Path) -> PathBuf {
    root.join("limits")
}

fn is_valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-')) && name.len() <= 64
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
#[path = "managed_profiles_tests.rs"]
mod tests;
