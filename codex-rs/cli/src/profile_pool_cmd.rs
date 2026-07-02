use anyhow::Context;
use clap::Args;
use clap::Parser;
use codex_config::ProfileV2Name;
use codex_config::types::AuthCredentialsStoreMode;
use codex_core::config::ConfigBuilder;
use codex_core::config::LoaderOverrides;
use codex_core::config::find_codex_home;
use codex_core::config::resolve_profile_v2_config_path;
use codex_login::AuthDotJson;
use codex_login::load_auth_dot_json;
use codex_protocol::auth::AuthMode;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;
use toml::Value as TomlValue;

#[cfg(test)]
#[path = "profile_pool_cmd_tests.rs"]
mod tests;

/// [experimental] Manage a portable pool of Codex account profiles.
#[derive(Debug, Parser)]
pub(crate) struct ProfilePoolCli {
    #[command(subcommand)]
    subcommand: ProfilePoolSubcommand,
}

#[derive(Debug, clap::Subcommand)]
enum ProfilePoolSubcommand {
    /// Show configured pool entries and local health.
    Status(ProfilePoolCheckArgs),

    /// Validate configured pool entries and fail if any entry is unusable.
    Test(ProfilePoolCheckArgs),
}

#[derive(Debug, Args)]
struct ProfilePoolCheckArgs {
    /// Path to the pool TOML file. Defaults to $CODEX_HOME/pool.toml.
    #[arg(long = "pool", value_name = "FILE")]
    pool: Option<PathBuf>,

    /// Print machine-readable JSON.
    #[arg(long = "json", default_value_t = false)]
    json: bool,

