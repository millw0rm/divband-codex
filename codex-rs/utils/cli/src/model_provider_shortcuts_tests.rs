use pretty_assertions::assert_eq;
use toml::Value as TomlValue;

use super::*;

#[test]
fn avalai_overrides_define_openai_compatible_provider() {
    let overrides = avalai_cli_overrides();

    assert_eq!(
        overrides,
        vec![
            (
                "model_provider".to_string(),
                TomlValue::String("avalai".to_string())
            ),
            (
                "model".to_string(),
                TomlValue::String("deepseek-v4-pro".to_string())
            ),
            (
                "model_providers.avalai.name".to_string(),
                TomlValue::String("AvalAI".to_string())
            ),
            (
                "model_providers.avalai.base_url".to_string(),
                TomlValue::String("https://api.avalai.ir/v1".to_string())
            ),
            (
                "model_providers.avalai.env_key".to_string(),
                TomlValue::String("AVALAI_API_KEY".to_string())
            ),
            (
                "model_providers.avalai.env_key_instructions".to_string(),
                TomlValue::String(
                    "Set the AVALAI_API_KEY environment variable to your AvalAI API key."
                        .to_string()
                )
            ),
            (
                "model_providers.avalai.wire_api".to_string(),
                TomlValue::String("responses".to_string())
            ),
            (
                "model_providers.avalai.requires_openai_auth".to_string(),
                TomlValue::Boolean(false)
            ),
            (
                "model_providers.avalai.supports_websockets".to_string(),
                TomlValue::Boolean(false)
            ),
        ]
    );
}

#[test]
fn prepend_avalai_overrides_keeps_user_overrides_last() {
    let mut overrides = vec![
        (
            "model_providers.avalai.base_url".to_string(),
            TomlValue::String("https://avalai.example.test/v1".to_string()),
        ),
        (
            "model".to_string(),
            TomlValue::String("user-selected-model".to_string()),
        ),
    ];

    prepend_avalai_cli_overrides(&mut overrides);

    assert_eq!(
        overrides.last(),
        Some(&(
            "model".to_string(),
            TomlValue::String("user-selected-model".to_string())
        ))
    );
}
