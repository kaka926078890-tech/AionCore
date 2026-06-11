//! Map AionUi / ACP `session_mode` labels to FinClaw `/ai/infer/stream` capability tokens.

/// FinClaw infer capability tokens (finclaw 0.5.2+).
pub const GENERAL: &str = "general";
pub const CODING: &str = "coding";
pub const READ_ONLY: &str = "read_only";
pub const CHANNEL_INBOUND: &str = "channel_inbound";

fn is_read_only_alias(lower: &str) -> bool {
    matches!(lower, "read_only" | "readonly" | "read-only")
}

fn is_coding_ui_alias(lower: &str) -> bool {
    matches!(
        lower,
        "yolo"
            | "bypasspermissions"
            | "build"
            | "auto"
            | "autoedit"
            | "auto_edit"
            | "acceptedits"
    )
}

/// Map conversation `session_mode` (ACP/UI labels) to FinClaw infer `capability`.
///
/// Ported from Findesk `resolveFinclawInferCapability.ts` — FinClaw rejects ACP-only
/// tokens such as `default` and `supervised`.
pub fn resolve_finclaw_infer_capability(session_mode: Option<&str>) -> &'static str {
    let Some(raw) = session_mode.map(str::trim).filter(|s| !s.is_empty()) else {
        return CODING;
    };

    let lower = raw.to_ascii_lowercase();
    match lower.as_str() {
        "general" => GENERAL,
        "coding" => CODING,
        "channel_inbound" => CHANNEL_INBOUND,
        s if is_read_only_alias(s) => READ_ONLY,
        "default" | "plan" => GENERAL,
        "ask" | "dontask" => READ_ONLY,
        s if is_coding_ui_alias(s) => CODING,
        _ => GENERAL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_defaults_to_coding() {
        assert_eq!(resolve_finclaw_infer_capability(None), CODING);
        assert_eq!(resolve_finclaw_infer_capability(Some("")), CODING);
    }

    #[test]
    fn maps_ui_modes_to_finclaw_tokens() {
        assert_eq!(resolve_finclaw_infer_capability(Some("default")), GENERAL);
        assert_eq!(resolve_finclaw_infer_capability(Some("supervised")), GENERAL);
        assert_eq!(resolve_finclaw_infer_capability(Some("yolo")), CODING);
        assert_eq!(resolve_finclaw_infer_capability(Some("readonly")), READ_ONLY);
        assert_eq!(resolve_finclaw_infer_capability(Some("ask")), READ_ONLY);
    }
}
