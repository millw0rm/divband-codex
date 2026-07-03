use super::*;
use crate::config::ProfileAuthCandidateConfig;
use codex_config::types::AuthCredentialsStoreMode;
use codex_config::types::AuthKeyringBackendKind;
use codex_protocol::error::UsageLimitReachedError;
use codex_protocol::protocol::RateLimitSnapshot;
use codex_protocol::protocol::RateLimitWindow;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;

fn write_api_auth(path: &Path, key: &str) -> anyhow::Result<()> {
    fs::write(
        path,
        format!(r#"{{"auth_mode":"apikey","OPENAI_API_KEY":"{key}"}}"#),
    )?;
    Ok(())
}

fn usage_limit_error() -> UsageLimitReachedError {
    UsageLimitReachedError {
        plan_type: None,
        resets_at: None,
        rate_limits: None,
        promo_message: None,
        rate_limit_reached_type: None,
    }
}

#[test]
fn limited_payload_preserves_reset_deadline() {
    let payload = limited_payload(
        &UsageLimitReachedError {
            plan_type: None,
            resets_at: None,
            rate_limits: Some(Box::new(RateLimitSnapshot {
                limit_id: None,
                limit_name: None,
                primary: Some(RateLimitWindow {
                    used_percent: 100.0,
                    window_minutes: Some(300),
                    resets_at: Some(200),
                }),
                secondary: Some(RateLimitWindow {
                    used_percent: 80.0,
                    window_minutes: Some(10080),
                    resets_at: Some(500),
                }),
                credits: None,
                individual_limit: None,
                plan_type: None,
                rate_limit_reached_type: None,
            })),
            promo_message: None,
            rate_limit_reached_type: None,
        },
        /*observed_at*/ 100,
    );

    assert_eq!(
        payload,
        json!({
            "rate_limit": {
                "allowed": false,
                "limit_reached": true,
                "primary_window": {
                    "used_percent": 100.0,
                    "window_minutes": 300,
                    "reset_after_seconds": 100,
                    "reset_at": 200,
                },
                "secondary_window": {
                    "used_percent": 80.0,
                    "window_minutes": 10080,
                    "reset_after_seconds": 400,
                    "reset_at": 500,
                },
            },
            "rate_limit_reached_type": null,
        })
    );
}

#[tokio::test]
async fn switch_after_usage_limit_copies_next_auth_and_reloads_manager() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let codex_home = temp.path().join("project-home");
    let profiles = temp.path().join("profiles");
    let main = profiles.join("main");
    let backup = profiles.join("backup");
    fs::create_dir_all(&codex_home)?;
    fs::create_dir_all(&main)?;
    fs::create_dir_all(&backup)?;
    write_api_auth(&codex_home.join("auth.json"), "sk-main")?;
    write_api_auth(&main.join("auth.json"), "sk-main")?;
    write_api_auth(&backup.join("auth.json"), "sk-backup")?;
    let auth_manager = AuthManager::shared(
        codex_home.clone(),
        /*enable_codex_api_key_env*/ false,
        AuthCredentialsStoreMode::File,
        /*forced_chatgpt_workspace_id*/ None,
        /*chatgpt_base_url*/ None,
        AuthKeyringBackendKind::default(),
        /*auth_route_config*/ None,
    )
    .await;
    assert_eq!(
        auth_manager
            .auth()
            .await
            .and_then(|auth| auth.api_key().map(str::to_string)),
        Some("sk-main".to_string())
    );

    let failover = ProfileAuthFailover::new(
        codex_home.clone(),
        ProfileAuthFailoverConfig {
            active_profile: "main".to_string(),
            candidates: vec![
                ProfileAuthCandidateConfig {
                    name: "main".to_string(),
                    auth_file: main.join("auth.json"),
                    limit_file: None,
                },
                ProfileAuthCandidateConfig {
                    name: "backup".to_string(),
                    auth_file: backup.join("auth.json"),
                    limit_file: None,
                },
            ],
        },
    )
    .expect("two candidates should enable failover");

    assert_eq!(
        failover
            .switch_after_usage_limit(&auth_manager, &usage_limit_error())
            .await?,
        Some("backup".to_string())
    );
    assert_eq!(
        auth_manager
            .auth()
            .await
            .and_then(|auth| auth.api_key().map(str::to_string)),
        Some("sk-backup".to_string())
    );
    assert_eq!(
        fs::read_to_string(codex_home.join("auth.json"))?,
        fs::read_to_string(backup.join("auth.json"))?
    );
    assert_eq!(
        failover
            .switch_after_usage_limit(&auth_manager, &usage_limit_error())
            .await?,
        None
    );
    Ok(())
}

