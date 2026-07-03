use toml::Value as TomlValue;

pub const AVALAI_PROVIDER_ID: &str = "avalai";
pub const AVALAI_DEFAULT_MODEL: &str = "deepseek-v4-pro";
pub const AVALAI_DEFAULT_BASE_URL: &str = "https://api.avalai.ir/v1";
pub const AVALAI_API_KEY_ENV_VAR: &str = "AVALAI_API_KEY";
pub const AVALAI_ENV_KEY_INSTRUCTIONS: &str =
    "Set the AVALAI_API_KEY environment variable to your AvalAI API key.";

pub fn avalai_cli_overrides() -> Vec<(String, TomlValue)> {
    vec![
        (
            "model_provider".to_string(),
            TomlValue::String(AVALAI_PROVIDER_ID.to_string()),
        ),
        (
            "model".to_string(),
            TomlValue::String(AVALAI_DEFAULT_MODEL.to_string()),
        ),
        (
            format!("model_providers.{AVALAI_PROVIDER_ID}.name"),
            TomlValue::String("AvalAI".to_string()),
        ),
        (
            format!("model_providers.{AVALAI_PROVIDER_ID}.base_url"),
            TomlValue::String(AVALAI_DEFAULT_BASE_URL.to_string()),
        ),
        (
            format!("model_providers.{AVALAI_PROVIDER_ID}.env_key"),
            TomlValue::String(AVALAI_API_KEY_ENV_VAR.to_string()),
        ),
        (
            format!("model_providers.{AVALAI_PROVIDER_ID}.env_key_instructions"),
            TomlValue::String(AVALAI_ENV_KEY_INSTRUCTIONS.to_string()),
        ),
        (
            format!("model_providers.{AVALAI_PROVIDER_ID}.wire_api"),
            TomlValue::String("responses".to_string()),
        ),
        (
            format!("model_providers.{AVALAI_PROVIDER_ID}.requires_openai_auth"),
            TomlValue::Boolean(false),
        ),
        (
            format!("model_providers.{AVALAI_PROVIDER_ID}.supports_websockets"),
            TomlValue::Boolean(false),
        ),
    ]
}

pub fn prepend_avalai_cli_overrides(overrides: &mut Vec<(String, TomlValue)>) {
    let mut avalai_overrides = avalai_cli_overrides();
    avalai_overrides.append(overrides);
    *overrides = avalai_overrides;
}

#[cfg(test)]
#[path = "model_provider_shortcuts_tests.rs"]
mod tests;
