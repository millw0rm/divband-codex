#[path = "profile_manager_cmd/auth.rs"]
mod auth;
#[path = "profile_manager_cmd/fs_utils.rs"]
mod fs_utils;
#[path = "profile_manager_cmd/limits.rs"]
mod limits;
#[path = "profile_manager_cmd/project.rs"]
mod project;
#[path = "profile_manager_cmd/root.rs"]
mod root;

use auth::require_auth;
use clap::Args;
use clap::Parser;
use codex_core::config::ProfileAuthCandidateConfig;
use codex_core::config::ProfileAuthFailoverConfig;
use limits::best_profile;
use limits::limit_reports;
use limits::percent_text;
use limits::print_limits;
use limits::ranked_profiles;
use limits::status_name;
use project::resolve_project;
use root::ProfilesRoot;
use root::resolve_codex_bin;
use root::resolve_root;
use root::run_codex;
use root::run_codex_login;
use root::shell_quote;
use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;

#[cfg(test)]
use limits::LimitStatus;
#[cfg(test)]
use limits::classify_limit_status;
#[cfg(test)]
use project::project_id_for_root;
#[cfg(test)]
use root::default_root;
#[cfg(test)]
use std::fs;

#[cfg(test)]
#[path = "profile_manager_cmd_tests.rs"]
mod tests;

/// Manage self-contained Codex profiles and project homes.
#[derive(Debug, Parser)]
#[command(
    name = "codex-profiles",
    version,
    about = "Manage self-contained Codex profile homes"
)]
pub struct ProfilesCli {
    #[command(flatten)]
    args: ProfilesArgs,
}

/// Arguments for managing self-contained Codex profile homes.
#[derive(Debug, Args)]
pub struct ProfilesArgs {
    /// Root directory for all managed profile state.
    #[arg(long, env = "CODEX_PROFILES_DIR", value_name = "DIR")]
    root: Option<PathBuf>,

    /// Codex executable to launch for login/run commands.
    #[arg(
        long = "codex-bin",
        env = "CODEX_PROFILES_CODEX_BIN",
        value_name = "BIN"
    )]
    codex_bin: Option<PathBuf>,

    #[command(subcommand)]
    command: ProfilesCommand,
}

#[derive(Debug, clap::Subcommand)]
enum ProfilesCommand {
    /// Create the profile root directories.
    Init,

    /// Create a profile home, optionally running `codex login`.
    Add(AddArgs),

    /// Import auth.json from a file or another CODEX_HOME.
    Import(ImportArgs),

    /// Run `codex login` in a managed profile home.
    Login(LoginArgs),

    /// Select the default profile.
    Use(ProfileNameArg),

    /// Print the selected profile.
    Current,

    /// List managed profiles.
    List,

    /// Remove a profile home and cached limits.
    Remove(ProfileNameArg),

    /// Print a profile CODEX_HOME.
    Path(OptionalProfileNameArg),

    /// Print shell exports for a profile CODEX_HOME.
    Env(OptionalProfileNameArg),

    /// Print the stable project CODEX_HOME for a directory.
    Project(ProjectArgs),

    /// Run Codex with a managed CODEX_HOME.
    Run(RunArgs),

    /// Show cached or refreshed usage limits.
    Limits(LimitsArgs),

    /// Print the best currently usable existing profile.
    Best(ProfilesFilterArgs),
}

#[derive(Debug, Args)]
struct AddArgs {
    name: String,

    /// Only create the home; do not start login.
    #[arg(long = "no-login", default_value_t = false)]
    no_login: bool,
}

#[derive(Debug, Args)]
struct ImportArgs {
    name: String,
    auth_file_or_codex_home: PathBuf,
}

#[derive(Debug, Args)]
struct LoginArgs {
    name: Option<String>,

    /// Arguments passed after `--` are forwarded to `codex login`.
    #[arg(last = true)]
    login_args: Vec<OsString>,
}

#[derive(Debug, Args)]
struct ProfileNameArg {
    name: String,
}

#[derive(Debug, Args)]
struct OptionalProfileNameArg {
    name: Option<String>,
}

#[derive(Debug, Args)]
struct ProjectArgs {
    directory: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct RunArgs {
    /// Select the best usable existing profile from cached/refreshed limits.
    #[arg(long, default_value_t = false)]
    best: bool,

    /// Refresh ChatGPT usage before selecting an existing profile with --best.
    #[arg(long = "refresh-limits", default_value_t = false)]
    refresh_limits: bool,

    /// Use a stable per-project CODEX_HOME while copying selected auth into it.
    #[arg(long, default_value_t = false)]
    project: bool,

    /// Override the generated project id.
    #[arg(long = "project-id", value_name = "ID")]
    project_id: Option<String>,

    /// Directory used for project id/root resolution.
    #[arg(long = "project-dir", value_name = "DIR")]
    project_dir: Option<PathBuf>,

    /// Existing profile name. Omit with --best to auto-select.
    name: Option<String>,

    /// Arguments passed after `--` are forwarded to Codex.
    #[arg(last = true)]
    codex_args: Vec<OsString>,
}

#[derive(Debug, Args)]
struct LimitsArgs {
    /// Refresh ChatGPT usage from the backend even when a cache exists.
    #[arg(long, default_value_t = false)]
    refresh: bool,

    names: Vec<String>,
}

#[derive(Debug, Args)]
struct ProfilesFilterArgs {
    /// Refresh ChatGPT usage from the backend even when a cache exists.
    #[arg(long, default_value_t = false)]
    refresh: bool,

