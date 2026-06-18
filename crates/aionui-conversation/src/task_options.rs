//! Shared helpers for translating a [`ConversationRow`] into the inputs
//! agent factories expect.
//!
//! The two execution entry points — interactive `send_message` and the
//! cron executor — must derive the same `(provider_id, model)` for a
//! given conversation; otherwise an aionrs job that runs fine
//! interactively can fail under cron with `Provider '<vendor>' not
//! found` (Sentry ELECTRON-1HM). Centralising the lookup here forces
//! both paths through one parser.
//!
//! The parser intentionally accepts both the canonical `ProviderWithModel`
//! shape and a few legacy variants (camelCase keys, `id` instead of
//! `provider_id`). When the row holds an unparseable or missing model,
//! we return an empty `ProviderWithModel`; non-aionrs factory branches
//! ignore the field, and the aionrs branch surfaces a clear "provider
//! not found" error against an empty id rather than a stale vendor
//! label.

use aionui_common::ProviderWithModel;
use aionui_db::models::ConversationRow;

/// Non-aionrs conversations (FinClaw, etc.) persist model in `extra.providerModel`
/// per desktop spec 2026-05-12; aionrs uses the top-level `model` column.
pub const PROVIDER_MODEL_EXTRA_KEY: &str = "providerModel";

/// Resolve a conversation row's stored model into a [`ProviderWithModel`].
///
/// Returns an empty `ProviderWithModel { provider_id: "", model: "", use_model: None }`
/// when neither the `model` column nor `extra.providerModel` is parseable.
pub fn provider_model_from_conversation_row(row: &ConversationRow) -> ProviderWithModel {
    if let Some(parsed) = row.model.as_deref().and_then(parse_provider_with_model_loose) {
        return parsed;
    }

    provider_model_from_extra_json(&row.extra).unwrap_or_else(empty_provider_model)
}

fn provider_model_from_extra_json(extra_raw: &str) -> Option<ProviderWithModel> {
    let extra = serde_json::from_str::<serde_json::Value>(extra_raw).ok()?;
    let provider_model = extra.get(PROVIDER_MODEL_EXTRA_KEY)?;
    parse_provider_with_model_value(provider_model)
}

/// Canonical sentinel `ProviderWithModel` used when a conversation row has
/// no parseable model. Shared by both the interactive `send_message` path
/// and the cron executor so they agree on the "no model selected" shape:
/// `provider_id: ""`, `model: ""`, `use_model: None`. Non-aionrs factories
/// ignore the field, while the aionrs factory surfaces a clear "Provider
/// '' not found" error against the empty id rather than silently using a
/// stale vendor label.
pub fn empty_provider_model() -> ProviderWithModel {
    ProviderWithModel {
        provider_id: String::new(),
        model: String::new(),
        use_model: None,
    }
}

/// Returns true when `backend` names an agent/vendor label rather than a
/// provider row id stored in FinDesk settings.
pub fn is_known_agent_backend_label(backend: &str) -> bool {
    matches!(
        backend.trim().to_ascii_lowercase().as_str(),
        "finclaw" | "aionrs" | "acp" | "claude" | "gemini" | "codex" | "hermes"
    )
}

/// Serialize a provider model for `extra.providerModel`.
pub fn provider_model_extra_value(model: &ProviderWithModel) -> serde_json::Value {
    serde_json::json!({
        "provider_id": model.provider_id,
        "model": model.model,
        "use_model": model
            .use_model
            .clone()
            .unwrap_or_else(|| model.model.clone()),
    })
}

/// Permissive parser for `conversation.model` JSON.
///
/// Tries strict serde first, then falls back to manual extraction so older
/// shapes (camelCase, `id` instead of `provider_id`) keep working. Returns
/// `None` when no `provider_id` can be extracted; callers treat that as
/// "no model selected".
fn parse_provider_with_model_loose(raw: &str) -> Option<ProviderWithModel> {
    let value = serde_json::from_str::<serde_json::Value>(raw).ok()?;
    parse_provider_with_model_value(&value)
}