#[tokio::test]
async fn switch_to_next_profile_copies_next_auth_and_reloads_manager() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let codex_home = temp.path().join("project-home");
    let profiles = temp.path().join("profiles");
    let main = profiles.join("main");
    let backup = profiles.join("backup");
    fs::create_dir_all(&codex_home)?;
    fs::create_dir_all(&main)?;
    fs::create_dir_all(&backup)?;
    write_api_auth(&codex_home.join("auth.json"), "sk-main")?;
    write_api_auth(&main.join("auth.json"), "sk-main")?;
    write_api_auth(&backup.join("auth.json"), "sk-backup")?;
    let auth_manager = AuthManager::shared(
        codex_home.clone(),
        /*enable_codex_api_key_env*/ false,
        AuthCredentialsStoreMode::File,
        /*forced_chatgpt_workspace_id*/ None,
        /*chatgpt_base_url*/ None,
        AuthKeyringBackendKind::default(),
        /*auth_route_config*/ None,
    )
    .await;
    let failover = ProfileAuthFailover::new(
        codex_home.clone(),
        ProfileAuthFailoverConfig {
            active_profile: "main".to_string(),
            candidates: vec![
                ProfileAuthCandidateConfig {
                    name: "main".to_string(),
                    auth_file: main.join("auth.json"),
                    limit_file: None,
                },
                ProfileAuthCandidateConfig {
                    name: "backup".to_string(),
                    auth_file: backup.join("auth.json"),
                    limit_file: None,
                },
            ],
        },
    )
    .expect("two candidates should enable failover");

    assert_eq!(
        failover.switch_to_next_profile(&auth_manager).await?,
        Some("backup".to_string())
    );
    assert_eq!(
        auth_manager
            .auth()
            .await
            .and_then(|auth| auth.api_key().map(str::to_string)),
        Some("sk-backup".to_string())
    );
    assert_eq!(failover.switch_to_next_profile(&auth_manager).await?, None);
    Ok(())
}

#[tokio::test]
async fn single_candidate_is_configured_but_has_no_next_profile() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let codex_home = temp.path().join("project-home");
    let profiles = temp.path().join("profiles");
    let main = profiles.join("main");
    fs::create_dir_all(&codex_home)?;
    fs::create_dir_all(&main)?;
    write_api_auth(&codex_home.join("auth.json"), "sk-main")?;
    write_api_auth(&main.join("auth.json"), "sk-main")?;
    let auth_manager = AuthManager::shared(
        codex_home.clone(),
        /*enable_codex_api_key_env*/ false,
        AuthCredentialsStoreMode::File,
        /*forced_chatgpt_workspace_id*/ None,
        /*chatgpt_base_url*/ None,
        AuthKeyringBackendKind::default(),
        /*auth_route_config*/ None,
    )
    .await;
    let failover = ProfileAuthFailover::new(
        codex_home,
        ProfileAuthFailoverConfig {
            active_profile: "main".to_string(),
            candidates: vec![ProfileAuthCandidateConfig {
                name: "main".to_string(),
                auth_file: main.join("auth.json"),
                limit_file: None,
            }],
        },
    )
    .expect("one candidate should keep managed profile mode configured");

    assert_eq!(failover.switch_to_next_profile(&auth_manager).await?, None);
    Ok(())
}
