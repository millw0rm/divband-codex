use crate::profile_manager_cmd::auth::AuthSummary;
use crate::profile_manager_cmd::auth::read_auth_summary;
use crate::profile_manager_cmd::fs_utils::copy_file_private;
use crate::profile_manager_cmd::fs_utils::copy_template_entries;
use crate::profile_manager_cmd::fs_utils::restrict_dir;
use crate::profile_manager_cmd::fs_utils::restrict_file;
use crate::profile_manager_cmd::fs_utils::source_auth_path;
use anyhow::Context;
use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

const CURRENT_FILE: &str = "current";
const DEFAULT_TEMPLATE_HOME: &str = "template-home";

#[derive(Debug, Clone)]
pub(super) struct ProfilesRoot {
    pub(super) root: PathBuf,
}

#[derive(Debug, Clone)]
pub(super) struct ProfileEntry {
    pub(super) name: String,
    pub(super) home: PathBuf,
    pub(super) auth: Option<AuthSummary>,
    pub(super) sessions: usize,
}

impl ProfilesRoot {
    pub(super) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub(super) fn ensure_dirs(&self) -> anyhow::Result<()> {
        for dir in [
            self.homes_dir(),
            self.projects_dir(),
            self.handoffs_dir(),
            self.limits_dir(),
            self.runs_dir(),
            self.template_home(),
        ] {
            fs::create_dir_all(&dir)
                .with_context(|| format!("failed to create {}", dir.display()))?;
            restrict_dir(&dir)?;
        }
        Ok(())
    }

    fn homes_dir(&self) -> PathBuf {
        self.root.join("homes")
    }

    fn projects_dir(&self) -> PathBuf {
        self.root.join("projects")
    }

    fn handoffs_dir(&self) -> PathBuf {
        self.root.join("handoffs")
    }

    pub(super) fn limits_dir(&self) -> PathBuf {
        self.root.join("limits")
    }

    fn runs_dir(&self) -> PathBuf {
        self.root.join("runs")
    }

    fn template_home(&self) -> PathBuf {
        self.root.join(DEFAULT_TEMPLATE_HOME)
    }

    pub(super) fn current_file(&self) -> PathBuf {
        self.root.join(CURRENT_FILE)
    }

    pub(super) fn profile_home(&self, name: &str) -> PathBuf {
        self.homes_dir().join(name)
    }

    pub(super) fn project_home(&self, id: &str) -> PathBuf {
        self.projects_dir().join(id)
    }

    pub(super) fn limit_file(&self, name: &str) -> PathBuf {
        self.limits_dir().join(format!("{name}.json"))
    }

    fn validate_name(&self, name: &str) -> anyhow::Result<()> {
        if is_valid_name(name) {
            Ok(())
        } else {
            anyhow::bail!("invalid profile name `{name}`")
        }
    }

    pub(super) fn ensure_profile_home(&self, name: &str) -> anyhow::Result<PathBuf> {
        self.validate_name(name)?;
        self.ensure_dirs()?;
        let home = self.profile_home(name);
        fs::create_dir_all(&home)
            .with_context(|| format!("failed to create {}", home.display()))?;
        restrict_dir(&home)?;
        copy_template_entries(&self.template_home(), &home)?;
        Ok(home)
    }

    pub(super) fn require_profile(&self, name: &str) -> anyhow::Result<()> {
        self.validate_name(name)?;
        if self.profile_home(name).join("auth.json").is_file() {
            Ok(())
        } else {
            anyhow::bail!("profile `{name}` does not exist or has no auth.json")
        }
    }

    pub(super) fn import_profile(&self, name: &str, source: &Path) -> anyhow::Result<()> {
        let home = self.ensure_profile_home(name)?;
        let auth_source = source_auth_path(source)?;
        copy_file_private(&auth_source, &home.join("auth.json"))?;
        self.set_current(name)
    }

    pub(super) fn set_current(&self, name: &str) -> anyhow::Result<()> {
        self.validate_name(name)?;
        self.ensure_dirs()?;
        fs::write(self.current_file(), format!("{name}\n"))?;
        restrict_file(&self.current_file())?;
        Ok(())
    }

    pub(super) fn current_name(&self) -> anyhow::Result<String> {
        let value = fs::read_to_string(self.current_file())
            .context("no current profile selected; run `codex-profiles use NAME`")?;
        let name = value.trim();
        self.validate_name(name)?;
        self.require_profile(name)?;
        Ok(name.to_string())
    }

    pub(super) fn resolve_name(&self, name: Option<&str>) -> anyhow::Result<String> {
        match name {
            Some(name) => {
                self.validate_name(name)?;
                Ok(name.to_string())
            }
            None => self.current_name(),
        }
    }

