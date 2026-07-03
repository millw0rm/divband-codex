//! Shared command-line flags used by both interactive and non-interactive Codex entry points.

use crate::SandboxModeCliArg;
use clap::Args;
use codex_protocol::config_types::ProfileV2Name;
use std::path::PathBuf;

#[derive(Args, Clone, Debug, Default)]
pub struct SharedCliOptions {
    /// Optional image(s) to attach to the initial prompt.
    #[arg(
        long = "image",
        short = 'i',
        value_name = "FILE",
        value_delimiter = ',',
        num_args = 1..
    )]
    pub images: Vec<PathBuf>,

    /// Model the agent should use.
    #[arg(long, short = 'm')]
    pub model: Option<String>,

    /// Use open-source provider.
    #[arg(long = "oss", default_value_t = false)]
    pub oss: bool,

    /// Use AvalAI's OpenAI-compatible Responses endpoint.
    #[arg(long = "avalai", default_value_t = false, conflicts_with_all = ["oss", "oss_provider"])]
    pub avalai: bool,

    /// Specify which local provider to use (lmstudio or ollama).
    /// If not specified with --oss, will use config default or show selection.
    #[arg(long = "local-provider")]
    pub oss_provider: Option<String>,

    /// Layer $CODEX_HOME/<name>.config.toml on top of the base user config.
    #[arg(long = "profile", short = 'p')]
    pub config_profile_v2: Option<ProfileV2Name>,

    /// Use a durable project-scoped Codex home for sessions, memories, and related state.
    #[arg(long = "project", value_name = "ID")]
    pub project: Option<String>,

    /// Directory whose repository/root should be bound to --project. Defaults to --cd or cwd.
    #[arg(long = "project-dir", value_name = "DIR", requires = "project")]
    pub project_dir: Option<PathBuf>,

    /// Select the sandbox policy to use when executing model-generated shell
    /// commands.
    #[arg(long = "sandbox", short = 's')]
    pub sandbox_mode: Option<SandboxModeCliArg>,

    /// Skip all confirmation prompts and execute commands without sandboxing.
    /// EXTREMELY DANGEROUS. Intended solely for running in environments that are externally sandboxed.
    #[arg(
        long = "dangerously-bypass-approvals-and-sandbox",
        alias = "yolo",
        default_value_t = false
    )]
    pub dangerously_bypass_approvals_and_sandbox: bool,

    /// Run enabled hooks without requiring persisted hook trust for this invocation.
    /// DANGEROUS. Intended only for automation that already vets hook sources.
    #[arg(long = "dangerously-bypass-hook-trust", default_value_t = false)]
    pub bypass_hook_trust: bool,

    /// Tell the agent to use the specified directory as its working root.
    #[clap(long = "cd", short = 'C', value_name = "DIR")]
    pub cwd: Option<PathBuf>,

    /// Additional directories that should be writable alongside the primary workspace.
    #[arg(long = "add-dir", value_name = "DIR", value_hint = clap::ValueHint::DirPath)]
    pub add_dir: Vec<PathBuf>,
}

impl SharedCliOptions {
    pub fn inherit_exec_root_options(&mut self, root: &Self) {
        let self_selected_sandbox_mode =
            self.sandbox_mode.is_some() || self.dangerously_bypass_approvals_and_sandbox;
        let Self {
            images,
            model,
            oss,
            avalai,
            oss_provider,
            config_profile_v2,
            project,
            project_dir,
            sandbox_mode,
            dangerously_bypass_approvals_and_sandbox,
            bypass_hook_trust,
            cwd,
            add_dir,
        } = self;
        let Self {
            images: root_images,
            model: root_model,
            oss: root_oss,
            avalai: root_avalai,
            oss_provider: root_oss_provider,
            config_profile_v2: root_config_profile_v2,
            project: root_project,
            project_dir: root_project_dir,
            sandbox_mode: root_sandbox_mode,
            dangerously_bypass_approvals_and_sandbox: root_dangerously_bypass_approvals_and_sandbox,
            bypass_hook_trust: root_bypass_hook_trust,
            cwd: root_cwd,
            add_dir: root_add_dir,
        } = root;

        if model.is_none() {
            model.clone_from(root_model);
        }
        if *root_avalai && !*oss {
            *avalai = true;
        }
        if *root_oss && !*avalai {
            *oss = true;
        }
        if oss_provider.is_none() && !*avalai {
            oss_provider.clone_from(root_oss_provider);
        }
        if config_profile_v2.is_none() {
            config_profile_v2.clone_from(root_config_profile_v2);
        }
        if project.is_none() {
            project.clone_from(root_project);
        }
        if project_dir.is_none() {
            project_dir.clone_from(root_project_dir);
        }
        if sandbox_mode.is_none() {
            *sandbox_mode = *root_sandbox_mode;
        }
        if !self_selected_sandbox_mode {
            *dangerously_bypass_approvals_and_sandbox =
                *root_dangerously_bypass_approvals_and_sandbox;
        }
        if !*bypass_hook_trust {
            *bypass_hook_trust = *root_bypass_hook_trust;
        }
        if cwd.is_none() {
            cwd.clone_from(root_cwd);
        }
        if !root_images.is_empty() {
            let mut merged_images = root_images.clone();
            merged_images.append(images);
            *images = merged_images;
        }
        if !root_add_dir.is_empty() {
            let mut merged_add_dir = root_add_dir.clone();
            merged_add_dir.append(add_dir);
            *add_dir = merged_add_dir;
        }
    }

    pub fn apply_subcommand_overrides(&mut self, subcommand: Self) {
        let subcommand_selected_sandbox_mode = subcommand.sandbox_mode.is_some()
            || subcommand.dangerously_bypass_approvals_and_sandbox;
        let Self {
            images,
            model,
            oss,
            avalai,
            oss_provider,
            config_profile_v2,
            project,
            project_dir,
            sandbox_mode,
            dangerously_bypass_approvals_and_sandbox,
            bypass_hook_trust,
            cwd,
            add_dir,
        } = subcommand;

        if let Some(model) = model {
            self.model = Some(model);
        }
        if avalai {
            self.avalai = true;
            self.oss = false;
            self.oss_provider = None;
        }
        if oss {
            self.oss = true;
            self.avalai = false;
        }
        if let Some(oss_provider) = oss_provider {
            self.oss_provider = Some(oss_provider);
        }
        if let Some(config_profile_v2) = config_profile_v2 {
            self.config_profile_v2 = Some(config_profile_v2);
        }
        if let Some(project) = project {
            self.project = Some(project);
        }
        if let Some(project_dir) = project_dir {
            self.project_dir = Some(project_dir);
        }
        if subcommand_selected_sandbox_mode {
            self.sandbox_mode = sandbox_mode;
            self.dangerously_bypass_approvals_and_sandbox =
                dangerously_bypass_approvals_and_sandbox;
        }
        if bypass_hook_trust {
            self.bypass_hook_trust = true;
        }
        if let Some(cwd) = cwd {
            self.cwd = Some(cwd);
        }
        if !images.is_empty() {
            self.images = images;
        }
        if !add_dir.is_empty() {
            self.add_dir.extend(add_dir);
        }
    }
}
