use super::*;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

fn write_api_auth(path: &Path, key: &str) -> anyhow::Result<()> {
    fs::write(
        path,
        format!(r#"{{"auth_mode":"apikey","OPENAI_API_KEY":"{key}"}}"#),
    )?;
    Ok(())
}

fn write_chatgpt_auth(home: &Path, account_id: &str) -> anyhow::Result<()> {
    fs::write(
        home.join("auth.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "auth_mode": "chatgpt",
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token": "eyJhbGciOiJub25lIn0.e30.c2ln",
                "access_token": "access-token",
                "refresh_token": "refresh-token",
                "account_id": account_id,
            },
        }))?,
    )?;
    Ok(())
}

fn write_limit_cache(
    root: &ProfilesRoot,
    name: &str,
    five_hour_percent: f64,
    weekly_percent: f64,
) -> anyhow::Result<()> {
    fs::create_dir_all(root.limits_dir())?;
    fs::write(
        root.limit_file(name),
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": 1,
            "account_id": name,
            "observed_at": 1,
            "status": classify_limit_status(&serde_json::json!({
                "rate_limit": {
                    "primary_window": {
                        "used_percent": five_hour_percent,
                        "window_minutes": 300,
                    },
                    "secondary_window": {
                        "used_percent": weekly_percent,
                        "window_minutes": 10080,
                    },
                },
            })),
            "payload": {
                "rate_limit": {
                    "primary_window": {
                        "used_percent": five_hour_percent,
                        "window_minutes": 300,
                    },
                    "secondary_window": {
                        "used_percent": weekly_percent,
                        "window_minutes": 10080,
                    },
                },
            },
        }))?,
    )?;
    Ok(())
}

#[test]
fn import_list_and_remove_profile_stays_under_root() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let root = ProfilesRoot::new(temp.path().join("codex-profiles"));
    let source = temp.path().join("auth.json");
    write_api_auth(&source, "sk-test")?;

    root.import_profile("main", &source)?;

    assert_eq!(root.current_name()?, "main");
    let profiles = root.list_profiles()?;
    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles[0].name, "main");
    assert_eq!(profiles[0].home, root.profile_home("main"));
    assert_eq!(
        profiles[0].auth.as_ref().map(|auth| auth.message.as_str()),
        Some("api_key")
    );
    assert!(root.profile_home("main").join("auth.json").is_file());

    root.remove_profile("main")?;

    assert!(!root.profile_home("main").exists());
    assert!(!root.current_file().exists());
    Ok(())
}

#[test]
fn project_home_copies_auth_and_records_profile_and_root() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let root = ProfilesRoot::new(temp.path().join("codex-profiles"));
    let source = temp.path().join("auth.json");
    write_api_auth(&source, "sk-project")?;
    root.import_profile("main", &source)?;

    let project_root = temp.path().join("workspace");
    fs::create_dir(&project_root)?;
    let project_home = root.ensure_project_home("workspace-123", &project_root, "main")?;

    assert_eq!(
        fs::read_to_string(project_home.join("auth.json"))?,
        fs::read_to_string(source)?
    );
    assert_eq!(
        fs::read_to_string(project_home.join(".codex-profile-account"))?,
        "main\n"
    );
    assert_eq!(
        fs::read_to_string(project_home.join(".codex-profile-project-root"))?,
        format!("{}\n", project_root.display())
    );
    Ok(())
}