fn parse_provider_with_model_value(value: &serde_json::Value) -> Option<ProviderWithModel> {
    if let Ok(model) = serde_json::from_value::<ProviderWithModel>(value.clone()) {
        if !model.provider_id.is_empty() {
            return Some(model);
        }
    }

    let provider_id = value
        .get("provider_id")
        .or_else(|| value.get("providerId"))
        .or_else(|| value.get("id"))
        .and_then(|item| item.as_str())
        .unwrap_or_default()
        .to_owned();

    if provider_id.is_empty() {
        return None;
    }

    let model = value
        .get("model")
        .and_then(|item| item.as_str())
        .unwrap_or_default()
        .to_owned();
    let use_model = value
        .get("use_model")
        .or_else(|| value.get("useModel"))
        .and_then(|item| item.as_str())
        .map(ToOwned::to_owned);

    Some(ProviderWithModel {
        provider_id,
        model,
        use_model,
    })
}

/// Effective model id used by agent factories (prefers `use_model` when set).
pub fn effective_model_id(model: &ProviderWithModel) -> &str {
    model
        .use_model
        .as_deref()
        .filter(|value| !value.is_empty())
        .unwrap_or(model.model.as_str())
}

fn provider_model_runtime_identity(model: &ProviderWithModel) -> (&str, &str) {
    (model.provider_id.as_str(), effective_model_id(model))
}

/// True when the provider row or effective model id changed.
pub fn provider_model_runtime_identity_changed(before: &ProviderWithModel, after: &ProviderWithModel) -> bool {
    provider_model_runtime_identity(before) != provider_model_runtime_identity(after)
}