    /// Error out when an entry config.toml contains fields that are not recognized by this version of Codex.
    #[arg(long = "strict-config", default_value_t = false)]
    strict_config: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PoolToml {
    #[serde(default)]
    default_strategy: Option<String>,
    #[serde(default)]
    fallback_cooldown_seconds: Option<u64>,
    profiles: Vec<PoolProfileToml>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PoolProfileToml {
    id: String,
    codex_home: PathBuf,
    #[serde(default)]
    config_profile: Option<String>,
    #[serde(default)]
    priority: i64,
    #[serde(default)]
    cooldown_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PoolProfile {
    id: String,
    codex_home: PathBuf,
    config_profile: Option<ProfileV2Name>,
    priority: i64,
    cooldown_seconds: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PoolStatusOutput {
    pool: String,
    default_strategy: Option<String>,
    fallback_cooldown_seconds: Option<u64>,
    profiles: Vec<ProfileStatus>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProfileStatus {
    id: String,
    codex_home: String,
    config_profile: Option<String>,
    priority: i64,
    cooldown_seconds: Option<u64>,
    status: HealthStatus,
    config: CheckResult,
    auth: CheckResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum HealthStatus {
    Ok,
    Fail,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CheckResult {
    status: HealthStatus,
    message: String,
}

pub(crate) async fn run(
    cli: ProfilePoolCli,
    cli_overrides: Vec<(String, TomlValue)>,
) -> anyhow::Result<()> {
    match cli.subcommand {
        ProfilePoolSubcommand::Status(args) => {
            let output = collect_pool_status(args.pool, args.strict_config, cli_overrides).await?;
            print_pool_status(&output, args.json)?;
        }
        ProfilePoolSubcommand::Test(args) => {
            let output = collect_pool_status(args.pool, args.strict_config, cli_overrides).await?;
            print_pool_status(&output, args.json)?;
            if output
                .profiles
                .iter()
                .any(|profile| profile.status == HealthStatus::Fail)
            {
                anyhow::bail!("one or more pool profiles failed validation");
            }
        }
    }

    Ok(())
}

async fn collect_pool_status(
    pool_path: Option<PathBuf>,
    strict_config: bool,
    cli_overrides: Vec<(String, TomlValue)>,
) -> anyhow::Result<PoolStatusOutput> {
    let pool_path = resolve_pool_path(pool_path)?;
    let pool = load_pool_config(&pool_path)?;
    let statuses = check_profiles(&pool.profiles, strict_config, cli_overrides).await;

    Ok(PoolStatusOutput {
        pool: pool_path.display().to_string(),
        default_strategy: pool.default_strategy,
        fallback_cooldown_seconds: pool.fallback_cooldown_seconds,
        profiles: statuses,
    })
}

fn resolve_pool_path(pool_path: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    match pool_path {
        Some(path) => Ok(path),
        None => Ok(find_codex_home()?.join("pool.toml").to_path_buf()),
    }
}

fn load_pool_config(path: &Path) -> anyhow::Result<PoolConfig> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read pool config {}", path.display()))?;
    let raw: PoolToml = toml::from_str(&contents)
        .with_context(|| format!("failed to parse pool config {}", path.display()))?;
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    PoolConfig::from_toml(raw, base_dir)
}

#[derive(Debug, PartialEq, Eq)]
struct PoolConfig {
    default_strategy: Option<String>,
    fallback_cooldown_seconds: Option<u64>,
    profiles: Vec<PoolProfile>,
}

impl PoolConfig {
    fn from_toml(raw: PoolToml, base_dir: &Path) -> anyhow::Result<Self> {
        if raw.profiles.is_empty() {
            anyhow::bail!("pool config must include at least one [[profiles]] entry");
        }

        let mut ids = HashSet::new();
        let mut profiles = Vec::with_capacity(raw.profiles.len());
        for profile in raw.profiles {
            if profile.id.trim().is_empty() {
                anyhow::bail!("pool profile id must not be empty");
            }
            if !ids.insert(profile.id.clone()) {
                anyhow::bail!("duplicate pool profile id `{}`", profile.id);
            }
            let config_profile = profile
                .config_profile
                .as_deref()
                .map(ProfileV2Name::from_str)
                .transpose()
                .with_context(|| {
                    format!("invalid config_profile for pool profile `{}`", profile.id)
                })?;
            profiles.push(PoolProfile {
                id: profile.id,
                codex_home: resolve_pool_relative_path(base_dir, profile.codex_home),
                config_profile,
                priority: profile.priority,
                cooldown_seconds: profile.cooldown_seconds,
            });
        }

        profiles.sort_by(|left, right| {
            right
                .priority
                .cmp(&left.priority)
                .then_with(|| left.id.cmp(&right.id))
        });

        Ok(Self {
            default_strategy: raw.default_strategy,
            fallback_cooldown_seconds: raw.fallback_cooldown_seconds,
            profiles,
        })
    }
}

fn resolve_pool_relative_path(base_dir: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        base_dir.join(path)
    }
}

async fn check_profiles(
    profiles: &[PoolProfile],
    strict_config: bool,
    cli_overrides: Vec<(String, TomlValue)>,
) -> Vec<ProfileStatus> {
    let mut statuses = Vec::with_capacity(profiles.len());
    for profile in profiles {
        statuses.push(check_profile(profile, strict_config, cli_overrides.clone()).await);
    }
    statuses
}

async fn check_profile(
    profile: &PoolProfile,
    strict_config: bool,
    cli_overrides: Vec<(String, TomlValue)>,
) -> ProfileStatus {
    let config_profile = profile
        .config_profile
        .as_ref()
        .map(|profile| profile.as_str().to_string());
    let (config, auth) = match load_profile_config(profile, strict_config, cli_overrides).await {
        Ok(config) => {
            let auth = check_auth(&config);
            (
                CheckResult {
                    status: HealthStatus::Ok,
                    message: "ok".to_string(),
                },
                auth,
            )
        }
        Err(err) => (
            CheckResult {
                status: HealthStatus::Fail,
                message: err.to_string(),
            },
            CheckResult {
                status: HealthStatus::Fail,
                message: "skipped because config did not load".to_string(),
            },
        ),
    };
    let status = if config.status == HealthStatus::Ok && auth.status == HealthStatus::Ok {
        HealthStatus::Ok
    } else {
        HealthStatus::Fail
    };

    ProfileStatus {
        id: profile.id.clone(),
        codex_home: profile.codex_home.display().to_string(),
        config_profile,
        priority: profile.priority,
        cooldown_seconds: profile.cooldown_seconds,
        status,
        config,
        auth,
    }
}

async fn load_profile_config(
    profile: &PoolProfile,
    strict_config: bool,
    cli_overrides: Vec<(String, TomlValue)>,
) -> anyhow::Result<codex_core::config::Config> {
    let mut loader_overrides = LoaderOverrides::default();
    if let Some(config_profile) = profile.config_profile.as_ref() {
        loader_overrides.user_config_path = Some(resolve_profile_v2_config_path(
            &profile.codex_home,
            config_profile,
        ));
        loader_overrides.user_config_profile = Some(config_profile.clone());
    }

    Ok(ConfigBuilder::default()
        .codex_home(profile.codex_home.clone())
        .cli_overrides(cli_overrides)
        .loader_overrides(loader_overrides)
        .strict_config(strict_config)
        .build()
        .await?)
}

fn check_auth(config: &codex_core::config::Config) -> CheckResult {
    match load_auth_dot_json(
        &config.codex_home,
        config.cli_auth_credentials_store_mode,
        config.auth_keyring_backend_kind(),
    ) {
        Ok(Some(auth)) => match stored_auth_mode(&auth) {
            Ok(mode) => CheckResult {
                status: HealthStatus::Ok,
                message: auth_mode_name(mode).to_string(),
            },
            Err(message) => CheckResult {
                status: HealthStatus::Fail,
                message,
            },
        },
        Ok(None) => CheckResult {
            status: HealthStatus::Fail,
            message: format!(
                "no stored credentials found in {} auth storage",
                auth_store_name(config.cli_auth_credentials_store_mode)
            ),
        },
        Err(err) => CheckResult {
            status: HealthStatus::Fail,
            message: format!("failed to load auth storage: {err}"),
        },
    }
}

fn stored_auth_mode(auth: &AuthDotJson) -> Result<AuthMode, String> {
    let mode = resolved_auth_mode(auth);
    match mode {
        AuthMode::ApiKey => {
            require_non_empty(
                auth.openai_api_key.as_deref(),
                "API key auth is missing a key",
            )?;
        }
        AuthMode::Chatgpt | AuthMode::ChatgptAuthTokens => {
            let Some(tokens) = auth.tokens.as_ref() else {
                return Err("ChatGPT auth is missing tokens".to_string());
            };
            require_non_empty(
                Some(&tokens.access_token),
                "ChatGPT auth is missing an access token",
            )?;
            require_non_empty(
                Some(&tokens.refresh_token),
                "ChatGPT auth is missing a refresh token",
            )?;
        }
        AuthMode::AgentIdentity => {
            let Some(agent_identity) = auth.agent_identity.as_ref() else {
                return Err("agent identity auth is missing auth material".to_string());
            };
            if !agent_identity.has_auth_material() {
                return Err("agent identity auth material is empty".to_string());
            }
        }
        AuthMode::PersonalAccessToken => {
            require_non_empty(
                auth.personal_access_token.as_deref(),
                "personal access token auth is missing a token",
            )?;
        }
        AuthMode::BedrockApiKey => {
            if auth.bedrock_api_key.is_none() {
                return Err("Bedrock API key auth is missing a key".to_string());
            }
        }
    }
    Ok(mode)
}

fn resolved_auth_mode(auth: &AuthDotJson) -> AuthMode {
    if let Some(mode) = auth.auth_mode {
        return mode;
    }
    if auth.personal_access_token.is_some() {
        return AuthMode::PersonalAccessToken;
    }
    if auth.bedrock_api_key.is_some() {
        return AuthMode::BedrockApiKey;
    }
    if auth.openai_api_key.is_some() {
        return AuthMode::ApiKey;
    }
    AuthMode::Chatgpt
}

fn require_non_empty(value: Option<&str>, message: &str) -> Result<(), String> {
    match value {
        Some(value) if !value.trim().is_empty() => Ok(()),
        Some(_) | None => Err(message.to_string()),
    }
}

fn auth_mode_name(mode: AuthMode) -> &'static str {
    match mode {
        AuthMode::ApiKey => "api_key",
        AuthMode::Chatgpt => "chatgpt",
        AuthMode::ChatgptAuthTokens => "chatgpt_auth_tokens",
        AuthMode::AgentIdentity => "agent_identity",
        AuthMode::PersonalAccessToken => "personal_access_token",
        AuthMode::BedrockApiKey => "bedrock_api_key",
    }
}

fn auth_store_name(mode: AuthCredentialsStoreMode) -> &'static str {
    match mode {
        AuthCredentialsStoreMode::File => "file",
        AuthCredentialsStoreMode::Keyring => "keyring",
        AuthCredentialsStoreMode::Auto => "auto",
        AuthCredentialsStoreMode::Ephemeral => "ephemeral",
    }
}

fn print_pool_status(output: &PoolStatusOutput, json: bool) -> anyhow::Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(output)?);
        return Ok(());
    }

    println!("Profile pool: {}", output.pool);
    for profile in &output.profiles {
        let status = match profile.status {
            HealthStatus::Ok => "ok",
            HealthStatus::Fail => "fail",
        };
        let config_profile = profile.config_profile.as_deref().unwrap_or("-");
        println!(
            "{} {status} priority={} home={} config_profile={} auth={}",
            profile.id, profile.priority, profile.codex_home, config_profile, profile.auth.message,
        );
        if profile.config.status == HealthStatus::Fail {
            println!("  config: {}", profile.config.message);
        }
        if profile.auth.status == HealthStatus::Fail {
            println!("  auth: {}", profile.auth.message);
        }
    }

    Ok(())
}