#[test]
fn resolve_managed_session_home_finds_project_session_by_id() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let root_dir = temp.path().join("codex-profiles");
    let root = ProfilesRoot::new(root_dir.clone());
    let source = temp.path().join("auth.json");
    write_api_auth(&source, "sk-project")?;
    root.import_profile("main", &source)?;

    let project_root = temp.path().join("workspace");
    fs::create_dir(&project_root)?;
    let project_home = root.ensure_project_home("workspace-123", &project_root, "main")?;
    let session_id = "123e4567-e89b-12d3-a456-426614174000";
    let session_dir = project_home.join("sessions/2026/07/04");
    fs::create_dir_all(&session_dir)?;
    fs::write(
        session_dir.join(format!("rollout-2026-07-04T00-00-00-{session_id}.jsonl")),
        format!(r#"{{"type":"session_meta","payload":{{"session_id":"{session_id}"}}}}"#),
    )?;

    let owner = resolve_managed_session_home(Some(root_dir), session_id)?.expect("session owner");

    assert_eq!(owner.codex_home, project_home);
    assert_eq!(
        owner.kind,
        ManagedSessionHomeKind::Project {
            id: "workspace-123".to_string(),
            project_root: Some(project_root),
        }
    );
    Ok(())
}

#[test]
fn resolve_managed_session_home_ignores_uuid_mentions_in_other_transcripts() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let root_dir = temp.path().join("codex-profiles");
    let root = ProfilesRoot::new(root_dir.clone());
    let source = temp.path().join("auth.json");
    write_api_auth(&source, "sk-project")?;
    root.import_profile("main", &source)?;

    let session_id = "123e4567-e89b-12d3-a456-426614174000";
    let owning_root = temp.path().join("owning-workspace");
    let noisy_root = temp.path().join("noisy-workspace");
    fs::create_dir(&owning_root)?;
    fs::create_dir(&noisy_root)?;
    let owning_home = root.ensure_project_home("owning-123", &owning_root, "main")?;
    let noisy_home = root.ensure_project_home("noisy-123", &noisy_root, "main")?;

    let owning_session_dir = owning_home.join("sessions/2026/07/04");
    fs::create_dir_all(&owning_session_dir)?;
    fs::write(
        owning_session_dir.join(format!("rollout-2026-07-04T00-00-00-{session_id}.jsonl")),
        format!(r#"{{"type":"session_meta","payload":{{"session_id":"{session_id}"}}}}"#),
    )?;

    let noisy_session_dir = noisy_home.join("sessions/2026/07/05");
    fs::create_dir_all(&noisy_session_dir)?;
    fs::write(
        noisy_session_dir
            .join("rollout-2026-07-05T00-00-00-223e4567-e89b-12d3-a456-426614174000.jsonl"),
        format!(r#"{{"type":"event_msg","payload":{{"message":"codex resume {session_id}"}}}}"#),
    )?;

    let owner = resolve_managed_session_home(Some(root_dir), session_id)?.expect("session owner");

    assert_eq!(owner.codex_home, owning_home);
    assert_eq!(
        owner.kind,
        ManagedSessionHomeKind::Project {
            id: "owning-123".to_string(),
            project_root: Some(owning_root),
        }
    );
    Ok(())
}

#[test]
fn best_profile_launch_prepares_project_home_and_failover_candidates() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let root_dir = temp.path().join("codex-profiles");
    let root = ProfilesRoot::new(root_dir.clone());
    let backup_source = temp.path().join("backup-auth.json");
    let main_source = temp.path().join("main-auth.json");
    write_api_auth(&backup_source, "sk-backup")?;
    write_api_auth(&main_source, "sk-main")?;
    root.import_profile("backup", &backup_source)?;
    root.import_profile("main", &main_source)?;
    let project_root = temp.path().join("workspace");
    fs::create_dir(&project_root)?;

    let launch = prepare_best_profile_launch(
        Some(root_dir),
        Some(project_root.as_path()),
        /*project_id*/ None,
        /*refresh_limits*/ false,
    )?;

    assert_eq!(launch.active_profile, "backup");
    assert!(launch.project_id.starts_with("workspace-"));
    assert_eq!(
        fs::read_to_string(launch.codex_home.join("auth.json"))?,
        fs::read_to_string(&backup_source)?
    );
    assert_eq!(launch.failover.active_profile, "backup");
    assert_eq!(
        launch
            .failover
            .candidates
            .into_iter()
            .map(|candidate| candidate.name)
            .collect::<Vec<_>>(),
        vec!["backup".to_string(), "main".to_string()]
    );
    Ok(())
}

