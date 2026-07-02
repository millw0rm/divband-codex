use crate::config::ProfileAuthFailoverConfig;
use codex_login::AuthManager;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Debug, Clone)]
struct ProfileAuthCandidate {
    name: String,
    auth_file: PathBuf,
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
        if config.candidates.len() < 2 {
            return None;
        }
        let candidates = config
            .candidates
            .into_iter()
            .map(|candidate| ProfileAuthCandidate {
                name: candidate.name,
                auth_file: candidate.auth_file,
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
    ) -> anyhow::Result<Option<String>> {
        let Some(next) = self.next_candidate_after_marking_active_limited()? else {
            return Ok(None);
        };

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

#[cfg(test)]
#[path = "profile_auth_failover_tests.rs"]
mod tests;
