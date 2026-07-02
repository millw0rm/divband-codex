use anyhow::Context;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

pub(super) fn copy_template_entries(
    template_home: &Path,
    destination_home: &Path,
) -> anyhow::Result<()> {
    for entry in [
        "config.toml",
        "requirements.toml",
        "skills",
        "plugins",
        "rules",
        "memories",
    ] {
        let source = template_home.join(entry);
        let destination = destination_home.join(entry);
        if source.exists() && !destination.exists() {
            copy_recursively(&source, &destination)
                .with_context(|| format!("failed to copy template entry `{entry}`"))?;
        }
    }
    Ok(())
}

fn copy_recursively(source: &Path, destination: &Path) -> anyhow::Result<()> {
    if source.is_dir() {
        fs::create_dir_all(destination)?;
        restrict_dir(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_recursively(&entry.path(), &destination.join(entry.file_name()))?;
        }
    } else {
        copy_file_private(source, destination)?;
    }
    Ok(())
}

pub(super) fn source_auth_path(source: &Path) -> anyhow::Result<PathBuf> {
    if source.is_dir() {
        let auth = source.join("auth.json");
        if auth.is_file() {
            return Ok(auth);
        }
    }
    if source.is_file() {
        return Ok(source.to_path_buf());
    }
    anyhow::bail!("auth source not found: {}", source.display())
}

pub(super) fn copy_file_private(source: &Path, destination: &Path) -> anyhow::Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(source, destination).with_context(|| {
        format!(
            "failed to copy {} to {}",
            source.display(),
            destination.display()
        )
    })?;
    restrict_file(destination)
}

#[cfg(unix)]
pub(super) fn restrict_dir(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
pub(super) fn restrict_dir(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(unix)]
pub(super) fn restrict_file(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
pub(super) fn restrict_file(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}