/// Detect `extra.providerModel` patches that change the runtime model for non-aionrs conversations.
pub fn extra_provider_model_patch_changed(
    existing: &ConversationRow,
    merged_extra: Option<&str>,
    req_extra: Option<&serde_json::Value>,
) -> bool {
    if !req_extra.is_some_and(|extra| extra.get(PROVIDER_MODEL_EXTRA_KEY).is_some()) {
        return false;
    }

    let before = provider_model_from_conversation_row(existing);
    let after_extra = merged_extra.unwrap_or(existing.extra.as_str());
    let after_row = ConversationRow {
        extra: after_extra.to_string(),
        ..existing.clone()
    };
    let after = provider_model_from_conversation_row(&after_row);
    provider_model_runtime_identity_changed(&before, &after)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row_with_model(model: Option<&str>) -> ConversationRow {
        row_with_model_and_extra(model, "{}")
    }

    fn row_with_model_and_extra(model: Option<&str>, extra: &str) -> ConversationRow {
        ConversationRow {
            id: "conv-1".into(),
            user_id: "user-1".into(),
            name: "test".into(),
            r#type: "aionrs".into(),
            model: model.map(ToOwned::to_owned),
            extra: extra.into(),
            status: None,
            source: None,
            channel_chat_id: None,
            pinned: false,
            pinned_at: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn finclaw_reads_provider_model_from_extra_when_column_empty() {
        let extra = r#"{"providerModel":{"provider_id":"deepseek-id","model":"deepseek-v4-flash"}}"#;
        let row = row_with_model_and_extra(None, extra);
        let m = provider_model_from_conversation_row(&row);
        assert_eq!(m.provider_id, "deepseek-id");
        assert_eq!(m.model, "deepseek-v4-flash");
    }

    #[test]
    fn column_model_takes_precedence_over_extra_provider_model() {
        let extra = r#"{"providerModel":{"provider_id":"from-extra","model":"m1"}}"#;
        let row = row_with_model_and_extra(Some(r#"{"provider_id":"from-column","model":"m2"}"#), extra);
        let m = provider_model_from_conversation_row(&row);
        assert_eq!(m.provider_id, "from-column");
        assert_eq!(m.model, "m2");
    }

    #[test]
    fn parses_canonical_shape() {
        let json = r#"{"provider_id":"abc123","model":"gpt-5","use_model":"gpt-5-turbo"}"#;
        let row = row_with_model(Some(json));
        let m = provider_model_from_conversation_row(&row);
        assert_eq!(m.provider_id, "abc123");
        assert_eq!(m.model, "gpt-5");
        assert_eq!(m.use_model.as_deref(), Some("gpt-5-turbo"));
    }

    #[test]
    fn parses_camelcase_legacy_shape() {
        let json = r#"{"providerId":"abc123","model":"gpt-5","useModel":"gpt-5-turbo"}"#;
        let row = row_with_model(Some(json));
        let m = provider_model_from_conversation_row(&row);
        assert_eq!(m.provider_id, "abc123");
        assert_eq!(m.model, "gpt-5");
        assert_eq!(m.use_model.as_deref(), Some("gpt-5-turbo"));
    }

    #[test]
    fn parses_id_alias() {
        let json = r#"{"id":"abc123","model":"gpt-5"}"#;
        let row = row_with_model(Some(json));
        let m = provider_model_from_conversation_row(&row);
        assert_eq!(m.provider_id, "abc123");
        assert_eq!(m.model, "gpt-5");
        assert!(m.use_model.is_none());
    }

    #[test]
    fn empty_provider_model_returns_documented_sentinel() {
        let m = empty_provider_model();
        assert!(m.provider_id.is_empty());
        assert!(m.model.is_empty());
        assert!(m.use_model.is_none());
    }

    #[test]
    fn null_model_returns_empty_sentinel() {
        let row = row_with_model(None);
        let m = provider_model_from_conversation_row(&row);
        assert!(m.provider_id.is_empty());
        assert!(m.model.is_empty());
        assert!(m.use_model.is_none());
    }

    #[test]
    fn invalid_json_returns_empty_sentinel() {
        let row = row_with_model(Some("not-json"));
        let m = provider_model_from_conversation_row(&row);
        assert!(m.provider_id.is_empty());
    }

    #[test]
    fn missing_provider_id_returns_empty_sentinel() {
        let json = r#"{"model":"gpt-5"}"#;
        let row = row_with_model(Some(json));
        let m = provider_model_from_conversation_row(&row);
        assert!(m.provider_id.is_empty());
    }

    /// Regression: the interactive `send_message` path and the cron
    /// executor must derive the same `(provider_id, model)` for a given
    /// conversation. Before this helper existed, cron read
    /// `agent_config.backend` (which fell back to the literal vendor
    /// label `"aionrs"` when the conversation's model JSON was an older
    /// shape) and `send_message` parsed the row directly, so the cron
    /// path would emit `Provider 'aionrs' not found` while the
    /// interactive path used the real provider hash. Now both paths
    /// route through `provider_model_from_conversation_row` and must
    /// agree on every row shape we accept.
    #[test]
    fn interactive_and_cron_paths_agree_on_provider_id() {
        // Canonical shape (what `build_task_options` previously parsed strictly).
        let canonical = r#"{"provider_id":"hash-abc","model":"gpt-5","use_model":null}"#;
        // Legacy camelCase shape (what cron's loose parser previously
        // accepted but `build_task_options`'s strict parser rejected).
        let legacy = r#"{"providerId":"hash-abc","model":"gpt-5"}"#;

        let canonical_row = row_with_model(Some(canonical));
        let legacy_row = row_with_model(Some(legacy));

        let canonical_resolved = provider_model_from_conversation_row(&canonical_row);
        let legacy_resolved = provider_model_from_conversation_row(&legacy_row);

        // Both shapes must resolve to the same provider hash so the cron
        // executor and interactive `send_message` can never diverge.
        assert_eq!(canonical_resolved.provider_id, "hash-abc");
        assert_eq!(legacy_resolved.provider_id, "hash-abc");
        assert_eq!(canonical_resolved.provider_id, legacy_resolved.provider_id);
        // The vendor-label fallback must not leak in.
        assert_ne!(canonical_resolved.provider_id, "aionrs");
        assert_ne!(legacy_resolved.provider_id, "aionrs");
    }

    #[test]
    fn extra_provider_model_patch_changed_detects_model_switch() {
        let mut existing = row_with_model_and_extra(
            None,
            r#"{"providerModel":{"provider_id":"deepseek-id","model":"deepseek-chat","use_model":"deepseek-chat"}}"#,
        );
        existing.r#type = "finclaw".into();

        let merged = r#"{"providerModel":{"provider_id":"kimi-id","model":"kimi-k2.6","use_model":"kimi-k2.6"}}"#;
        let req_extra = serde_json::json!({
            "providerModel": {
                "provider_id": "kimi-id",
                "model": "kimi-k2.6",
                "use_model": "kimi-k2.6"
            }
        });

        assert!(extra_provider_model_patch_changed(
            &existing,
            Some(merged),
            Some(&req_extra),
        ));
    }

    #[test]
    fn extra_provider_model_patch_ignored_without_provider_model_key() {
        let mut existing = row_with_model_and_extra(
            None,
            r#"{"providerModel":{"provider_id":"deepseek-id","model":"deepseek-chat"}}"#,
        );
        existing.r#type = "finclaw".into();

        let merged = r#"{"providerModel":{"provider_id":"deepseek-id","model":"deepseek-chat"},"note":"x"}"#;
        let req_extra = serde_json::json!({ "note": "x" });

        assert!(!extra_provider_model_patch_changed(
            &existing,
            Some(merged),
            Some(&req_extra),
        ));
    }
}
