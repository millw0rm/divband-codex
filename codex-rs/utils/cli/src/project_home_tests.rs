use super::*;
use pretty_assertions::assert_eq;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

#[test]
fn prepare_project_home_creates_home_and_copies_auth() -> io::Result<()> {
    let temp = TempTree::new("creates-home")?;
    let base_home = temp.path().join("codex-home");
    let workspace = temp.path().join("workspace");
    fs::create_dir_all(&base_home)?;
    fs::create_dir_all(&workspace)?;
    fs::write(base_home.join("auth.json"), "{\"OPENAI_API_KEY\":\"test\"}")?;

    let launch = prepare_project_home(&base_home, "demo", Some(&workspace))?;

    assert_eq!(launch.codex_home, base_home.join("projects").join("demo"));
    assert_eq!(launch.base_codex_home, base_home);
    assert_eq!(launch.project_id, "demo");
    assert_eq!(launch.project_root, workspace.canonicalize()?);
    assert_eq!(
        fs::read_to_string(launch.codex_home.join("auth.json"))?,
        "{\"OPENAI_API_KEY\":\"test\"}"
    );
    assert_eq!(
        fs::read_to_string(launch.codex_home.join(PROJECT_ROOT_FILE))?,
        format!("{}\n", workspace.canonicalize()?.display())
    );

    Ok(())
}

#[test]
fn prepare_project_home_uses_managed_profiles_project_dir() -> io::Result<()> {
    let temp = TempTree::new("managed-profile")?;
    let profiles_root = temp.path().join("codex-switch");
    let base_home = profiles_root.join("homes").join("account-a");
    let workspace = temp.path().join("workspace");
    fs::create_dir_all(&base_home)?;
    fs::create_dir_all(&workspace)?;

    let launch = prepare_project_home(&base_home, "demo", Some(&workspace))?;

    assert_eq!(
        launch.codex_home,
        profiles_root.join("projects").join("demo")
    );
    assert_eq!(
        fs::read_to_string(launch.codex_home.join(PROJECT_SOURCE_HOME_FILE))?,
        format!("{}\n", base_home.display())
    );

    Ok(())
}

#[test]
fn prepare_project_home_rejects_project_id_reuse_for_different_root() -> io::Result<()> {
    let temp = TempTree::new("root-mismatch")?;
    let base_home = temp.path().join("codex-home");
    let workspace_a = temp.path().join("workspace-a");
    let workspace_b = temp.path().join("workspace-b");
    fs::create_dir_all(&base_home)?;
    fs::create_dir_all(&workspace_a)?;
    fs::create_dir_all(&workspace_b)?;

    prepare_project_home(&base_home, "demo", Some(&workspace_a))?;
    let err = prepare_project_home(&base_home, "demo", Some(&workspace_b))
        .expect_err("project id should stay bound to its first root");

    assert_eq!(err.kind(), ErrorKind::InvalidInput);
    assert!(err.to_string().contains("project id is already bound"));

    Ok(())
}

#[test]
fn prepare_project_home_rejects_invalid_project_id() -> io::Result<()> {
    let temp = TempTree::new("invalid-id")?;
    let err = prepare_project_home(temp.path(), "../bad", None)
        .expect_err("path-like project ids should be rejected");

    assert_eq!(err.kind(), ErrorKind::InvalidInput);

    Ok(())
}

struct TempTree {
    path: PathBuf,
}

impl TempTree {
    fn new(name: &str) -> io::Result<Self> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "codex-project-home-{name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