#[test]
fn limit_status_classifies_usage_payloads() {
    let ok = serde_json::json!({
        "rate_limit": {
            "primary_window": {"used_percent": 10, "window_minutes": 300},
            "secondary_window": {"used_percent": 20, "window_minutes": 10080}
        }
    });
    let near = serde_json::json!({
        "rate_limit": {
            "primary_window": {"used_percent": 91, "window_minutes": 300},
            "secondary_window": {"used_percent": 20, "window_minutes": 10080}
        }
    });
    let limited = serde_json::json!({
        "rate_limit_reached_type": {"type": "primary"},
        "rate_limit": {
            "primary_window": {"used_percent": 10, "window_minutes": 300},
            "secondary_window": {"used_percent": 20, "window_minutes": 10080}
        }
    });

    assert_eq!(classify_limit_status(&ok), LimitStatus::Ok);
    assert_eq!(classify_limit_status(&near), LimitStatus::NearLimit);
    assert_eq!(classify_limit_status(&limited), LimitStatus::Limited);
}

#[test]
fn best_profile_penalizes_weekly_usage() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let root = ProfilesRoot::new(temp.path().join("codex-profiles"));
    let weekly_hot = root.ensure_profile_home("weekly-hot")?;
    let balanced = root.ensure_profile_home("balanced")?;
    write_chatgpt_auth(&weekly_hot, "weekly-hot")?;
    write_chatgpt_auth(&balanced, "balanced")?;
    write_limit_cache(
        &root,
        "weekly-hot",
        /*five_hour_percent*/ 20.0,
        /*weekly_percent*/ 95.0,
    )?;
    write_limit_cache(
        &root, "balanced", /*five_hour_percent*/ 50.0, /*weekly_percent*/ 50.0,
    )?;

    let best = best_profile(&root, &[], /*refresh*/ false)?;

    assert_eq!(best.name, "balanced");
    assert_eq!(best.five_hour_percent, Some(50.0));
    assert_eq!(best.weekly_percent, Some(50.0));
    Ok(())
}

#[test]
fn limited_cache_skip_until_uses_window_reset_time() {
    let payload = serde_json::json!({
        "rate_limit": {
            "primary_window": {
                "used_percent": 100,
                "limit_window_seconds": 18000,
                "reset_after_seconds": 7200,
                "reset_at": 200,
            },
            "secondary_window": {
                "used_percent": 97,
                "limit_window_seconds": 604800,
                "reset_after_seconds": 300000,
                "reset_at": 500,
            },
        },
    });

    assert_eq!(
        limits::limited_cache_skip_until(LimitStatus::Limited, &payload, /*observed_at*/ 100),
        Some(200)
    );
    assert_eq!(
        limits::limited_cache_skip_until(LimitStatus::Ok, &payload, /*observed_at*/ 100),
        None
    );
}

#[test]
fn cached_limit_account_must_match_current_auth() {
    let auth = auth::AuthSummary {
        mode: codex_protocol::auth::AuthMode::Chatgpt,
        email: Some("user@example.com".to_string()),
        access_token: Some("access-token".to_string()),
        account_id: Some("current-account".to_string()),
        valid: true,
        message: "chatgpt".to_string(),
    };

    assert!(limits::cached_account_matches_auth(
        "current-account",
        &auth
    ));
    assert!(!limits::cached_account_matches_auth("old-account", &auth));
}

#[test]
fn project_ids_are_stable_and_include_slug() {
    let first = project_id_for_root(Path::new("/tmp/Agent Coordinator"));
    let second = project_id_for_root(Path::new("/tmp/Agent Coordinator"));

    assert_eq!(first, second);
    assert!(first.starts_with("agent-coordinator-"));
}

#[test]
fn default_root_matches_legacy_switcher_location() {
    assert_eq!(
        default_root(PathBuf::from("/home/alice")),
        PathBuf::from("/home/alice/.config/codex-switch")
    );
}
