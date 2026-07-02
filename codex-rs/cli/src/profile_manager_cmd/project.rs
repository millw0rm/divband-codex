use crate::profile_manager_cmd::root::is_valid_name;
use std::hash::Hasher;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

pub(super) fn resolve_project(
    dir: Option<&Path>,
    explicit_id: Option<&str>,
) -> anyhow::Result<(String, PathBuf)> {
    let start = match dir {
        Some(dir) => dir.to_path_buf(),
        None => std::env::current_dir()?,
    };
    let root = git_root(&start)?.unwrap_or_else(|| start.canonicalize().unwrap_or(start));
    let id = match explicit_id {
        Some(id) => {
            if !is_valid_name(id) {
                anyhow::bail!("invalid project id `{id}`");
            }
            id.to_string()
        }
        None => project_id_for_root(&root),
    };
    Ok((id, root))
}

fn git_root(dir: &Path) -> anyhow::Result<Option<PathBuf>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("rev-parse")
        .arg("--show-toplevel")
        .output();
    match output {
        Ok(output) if output.status.success() => {
            let root = String::from_utf8(output.stdout)?.trim().to_string();
            Ok(Some(PathBuf::from(root)))
        }
        Ok(_) => Ok(None),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

pub(super) fn project_id_for_root(root: &Path) -> String {
    let path = root.to_string_lossy();
    let base = root
        .file_name()
        .and_then(|name| name.to_str())
        .map(slugify)
        .filter(|slug| !slug.is_empty())
        .unwrap_or_else(|| "project".to_string());
    format!(
        "{base}-{:012x}",
        stable_hash(path.as_bytes()) & 0xffffffffffff
    )
}

fn slugify(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hasher = Fnv1a64::default();
    hasher.write(bytes);
    hasher.finish()
}

#[derive(Default)]
struct Fnv1a64(u64);

impl Hasher for Fnv1a64 {
    fn finish(&self) -> u64 {
        if self.0 == 0 {
            0xcbf29ce484222325
        } else {
            self.0
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        let mut value = self.finish();
        for byte in bytes {
            value ^= u64::from(*byte);
            value = value.wrapping_mul(0x100000001b3);
        }
        self.0 = value;
    }
}
