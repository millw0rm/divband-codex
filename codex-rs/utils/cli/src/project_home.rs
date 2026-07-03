use std::fs;
use std::io;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

const PROJECTS_DIR: &str = "projects";
const HOMES_DIR: &str = "homes";
const PROJECT_ROOT_FILE: &str = ".codex-project-root";
const PROFILE_PROJECT_ROOT_FILE: &str = ".codex-profile-project-root";
const PROJECT_ID_FILE: &str = ".codex-project-id";
const PROJECT_SOURCE_HOME_FILE: &str = ".codex-project-source-home";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectHomeLaunch {
    pub codex_home: PathBuf,
    pub base_codex_home: PathBuf,
    pub project_id: String,
    pub project_root: PathBuf,
}

pub fn prepare_project_home(
    base_codex_home: &Path,
    project_id: &str,
    project_dir: Option<&Path>,
) -> io::Result<ProjectHomeLaunch> {
    validate_project_id(project_id)?;
    let project_root = resolve_project_root(project_dir)?;
    let codex_home = project_home_for_base(base_codex_home, project_id);

    fs::create_dir_all(&codex_home)?;
    restrict_dir(&codex_home)?;

    ensure_project_root_marker(&codex_home, &project_root)?;
    write_private_file(
        &codex_home.join(PROJECT_ID_FILE),
        format!("{project_id}\n").as_bytes(),
    )?;
    write_private_file(
        &codex_home.join(PROJECT_SOURCE_HOME_FILE),
        format!("{}\n", base_codex_home.display()).as_bytes(),
    )?;
    copy_auth_if_present(base_codex_home, &codex_home)?;

    Ok(ProjectHomeLaunch {
        codex_home,
        base_codex_home: base_codex_home.to_path_buf(),
        project_id: project_id.to_string(),
        project_root,
    })
}

pub fn project_home_for_base(base_codex_home: &Path, project_id: &str) -> PathBuf {
    if let Some(profile_root) = managed_profile_root(base_codex_home) {
        return profile_root.join(PROJECTS_DIR).join(project_id);
    }
    base_codex_home.join(PROJECTS_DIR).join(project_id)
}

fn managed_profile_root(base_codex_home: &Path) -> Option<&Path> {
    let homes_dir = base_codex_home.parent()?;
    if homes_dir.file_name()? != HOMES_DIR {
        return None;
    }
    homes_dir.parent()
}

fn validate_project_id(project_id: &str) -> io::Result<()> {
    let mut chars = project_id.chars();
    let Some(first) = chars.next() else {
        return Err(invalid_input("project id cannot be empty"));
    };
    if !first.is_ascii_alphanumeric() {
        return Err(invalid_input(format!(
            "invalid project id `{project_id}`: must start with an ASCII letter or digit"
        )));
    }
    if project_id.len() > 64 {
        return Err(invalid_input(format!(
            "invalid project id `{project_id}`: must be 64 bytes or fewer"
        )));
    }
    if chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-')) {
        Ok(())
    } else {
        Err(invalid_input(format!(
            "invalid project id `{project_id}`: use only ASCII letters, digits, '.', '_' or '-'"
        )))
    }
}

fn resolve_project_root(dir: Option<&Path>) -> io::Result<PathBuf> {
    let start = match dir {
        Some(dir) => dir.to_path_buf(),
        None => std::env::current_dir()?,
    };
    Ok(git_root(&start)?.unwrap_or_else(|| start.canonicalize().unwrap_or(start)))
}

fn git_root(dir: &Path) -> io::Result<Option<PathBuf>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("rev-parse")
        .arg("--show-toplevel")
        .output();
    match output {
        Ok(output) if output.status.success() => {
            let root = String::from_utf8(output.stdout)
                .map_err(|err| io::Error::new(ErrorKind::InvalidData, err))?;
            Ok(Some(PathBuf::from(root.trim())))
        }
        Ok(_) => Ok(None),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

fn ensure_project_root_marker(codex_home: &Path, project_root: &Path) -> io::Result<()> {
    let expected = project_root.to_string_lossy();
    for marker in [PROJECT_ROOT_FILE, PROFILE_PROJECT_ROOT_FILE] {
        let path = codex_home.join(marker);
        match fs::read_to_string(&path) {
            Ok(existing) if existing.trim() != expected => {
                return Err(invalid_input(format!(
                    "project id is already bound to {}; choose another id or remove {}",
                    existing.trim(),
                    codex_home.display()
                )));
            }
            Ok(_) => {}
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }
    write_private_file(
        &codex_home.join(PROJECT_ROOT_FILE),
        format!("{expected}\n").as_bytes(),
    )
}

fn copy_auth_if_present(base_codex_home: &Path, project_codex_home: &Path) -> io::Result<()> {
    let auth_source = base_codex_home.join("auth.json");
    let auth_dest = project_codex_home.join("auth.json");
    if auth_source == auth_dest {
        return Ok(());
    }
    match fs::copy(&auth_source, &auth_dest) {
        Ok(_) => restrict_file(&auth_dest),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

fn write_private_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    restrict_file(path)
}

fn restrict_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn restrict_file(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(ErrorKind::InvalidInput, message.into())
}

#[cfg(test)]
#[path = "project_home_tests.rs"]
mod tests;
