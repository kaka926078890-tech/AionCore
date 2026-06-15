use std::collections::HashMap;

use aionui_runtime::Builder;
use serde_json::Value;

use crate::config::FindeskConfig;
use crate::finclaw::binary::resolve_finclaw_binary;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinclawLlmConfigSlice {
    pub provider: String,
    pub model: String,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
}

/// Map FinDesk provider settings to finclaw `llm.provider` values.
pub fn infer_finclaw_llm_provider(
    platform: &str,
    base_url: &str,
    model_id: &str,
    model_protocols: Option<&str>,
) -> String {
    if platform == "anthropic" {
        return "anthropic".into();
    }

    if platform == "new-api"
        && let Some(protocols_json) = model_protocols
        && let Ok(map) = serde_json::from_str::<HashMap<String, Value>>(protocols_json)
        && map.get(model_id).and_then(Value::as_str) == Some("anthropic")
    {
        return "anthropic".into();
    }

    let base = base_url.to_ascii_lowercase();
    if base.contains("deepseek.com") {
        return "deepseek".into();
    }
    if base.contains("openrouter.ai") {
        return "openrouter".into();
    }
    if base.contains("dashscope") || base.contains("aliyuncs.com") {
        return "qwen".into();
    }
    if base.contains("moonshot") || base.contains("kimi") {
        return "kimi".into();
    }
    if base.contains("minimax") {
        return "minimax".into();
    }
    if base.contains("bigmodel.cn") || base.contains("zhipuai") {
        return "glm".into();
    }

    "openai".into()
}

fn strip_trailing_v1(url: &str) -> String {
    url.trim_end_matches('/')
        .trim_end_matches("/v1")
        .trim_end_matches('/')
        .to_string()
}

