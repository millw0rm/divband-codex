use crate::profile_manager_cmd::auth::AuthSummary;
use crate::profile_manager_cmd::auth::read_auth_summary;
use crate::profile_manager_cmd::fs_utils::restrict_file;
use crate::profile_manager_cmd::root::ProfilesRoot;
use anyhow::Context;
use codex_protocol::auth::AuthMode;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

const NEAR_LIMIT_PERCENT: f64 = 90.0;
const FIVE_HOUR_WINDOW_MINUTES: i64 = 5 * 60;
const WEEKLY_WINDOW_MINUTES: i64 = 7 * 24 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum LimitStatus {
    Ok,
    NearLimit,
    Limited,
    Unknown,
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
pub(super) struct LimitReport {
    pub(super) name: String,
    pub(super) status: LimitStatus,
    pub(super) five_hour_percent: Option<f64>,
    pub(super) weekly_percent: Option<f64>,
    source: String,
    pub(super) detail: String,
}

#[derive(Debug, Clone, Copy)]
struct UsageWindow {
    used_percent: f64,
    window_minutes: Option<i64>,
    is_secondary: bool,
}

pub(super) fn limit_reports(
    root: &ProfilesRoot,
    names: &[String],
    refresh: bool,
) -> anyhow::Result<Vec<LimitReport>> {
    let names = selected_names(root, names)?;
    let mut reports = Vec::new();
    for name in names {
        reports.push(limit_report(root, &name, refresh));
    }
    Ok(reports)
}

fn selected_names(root: &ProfilesRoot, names: &[String]) -> anyhow::Result<Vec<String>> {
    if names.is_empty() {
        return Ok(root
            .list_profiles()?
            .into_iter()
            .map(|profile| profile.name)
            .collect());
    }
    for name in names {
        root.require_profile(name)?;
    }
    Ok(names.to_vec())
}

fn limit_report(root: &ProfilesRoot, name: &str, refresh: bool) -> LimitReport {
    let Ok(auth) = read_auth_summary(&root.profile_home(name)) else {
        return LimitReport {
            name: name.to_string(),
            status: LimitStatus::Unknown,
            five_hour_percent: None,
            weekly_percent: None,
            source: "local".to_string(),
            detail: "missing or invalid auth.json".to_string(),
        };
    };

    if !auth.valid {
        return LimitReport {
            name: name.to_string(),
            status: LimitStatus::Unknown,
            five_hour_percent: None,
            weekly_percent: None,
            source: "local".to_string(),
            detail: auth.message,
        };
    }

    if !matches!(auth.mode, AuthMode::Chatgpt | AuthMode::ChatgptAuthTokens) {
        return LimitReport {
            name: name.to_string(),
            status: LimitStatus::Ok,
            five_hour_percent: Some(0.0),
            weekly_percent: Some(0.0),
            source: "auth".to_string(),
            detail: format!("limits not applicable for {}", auth.message),
        };
    }

    match limit_cache_or_fetch(root, name, &auth, refresh) {
        Ok((cache, source)) => report_from_cache(name, cache, source),
        Err(err) => LimitReport {
            name: name.to_string(),
            status: LimitStatus::Unknown,
            five_hour_percent: None,
            weekly_percent: None,
            source: "remote".to_string(),
            detail: err.to_string(),
        },
    }
}

fn limit_cache_or_fetch(
    root: &ProfilesRoot,
    name: &str,
    auth: &AuthSummary,
    refresh: bool,
) -> anyhow::Result<(LimitCache, String)> {
    let cache_path = root.limit_file(name);
    if !refresh && cache_path.is_file() {
        let contents = fs::read_to_string(&cache_path)?;
        let cache: LimitCache = serde_json::from_str(&contents)?;
        if cached_account_matches_auth(&cache.account_id, auth) {
            return Ok((cache, "cache".to_string()));
        }
    }
    let cache = fetch_limits(auth)?;
    fs::create_dir_all(root.limits_dir())?;
    fs::write(&cache_path, serde_json::to_vec_pretty(&cache)?)?;
    restrict_file(&cache_path)?;
    Ok((cache, "remote".to_string()))
}

pub(super) fn cached_account_matches_auth(cached_account_id: &str, auth: &AuthSummary) -> bool {
    auth.account_id.as_deref() == Some(cached_account_id)
}

fn fetch_limits(auth: &AuthSummary) -> anyhow::Result<LimitCache> {
    let access_token = auth
        .access_token
        .as_deref()
        .context("missing ChatGPT access token")?;
    let account_id = auth
        .account_id
        .as_deref()
        .context("missing ChatGPT account id")?;
    let url = std::env::var("CODEX_PROFILES_LIMITS_URL")
        .unwrap_or_else(|_| "https://chatgpt.com/backend-api/wham/usage".to_string());
    let payload: Value = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()?
        .get(url)
        .bearer_auth(access_token)
        .header("ChatGPT-Account-ID", account_id)
        .header("User-Agent", "codex-profiles")
        .send()?
        .error_for_status()?
        .json()?;
    let status = classify_limit_status(&payload);
    Ok(LimitCache {
        version: 1,
        account_id: account_id.to_string(),
        observed_at: unix_now(),
        status,
        payload,
    })
}

fn report_from_cache(name: &str, cache: LimitCache, source: String) -> LimitReport {
    LimitReport {
        name: name.to_string(),
        status: cache.status,
        five_hour_percent: labeled_window_percent(&cache.payload, WindowLabel::FiveHour),
        weekly_percent: labeled_window_percent(&cache.payload, WindowLabel::Weekly),
        source,
        detail: format!("observed={}", cache.observed_at),
    }
}

pub(super) fn classify_limit_status(payload: &Value) -> LimitStatus {
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
        .and_then(Value::as_i64);
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

pub(super) fn best_profile(
    root: &ProfilesRoot,
    names: &[String],
    refresh: bool,
) -> anyhow::Result<LimitReport> {
    ranked_profiles(root, names, refresh)?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("no usable profile found"))
}

pub(super) fn ranked_profiles(
    root: &ProfilesRoot,
    names: &[String],
    refresh: bool,
) -> anyhow::Result<Vec<LimitReport>> {
    let mut reports = limit_reports(root, names, refresh)?;
    reports.retain(|report| matches!(report.status, LimitStatus::Ok | LimitStatus::NearLimit));
    reports.sort_by(|left, right| {
        limit_score(left)
            .total_cmp(&limit_score(right))
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(reports)
}

fn limit_score(report: &LimitReport) -> f64 {
    report
        .five_hour_percent
        .unwrap_or(100.0)
        .max(report.weekly_percent.unwrap_or(100.0))
}

pub(super) fn print_limits(reports: &[LimitReport]) {
    println!(
        "{:<24} {:<10} {:<10} {:<10} {:<8} DETAIL",
        "PROFILE", "STATUS", "5H", "WEEKLY", "SOURCE"
    );
    for report in reports {
        println!(
            "{:<24} {:<10} {:<10} {:<10} {:<8} {}",
            report.name,
            status_name(report.status),
            percent_text(report.five_hour_percent),
            percent_text(report.weekly_percent),
            report.source,
            report.detail
        );
    }
}

pub(super) fn status_name(status: LimitStatus) -> &'static str {
    match status {
        LimitStatus::Ok => "ok",
        LimitStatus::NearLimit => "near-limit",
        LimitStatus::Limited => "limited",
        LimitStatus::Unknown => "unknown",
    }
}

pub(super) fn percent_text(value: Option<f64>) -> String {
    value.map_or_else(|| "-".to_string(), |value| format!("{value:.0}%"))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
