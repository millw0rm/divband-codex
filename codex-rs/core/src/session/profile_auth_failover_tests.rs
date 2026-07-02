use super::*;
use crate::config::ProfileAuthCandidateConfig;
use codex_config::types::AuthCredentialsStoreMode;
use codex_config::types::AuthKeyringBackendKind;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

fn write_api_auth(path: &Path, key: &str) -> anyhow::Result<()> {
    fs::write(
        path,
        format!(r#"{{"auth_mode":"apikey","OPENAI_API_KEY":"{key}"}}"#),
    )?;
    Ok(())
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
                },
                ProfileAuthCandidateConfig {
                    name: "backup".to_string(),
                    auth_file: backup.join("auth.json"),
                },
            ],
        },
    )
    .expect("two candidates should enable failover");

    assert_eq!(
        failover.switch_after_usage_limit(&auth_manager).await?,
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
        failover.switch_after_usage_limit(&auth_manager).await?,
        None
    );
    Ok(())
}