    pub(super) fn list_profiles(&self) -> anyhow::Result<Vec<ProfileEntry>> {
        self.ensure_dirs()?;
        let mut names = HashSet::new();
        for entry in fs::read_dir(self.homes_dir())? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                let name = entry.file_name().to_string_lossy().to_string();
                if is_valid_name(&name) {
                    names.insert(name);
                }
            }
        }

        let mut profiles = Vec::new();
        for name in names {
            let home = self.profile_home(&name);
            profiles.push(ProfileEntry {
                name,
                auth: read_auth_summary(&home).ok(),
                sessions: count_session_files(&home.join("sessions"))?,
                home,
            });
        }
        profiles.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(profiles)
    }

    pub(super) fn print_profiles(&self) -> anyhow::Result<()> {
        print_profiles(&self.list_profiles()?);
        Ok(())
    }

    pub(super) fn remove_profile(&self, name: &str) -> anyhow::Result<()> {
        self.validate_name(name)?;
        let home = self.profile_home(name);
        if home.exists() {
            fs::remove_dir_all(&home)
                .with_context(|| format!("failed to remove {}", home.display()))?;
        }
        let limit_file = self.limit_file(name);
        if limit_file.exists() {
            fs::remove_file(limit_file)?;
        }
        if fs::read_to_string(self.current_file())
            .ok()
            .is_some_and(|current| current.trim() == name)
        {
            match fs::remove_file(self.current_file()) {
                Ok(()) => {}
                Err(err) if err.kind() == ErrorKind::NotFound => {}
                Err(err) => return Err(err.into()),
            }
        }
        Ok(())
    }

    pub(super) fn ensure_project_home(
        &self,
        id: &str,
        project_root: &Path,
        profile_name: &str,
    ) -> anyhow::Result<PathBuf> {
        self.validate_name(id)?;
        self.require_profile(profile_name)?;
        self.ensure_dirs()?;
        let home = self.project_home(id);
        fs::create_dir_all(&home)
            .with_context(|| format!("failed to create {}", home.display()))?;
        restrict_dir(&home)?;
        copy_template_entries(&self.template_home(), &home)?;
        copy_file_private(
            &self.profile_home(profile_name).join("auth.json"),
            &home.join("auth.json"),
        )?;
        fs::write(
            home.join(".codex-profile-account"),
            format!("{profile_name}\n"),
        )?;
        fs::write(
            home.join(".codex-profile-project-root"),
            format!("{}\n", project_root.display()),
        )?;
        Ok(home)
    }
}

pub(super) fn resolve_root(root: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    if let Some(root) = root {
        return Ok(root);
    }
    if let Some(root) = std::env::var_os("CODEX_PROFILES_DIR") {
        return Ok(PathBuf::from(root));
    }
    let home = home_dir().context("could not resolve home directory")?;
    Ok(default_root(home))
}

pub(super) fn default_root(home: PathBuf) -> PathBuf {
    home.join(".config").join("codex-switch")
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}

pub(super) fn resolve_codex_bin(configured: Option<PathBuf>) -> PathBuf {
    if let Some(path) = configured {
        return path;
    }
    if let Ok(path) = std::env::var("CODEX_PROFILES_CODEX_BIN") {
        return PathBuf::from(path);
    }
    if let Ok(current_exe) = std::env::current_exe()
        && let Some(dir) = current_exe.parent()
    {
        let sibling = dir.join(exe_name("codex"));
        if sibling.is_file() {
            return sibling;
        }
    }
    PathBuf::from("codex")
}

fn exe_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

pub(super) fn is_valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-')) && name.len() <= 64
}

fn count_session_files(path: &Path) -> anyhow::Result<usize> {
    if !path.exists() {
        return Ok(0);
    }
    let mut count = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "jsonl") {
                count += 1;
            }
        }
    }
    Ok(count)
}

fn print_profiles(profiles: &[ProfileEntry]) {
    println!(
        "{:<24} {:<24} {:<28} {:<8} CODEX_HOME",
        "PROFILE", "AUTH", "EMAIL", "SESSIONS"
    );
    for profile in profiles {
        let auth = profile.auth.as_ref().map_or("missing".to_string(), |auth| {
            if auth.valid {
                auth.message.clone()
            } else {
                format!("invalid: {}", auth.message)
            }
        });
        let email = profile
            .auth
            .as_ref()
            .and_then(|auth| auth.email.as_deref())
            .unwrap_or("-");
        println!(
            "{:<24} {:<24} {:<28} {:<8} {}",
            profile.name,
            auth,
            email,
            profile.sessions,
            profile.home.display()
        );
    }
}

pub(super) fn run_codex_login(
    codex_bin: &Path,
    codex_home: &Path,
    args: &[OsString],
) -> anyhow::Result<()> {
    fs::create_dir_all(codex_home)?;
    restrict_dir(codex_home)?;
    let status = Command::new(codex_bin)
        .arg("login")
        .args(args)
        .env("CODEX_HOME", codex_home)
        .status()
        .with_context(|| format!("failed to run {}", codex_bin.display()))?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("codex login exited with {status}")
    }
}

pub(super) fn run_codex(
    codex_bin: &Path,
    codex_home: &Path,
    args: &[OsString],
) -> anyhow::Result<()> {
    fs::create_dir_all(codex_home)?;
    restrict_dir(codex_home)?;
    let status = Command::new(codex_bin)
        .args(args)
        .env("CODEX_HOME", codex_home)
        .status()
        .with_context(|| format!("failed to run {}", codex_bin.display()))?;
    std::process::exit(status.code().unwrap_or(1));
}

pub(super) fn shell_quote(path: &Path) -> String {
    let value = path.to_string_lossy();
    format!("'{}'", value.replace('\'', "'\\''"))
}