    names: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct BestProfileLaunch {
    pub codex_home: PathBuf,
    pub active_profile: String,
    pub project_id: String,
    pub failover: ProfileAuthFailoverConfig,
}

pub fn prepare_best_profile_launch(
    root_dir: Option<PathBuf>,
    project_dir: Option<&Path>,
    project_id: Option<&str>,
    refresh_limits: bool,
) -> anyhow::Result<BestProfileLaunch> {
    let root = ProfilesRoot::new(resolve_root(root_dir)?);
    let ranked = ranked_profiles(&root, &[], refresh_limits)?;
    let Some(active) = ranked.first() else {
        anyhow::bail!("no usable profile found");
    };
    let active_profile = active.name.clone();

    let (project_id, project_root) = resolve_project(project_dir, project_id)?;
    let codex_home = root.ensure_project_home(&project_id, &project_root, &active_profile)?;
    let candidates = ranked
        .into_iter()
        .map(|profile| ProfileAuthCandidateConfig {
            auth_file: root.profile_home(&profile.name).join("auth.json"),
            limit_file: Some(root.limit_file(&profile.name)),
            name: profile.name,
        })
        .collect();

    Ok(BestProfileLaunch {
        codex_home,
        active_profile: active_profile.clone(),
        project_id,
        failover: ProfileAuthFailoverConfig {
            active_profile,
            candidates,
        },
    })
}

impl ProfilesCli {
    pub fn run_from_args() -> anyhow::Result<()> {
        Self::parse().run()
    }

    fn run(self) -> anyhow::Result<()> {
        self.args.run()
    }
}

impl ProfilesArgs {
    pub fn run(self) -> anyhow::Result<()> {
        let root = ProfilesRoot::new(resolve_root(self.root)?);
        let codex_bin = resolve_codex_bin(self.codex_bin);
        match self.command {
            ProfilesCommand::Init => {
                root.ensure_dirs()?;
                println!("{}", root.root.display());
            }
            ProfilesCommand::Add(args) => {
                root.ensure_profile_home(&args.name)?;
                if args.no_login {
                    root.set_current(&args.name)?;
                    println!("created {}", root.profile_home(&args.name).display());
                } else {
                    run_codex_login(&codex_bin, &root.profile_home(&args.name), &[])?;
                    require_auth(&root.profile_home(&args.name))?;
                    root.set_current(&args.name)?;
                    println!("added {}", args.name);
                }
            }
            ProfilesCommand::Import(args) => {
                root.import_profile(&args.name, &args.auth_file_or_codex_home)?;
                println!("imported {}", args.name);
            }
            ProfilesCommand::Login(args) => {
                let name = root.resolve_name(args.name.as_deref())?;
                root.ensure_profile_home(&name)?;
                run_codex_login(&codex_bin, &root.profile_home(&name), &args.login_args)?;
                require_auth(&root.profile_home(&name))?;
                root.set_current(&name)?;
                println!("logged in {name}");
            }
            ProfilesCommand::Use(args) => {
                root.require_profile(&args.name)?;
                root.set_current(&args.name)?;
                println!("selected {}", args.name);
            }
            ProfilesCommand::Current => {
                let name = root.current_name()?;
                println!("{}\t{}", name, root.profile_home(&name).display());
            }
            ProfilesCommand::List => root.print_profiles()?,
            ProfilesCommand::Remove(args) => {
                root.remove_profile(&args.name)?;
                println!("removed {}", args.name);
            }
            ProfilesCommand::Path(args) => {
                let name = root.resolve_name(args.name.as_deref())?;
                root.require_profile(&name)?;
                println!("{}", root.profile_home(&name).display());
            }
            ProfilesCommand::Env(args) => {
                let name = root.resolve_name(args.name.as_deref())?;
                root.require_profile(&name)?;
                println!(
                    "export CODEX_HOME={}",
                    shell_quote(&root.profile_home(&name))
                );
            }
            ProfilesCommand::Project(args) => {
                let (id, project_root) = resolve_project(args.directory.as_deref(), None)?;
                let home = root.project_home(&id);
                println!("id\t{id}");
                println!("root\t{}", project_root.display());
                println!("CODEX_HOME\t{}", home.display());
            }
            ProfilesCommand::Run(args) => {
                let name = if args.best {
                    best_profile(&root, &[], args.refresh_limits)?.name
                } else {
                    root.resolve_name(args.name.as_deref())?
                };
                root.require_profile(&name)?;

                let codex_home = if args.project {
                    let dir = args.project_dir.as_deref();
                    let (id, project_root) = resolve_project(dir, args.project_id.as_deref())?;
                    let home = root.ensure_project_home(&id, &project_root, &name)?;
                    eprintln!(
                        "Using project {id} with profile {name}; CODEX_HOME={}",
                        home.display()
                    );
                    home
                } else {
                    let home = root.profile_home(&name);
                    eprintln!("Using profile {name}; CODEX_HOME={}", home.display());
                    home
                };
                run_codex(&codex_bin, &codex_home, &args.codex_args)?;
            }
            ProfilesCommand::Limits(args) => {
                let reports = limit_reports(&root, &args.names, args.refresh)?;
                print_limits(&reports);
            }
            ProfilesCommand::Best(args) => {
                let best = best_profile(&root, &args.names, args.refresh)?;
                println!(
                    "best={} status={} 5h={} weekly={} {}",
                    best.name,
                    status_name(best.status),
                    percent_text(best.five_hour_percent),
                    percent_text(best.weekly_percent),
                    best.detail
                );
            }
        }
        Ok(())
    }
}
