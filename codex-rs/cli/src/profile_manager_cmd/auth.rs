use anyhow::Context;
use codex_login::AuthDotJson;
use codex_protocol::auth::AuthMode;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub(super) struct AuthSummary {
    pub(super) mode: AuthMode,
    pub(super) email: Option<String>,
    pub(super) access_token: Option<String>,
    pub(super) account_id: Option<String>,
    pub(super) valid: bool,
    pub(super) message: String,
}

pub(super) fn require_auth(codex_home: &Path) -> anyhow::Result<()> {
    if codex_home.join("auth.json").is_file() {
        Ok(())
    } else {
        anyhow::bail!(
            "Codex login completed without creating {}",
            codex_home.join("auth.json").display()
        )
    }
}

pub(super) fn read_auth_summary(codex_home: &Path) -> anyhow::Result<AuthSummary> {
    let path = codex_home.join("auth.json");
    let contents =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let auth: AuthDotJson = serde_json::from_str(&contents)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(auth_summary(&auth))
}

fn auth_summary(auth: &AuthDotJson) -> AuthSummary {
    let mode = resolved_auth_mode(auth);
    let mut valid = true;
    let message = match mode {
        AuthMode::ApiKey => match auth.openai_api_key.as_deref() {
            Some(key) if !key.trim().is_empty() => "api_key".to_string(),
            Some(_) | None => {
                valid = false;
                "missing API key".to_string()
            }
        },
        AuthMode::Chatgpt | AuthMode::ChatgptAuthTokens => {
            match auth
                .tokens
                .as_ref()
                .map(|tokens| tokens.access_token.trim())
            {
                Some(access_token) if !access_token.is_empty() => "chatgpt".to_string(),
                Some(_) | None => {
                    valid = false;
                    "missing ChatGPT access token".to_string()
                }
            }
        }
        AuthMode::AgentIdentity => {
            if auth
                .agent_identity
                .as_ref()
                .is_some_and(codex_login::auth::AgentIdentityStorage::has_auth_material)
            {
                "agent_identity".to_string()
            } else {
                valid = false;
                "missing agent identity auth material".to_string()
            }
        }
        AuthMode::PersonalAccessToken => {
            if auth
                .personal_access_token
                .as_deref()
                .is_some_and(|token| !token.trim().is_empty())
            {
                "personal_access_token".to_string()
            } else {
                valid = false;
                "missing personal access token".to_string()
            }
        }
        AuthMode::BedrockApiKey => {
            if auth.bedrock_api_key.is_some() {
                "bedrock_api_key".to_string()
            } else {
                valid = false;
                "missing Bedrock API key".to_string()
            }
        }
    };

    AuthSummary {
        mode,
        email: auth
            .tokens
            .as_ref()
            .and_then(|tokens| tokens.id_token.email.clone()),
        access_token: auth
            .tokens
            .as_ref()
            .map(|tokens| tokens.access_token.clone()),
        account_id: auth.tokens.as_ref().and_then(|tokens| {
            tokens
                .account_id
                .clone()
                .or_else(|| tokens.id_token.chatgpt_account_id.clone())
        }),
        valid,
        message,
    }
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
