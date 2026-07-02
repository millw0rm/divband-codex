use super::*;
use codex_login::AuthDotJson;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

#[test]
fn pool_config_resolves_relative_homes_and_sorts_by_priority() -> anyhow::Result<()> {
    let raw: PoolToml = toml::from_str(
        r#"
default_strategy = "priority"
fallback_cooldown_seconds = 3600

[[profiles]]
id = "backup"
codex_home = "backup-home"
priority = 5

[[profiles]]
id = "main"
codex_home = "main-home"
config_profile = "work"
priority = 10
cooldown_seconds = 120
"#,
    )?;
    let config = PoolConfig::from_toml(raw, Path::new("/tmp/pool"))?;

    assert_eq!(
        config.profiles,
        vec![
            PoolProfile {
                id: "main".to_string(),
                codex_home: PathBuf::from("/tmp/pool/main-home"),
                config_profile: Some(ProfileV2Name::from_str("work")?),
                priority: 10,
                cooldown_seconds: Some(120),
            },
            PoolProfile {
                id: "backup".to_string(),
                codex_home: PathBuf::from("/tmp/pool/backup-home"),
                config_profile: None,
                priority: 5,
                cooldown_seconds: None,
            },
        ]
    );

    Ok(())
}

#[test]
fn pool_config_rejects_duplicate_ids() -> anyhow::Result<()> {
    let raw: PoolToml = toml::from_str(
        r#"
[[profiles]]
id = "main"
codex_home = "/tmp/main"

[[profiles]]
id = "main"
codex_home = "/tmp/other"
"#,
    )?;
    let err = PoolConfig::from_toml(raw, Path::new("/tmp"))
        .expect_err("duplicate ids should be rejected");

    assert_eq!(err.to_string(), "duplicate pool profile id `main`");
    Ok(())
}

#[test]
fn stored_auth_mode_accepts_api_key_auth() {
    let auth = AuthDotJson {
        auth_mode: Some(AuthMode::ApiKey),
        openai_api_key: Some("sk-test".to_string()),
        tokens: None,
        last_refresh: None,
        agent_identity: None,
        personal_access_token: None,
        bedrock_api_key: None,
    };

    assert_eq!(stored_auth_mode(&auth), Ok(AuthMode::ApiKey));
}

#[test]
fn stored_auth_mode_rejects_empty_api_key_auth() {
    let auth = AuthDotJson {
        auth_mode: Some(AuthMode::ApiKey),
        openai_api_key: Some(" ".to_string()),
        tokens: None,
        last_refresh: None,
        agent_identity: None,
        personal_access_token: None,
        bedrock_api_key: None,
    };

    assert_eq!(
        stored_auth_mode(&auth),
        Err("API key auth is missing a key".to_string())
    );
}

#[tokio::test]
async fn collect_pool_status_checks_file_auth() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let pool_path = temp.path().join("pool.toml");
    let codex_home = temp.path().join("home");
    std::fs::create_dir(&codex_home)?;
    std::fs::write(
        codex_home.join("auth.json"),
        r#"{"auth_mode":"apikey","OPENAI_API_KEY":"sk-test"}"#,
    )?;
    std::fs::write(
        &pool_path,
        format!(
            r#"
[[profiles]]
id = "main"
codex_home = "{}"
"#,
            codex_home.display()
        ),
    )?;

    let output = collect_pool_status(Some(pool_path), false, Vec::new()).await?;

    assert_eq!(output.profiles.len(), 1);
    assert_eq!(output.profiles[0].status, HealthStatus::Ok);
    assert_eq!(output.profiles[0].auth.message, "api_key");
    Ok(())
}
