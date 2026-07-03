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

#[tokio::test]
async fn refresh_managed_profile_limits_returns_empty_when_no_profiles_exist() -> anyhow::Result<()>
{
    let temp = TempDir::new()?;

    let reports = refresh_managed_profile_limits(Some(temp.path().join("codex-switch"))).await?;

    assert_eq!(reports.len(), 0);
    assert_eq!(
        format_refresh_message(&reports),
        "No managed profiles found. Add one with `codex-profiles add NAME`."
    );
    Ok(())
}

#[tokio::test]
async fn refresh_managed_profile_limits_reports_api_key_profiles_without_remote_fetch()
-> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let root = temp.path().join("codex-switch");
    let home = root.join("homes").join("main");
    fs::create_dir_all(&home)?;
    write_api_auth(&home.join("auth.json"), "sk-test")?;

    let reports = refresh_managed_profile_limits(Some(root)).await?;

    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].name, "main");
    assert_eq!(reports[0].status, LimitStatus::Ok);
    assert_eq!(reports[0].five_hour_percent, Some(0.0));
    assert_eq!(reports[0].weekly_percent, Some(0.0));
    assert_eq!(
        format_refresh_message(&reports),
        "Refreshed 1 managed profile. Best available: `main` (5h 0%, weekly 0%)."
    );
    Ok(())
}
