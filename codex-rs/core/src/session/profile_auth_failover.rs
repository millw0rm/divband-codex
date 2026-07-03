use crate::config::ProfileAuthFailoverConfig;
use codex_login::AuthManager;
use codex_protocol::error::UsageLimitReachedError;
use codex_protocol::protocol::RateLimitWindow;
use serde_json::Value;
use serde_json::json;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone)]
struct ProfileAuthCandidate {
    name: String,
    auth_file: PathBuf,
    limit_file: Option<PathBuf>,
}

#[derive(Debug)]
struct ProfileAuthFailoverState {
    active_profile: String,
    limited_profiles: HashSet<String>,
}

#[derive(Debug)]
pub(crate) struct ProfileAuthFailover {
    codex_home: PathBuf,
    candidates: Vec<ProfileAuthCandidate>,
    state: Mutex<ProfileAuthFailoverState>,
}

impl ProfileAuthFailover {
    pub(crate) fn new(codex_home: PathBuf, config: ProfileAuthFailoverConfig) -> Option<Self> {
        if config.candidates.is_empty() {
            return None;
        }
        let candidates = config
            .candidates
            .into_iter()
            .map(|candidate| ProfileAuthCandidate {
                name: candidate.name,
                auth_file: candidate.auth_file,
                limit_file: candidate.limit_file,
            })
            .collect();
        Some(Self {
            codex_home,
            candidates,
            state: Mutex::new(ProfileAuthFailoverState {
                active_profile: config.active_profile,
                limited_profiles: HashSet::new(),
            }),
        })
    }

    pub(crate) async fn switch_after_usage_limit(
        &self,
        auth_manager: &AuthManager,
        error: &UsageLimitReachedError,
    ) -> anyhow::Result<Option<String>> {
        let active = self.active_candidate()?;
        let next = self.next_candidate_after_marking_active_limited()?;
        if let Some(active) = active
            && let Some(limit_file) = active.limit_file.as_ref()
            && let Err(err) = write_limited_cache(limit_file, &active.auth_file, error)
        {
            tracing::warn!(
                error = %err,
                profile = active.name,
                "failed to write profile usage-limit cache"
            );
        }
        let Some(next) = next else {
            return Ok(None);
        };
        self.switch_to_candidate(auth_manager, next).await
    }

    pub(crate) async fn switch_to_next_profile(
        &self,
        auth_manager: &AuthManager,
    ) -> anyhow::Result<Option<String>> {
        let Some(next) = self.next_candidate_after_marking_active_limited()? else {
            return Ok(None);
        };
        self.switch_to_candidate(auth_manager, next).await
    }

    async fn switch_to_candidate(
        &self,
        auth_manager: &AuthManager,
        next: ProfileAuthCandidate,
    ) -> anyhow::Result<Option<String>> {
        copy_auth_file(&next.auth_file, &self.codex_home.join("auth.json"))?;
        auth_manager.reload().await;

        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("profile auth failover state lock poisoned"))?;
        state.active_profile.clone_from(&next.name);
        Ok(Some(next.name))
    }

    fn next_candidate_after_marking_active_limited(
        &self,
    ) -> anyhow::Result<Option<ProfileAuthCandidate>> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("profile auth failover state lock poisoned"))?;
        let active_profile = state.active_profile.clone();
        state.limited_profiles.insert(active_profile);

        Ok(self
            .candidates
            .iter()
            .find(|candidate| !state.limited_profiles.contains(&candidate.name))
            .cloned())
    }

    fn active_candidate(&self) -> anyhow::Result<Option<ProfileAuthCandidate>> {
        let state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("profile auth failover state lock poisoned"))?;
        Ok(self
            .candidates
            .iter()
            .find(|candidate| candidate.name == state.active_profile)
            .cloned())
    }
}

fn copy_auth_file(source: &Path, destination: &Path) -> anyhow::Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(source, destination)?;
    restrict_file(destination)
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

fn write_limited_cache(
    limit_file: &Path,
    auth_file: &Path,
    error: &UsageLimitReachedError,
) -> anyhow::Result<()> {
    let account_id = auth_account_id(auth_file)?;
    let observed_at = unix_now();
    let payload = limited_payload(error, observed_at);
    if let Some(parent) = limit_file.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        limit_file,
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "account_id": account_id,
            "observed_at": observed_at,
            "status": "limited",
            "payload": payload,
        }))?,
    )?;
    restrict_file(limit_file)
}

fn auth_account_id(auth_file: &Path) -> anyhow::Result<String> {
    let auth: Value = serde_json::from_slice(&fs::read(auth_file)?)?;
    auth.pointer("/tokens/account_id")
        .or_else(|| auth.get("account_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("missing ChatGPT account id in {}", auth_file.display()))
}

fn limited_payload(error: &UsageLimitReachedError, observed_at: u64) -> Value {
    let rate_limits = error.rate_limits.as_deref();
    let primary = rate_limits
        .and_then(|snapshot| snapshot.primary.as_ref())
        .map(|window| window_payload(window, observed_at))
        .or_else(|| {
            error
                .resets_at
                .map(|resets_at| reset_only_window(resets_at.timestamp(), observed_at))
        });
    let secondary = rate_limits
        .and_then(|snapshot| snapshot.secondary.as_ref())
        .map(|window| window_payload(window, observed_at));

    json!({
        "rate_limit": {
            "allowed": false,
            "limit_reached": true,
            "primary_window": primary,
            "secondary_window": secondary,
        },
        "rate_limit_reached_type": error.rate_limit_reached_type.map(|value| json!({ "type": value })),
    })
}

fn window_payload(window: &RateLimitWindow, observed_at: u64) -> Value {
    let reset_after_seconds = window.resets_at.and_then(|resets_at| {
        resets_at.checked_sub(i64::try_from(observed_at).unwrap_or(i64::MAX))
    });
    json!({
        "used_percent": window.used_percent,
        "window_minutes": window.window_minutes,
        "reset_after_seconds": reset_after_seconds,
        "reset_at": window.resets_at,
    })
}

fn reset_only_window(resets_at: i64, observed_at: u64) -> Value {
    let reset_after_seconds = resets_at.checked_sub(i64::try_from(observed_at).unwrap_or(i64::MAX));
    json!({
        "used_percent": 100.0,
        "reset_after_seconds": reset_after_seconds,
        "reset_at": resets_at,
    })
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
#[path = "profile_auth_failover_tests.rs"]
mod tests;