fn resolve_openai_compat_base_url(platform: &str, base_url: &str) -> Option<String> {
    if platform == "gemini" {
        let raw = base_url.trim().trim_end_matches('/');
        if raw.is_empty() {
            return Some("https://generativelanguage.googleapis.com/v1beta/openai".into());
        }
        if raw.ends_with("/v1beta/openai") {
            return Some(raw.to_string());
        }
        return Some(format!("{raw}/v1beta/openai"));
    }
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub fn resolve_finclaw_llm_base_url(platform: &str, base_url: &str, provider: &str) -> Option<String> {
    if provider == "anthropic" {
        let url = base_url.trim();
        return if url.is_empty() {
            None
        } else {
            Some(strip_trailing_v1(url))
        };
    }

    let open_ai_compat = resolve_openai_compat_base_url(platform, base_url)?;
    let normalized = strip_trailing_v1(&open_ai_compat);
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

pub fn build_finclaw_llm_config_slice(
    platform: &str,
    base_url: &str,
    model_id: &str,
    api_key: &str,
    model_protocols: Option<&str>,
) -> FinclawLlmConfigSlice {
    let provider = infer_finclaw_llm_provider(platform, base_url, model_id, model_protocols);
    let base = resolve_finclaw_llm_base_url(platform, base_url, &provider);
    FinclawLlmConfigSlice {
        provider,
        model: model_id.to_string(),
        base_url: base,
        api_key: if api_key.trim().is_empty() {
            None
        } else {
            Some(api_key.to_string())
        },
    }
}

pub fn build_finclaw_model_fingerprint(slice: &FinclawLlmConfigSlice) -> String {
    format!(
        "{}\0{}\0{}\0{}",
        slice.provider,
        slice.model,
        slice.base_url.as_deref().unwrap_or(""),
        slice.api_key.as_deref().unwrap_or(""),
    )
}

/// Env overrides for `finclaw serve` — mirrors 1.9.25 `buildFinclawLlmServeEnv`.
pub fn build_finclaw_llm_serve_env(slice: &FinclawLlmConfigSlice) -> HashMap<String, String> {
    let mut env = HashMap::from([
        ("FINCLAW_LLM_PROVIDER".into(), slice.provider.clone()),
        ("FINCLAW_LLM_MODEL".into(), slice.model.clone()),
    ]);

    if let Some(base_url) = &slice.base_url {
        env.insert("FINCLAW_LLM_BASE_URL".into(), base_url.clone());
    }
    if let Some(api_key) = &slice.api_key {
        env.insert("FINCLAW_LLM_API_KEY".into(), api_key.clone());
        let provider_key = match slice.provider.as_str() {
            "anthropic" => "ANTHROPIC_API_KEY",
            "deepseek" => "DEEPSEEK_API_KEY",
            "openrouter" => "OPENROUTER_API_KEY",
            "qwen" => "DASHSCOPE_API_KEY",
            "kimi" => "MOONSHOT_API_KEY",
            "minimax" => "MINIMAX_API_KEY",
            "glm" => "ZHIPUAI_API_KEY",
            _ => "OPENAI_API_KEY",
        };
        env.insert(provider_key.into(), api_key.clone());
    }

    env
}

async fn finclaw_config_set(
    findesk: &FindeskConfig,
    cli_path: Option<&str>,
    profile: &str,
    key: &str,
    value: &str,
) -> Result<(), String> {
    let binary = resolve_finclaw_binary(findesk, cli_path)
        .ok_or_else(|| "finclaw binary not found (set AIONCORE_FINCLAW_BIN)".to_string())?;
    let mut builder = Builder::clean_cli(&binary);
    builder.backend("finclaw");
    builder.env("FINCLAW_PROFILE", profile);
    builder.arg("config");
    builder.arg("set");
    builder.arg(key);
    builder.arg(value);
    builder.arg("--profile");
    builder.arg(profile);
    builder.arg("--quiet");
    builder
        .output()
        .await
        .map_err(|e| format!("failed to run finclaw config set {key}: {e}"))?;
    Ok(())
}

/// Write FinDesk model selection into finclaw profile config before serve starts.
pub async fn sync_finclaw_profile_llm_config(
    findesk: &FindeskConfig,
    cli_path: Option<&str>,
    profile: &str,
    slice: &FinclawLlmConfigSlice,
) -> Result<(), String> {
    finclaw_config_set(findesk, cli_path, profile, "llm.provider", &slice.provider).await?;
    finclaw_config_set(findesk, cli_path, profile, "llm.model", &slice.model).await?;
    if let Some(base_url) = &slice.base_url {
        finclaw_config_set(findesk, cli_path, profile, "llm.base_url", base_url).await?;
    }
    if let Some(api_key) = &slice.api_key {
        finclaw_config_set(findesk, cli_path, profile, "llm.api_key", api_key).await?;
    }
    Ok(())
}

pub async fn prepare_finclaw_llm_for_profiles(
    findesk: &FindeskConfig,
    cli_path: Option<&str>,
    base_profile: &str,
    derived_profile: &str,
    slice: &FinclawLlmConfigSlice,
) -> Result<HashMap<String, String>, String> {
    sync_finclaw_profile_llm_config(findesk, cli_path, derived_profile, slice).await?;
    if derived_profile != base_profile {
        sync_finclaw_profile_llm_config(findesk, cli_path, base_profile, slice).await?;
    }
    Ok(build_finclaw_llm_serve_env(slice))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_deepseek_host_to_deepseek_provider() {
        assert_eq!(
            infer_finclaw_llm_provider("custom", "https://api.deepseek.com/v1", "deepseek-v4-flash", None),
            "deepseek"
        );
    }

    #[test]
    fn builds_serve_env_with_provider_specific_api_key() {
        let slice = FinclawLlmConfigSlice {
            provider: "deepseek".into(),
            model: "deepseek-v4-flash".into(),
            base_url: Some("https://api.deepseek.com".into()),
            api_key: Some("sk-test".into()),
        };
        let env = build_finclaw_llm_serve_env(&slice);
        assert_eq!(env.get("FINCLAW_LLM_PROVIDER"), Some(&"deepseek".into()));
        assert_eq!(env.get("DEEPSEEK_API_KEY"), Some(&"sk-test".into()));
    }
}
