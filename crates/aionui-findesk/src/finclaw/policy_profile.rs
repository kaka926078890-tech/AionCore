use std::path::Path;

/// FinClaw `presets.tool` tool-invocation policy (wire format matches FinClaw CLI).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinclawToolPolicy {
    AskForWrites,
    AutoAll,
    DenyAll,
}

impl FinclawToolPolicy {
    pub fn as_preset(self) -> &'static str {
        match self {
            Self::AskForWrites => "ask_for_writes",
            Self::AutoAll => "auto_all",
            Self::DenyAll => "deny_all",
        }
    }

    pub fn from_wire(value: &str) -> Option<Self> {
        match value.trim() {
            "ask_for_writes" => Some(Self::AskForWrites),
            "auto_all" => Some(Self::AutoAll),
            "deny_all" => Some(Self::DenyAll),
            // Legacy Findesk tokens on older conversations
            "supervised" => Some(Self::AskForWrites),
            "auto" => Some(Self::AutoAll),
            "readonly" => Some(Self::DenyAll),
            _ => None,
        }
    }
}

pub fn is_finclaw_tool_policy(value: &str) -> bool {
    FinclawToolPolicy::from_wire(value).is_some()
}

/// Prefer conversation-stored policy over workspace profile disk read.
pub fn resolve_finclaw_tool_policy_display(
    conversation_policy: Option<&str>,
    profile_policy: Option<FinclawToolPolicy>,
) -> FinclawToolPolicy {
    if let Some(policy) = conversation_policy.and_then(FinclawToolPolicy::from_wire) {
        return policy;
    }
    profile_policy.unwrap_or(FinclawToolPolicy::AutoAll)
}

/// Write `presets.tool` into the workspace serve overlay consumed by `finclaw serve --config`.
pub fn apply_tool_policy_serve_overlay(workspace: &Path, policy: &str) -> Result<(), String> {
    let preset = FinclawToolPolicy::from_wire(policy)
        .ok_or_else(|| format!("unknown finclaw tool policy: {policy}"))?
        .as_preset();

    let finclaw_dir = workspace.join(".finclaw");
    std::fs::create_dir_all(&finclaw_dir).map_err(|e| e.to_string())?;
    let config_path = finclaw_dir.join("aionui-serve-config.yaml");

    let mut body = if config_path.is_file() {
        std::fs::read_to_string(&config_path).map_err(|e| e.to_string())?
    } else {
        String::new()
    };

    if body.contains("presets:") {
        if let Some(start) = body.find("presets:") {
            let tail = &body[start..];
            if tail.contains("tool:") {
                body = replace_yaml_scalar(&body, "tool:", preset);
            } else {
                body.push_str(&format!("\npresets:\n  tool: {preset}\n"));
            }
        }
    } else if body.is_empty() {
        body = format!("presets:\n  tool: {preset}\n");
    } else {
        body.push_str(&format!("\npresets:\n  tool: {preset}\n"));
    }

    std::fs::write(&config_path, body).map_err(|e| e.to_string())?;
    Ok(())
}

/// Apply `finclaw_tool_policy` from conversation extra before serve starts.
/// Defaults to `auto_all` when the conversation has no stored policy.
pub fn apply_tool_policy_from_extra(workspace: &Path, finclaw_tool_policy: Option<&str>) -> Result<(), String> {
    let policy = finclaw_tool_policy
        .and_then(FinclawToolPolicy::from_wire)
        .unwrap_or(FinclawToolPolicy::AutoAll);
    apply_tool_policy_serve_overlay(workspace, policy.as_preset())
}

/// Read `presets.tool` from workspace serve overlay when present.
pub fn read_tool_policy_from_serve_overlay(workspace: &Path) -> Option<FinclawToolPolicy> {
    let config_path = workspace.join(".finclaw/aionui-serve-config.yaml");
    if !config_path.is_file() {
        return None;
    }
    let body = std::fs::read_to_string(&config_path).ok()?;
    for line in body.lines() {
        let trimmed = line.trim();
        if let Some(preset) = trimmed.strip_prefix("tool:").map(str::trim) {
            return FinclawToolPolicy::from_wire(preset);
        }
    }
    None
}

pub fn finclaw_tool_policy_to_wire(policy: FinclawToolPolicy) -> &'static str {
    policy.as_preset()
}

fn replace_yaml_scalar(body: &str, key: &str, value: &str) -> String {
    let mut lines: Vec<String> = body.lines().map(str::to_string).collect();
    for line in &mut lines {
        let trimmed = line.trim_start();
        if trimmed.starts_with(key) {
            let indent = line.len() - trimmed.len();
            *line = format!("{}{key} {value}", " ".repeat(indent));
            return lines.join("\n") + "\n";
        }
    }
    body.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parses_native_and_legacy_wire_values() {
        assert_eq!(
            FinclawToolPolicy::from_wire("ask_for_writes"),
            Some(FinclawToolPolicy::AskForWrites)
        );
        assert_eq!(
            FinclawToolPolicy::from_wire("auto_all"),
            Some(FinclawToolPolicy::AutoAll)
        );
        assert_eq!(
            FinclawToolPolicy::from_wire("deny_all"),
            Some(FinclawToolPolicy::DenyAll)
        );
        assert_eq!(
            FinclawToolPolicy::from_wire("supervised"),
            Some(FinclawToolPolicy::AskForWrites)
        );
    }

    #[test]
    fn resolve_display_prefers_conversation_policy() {
        assert_eq!(
            resolve_finclaw_tool_policy_display(Some("deny_all"), Some(FinclawToolPolicy::AutoAll)),
            FinclawToolPolicy::DenyAll
        );
    }

    #[test]
    fn writes_presets_tool_overlay() {
        let dir = tempdir().unwrap();
        apply_tool_policy_serve_overlay(dir.path(), "ask_for_writes").unwrap();
        let body = std::fs::read_to_string(dir.path().join(".finclaw/aionui-serve-config.yaml")).unwrap();
        assert!(body.contains("presets:"));
        assert!(body.contains("tool: ask_for_writes"));
    }

    #[test]
    fn apply_from_extra_defaults_to_auto_all_when_missing() {
        let dir = tempdir().unwrap();
        apply_tool_policy_from_extra(dir.path(), None).unwrap();
        let body = std::fs::read_to_string(dir.path().join(".finclaw/aionui-serve-config.yaml")).unwrap();
        assert!(body.contains("tool: auto_all"));
    }

    #[test]
    fn reads_tool_policy_from_serve_overlay() {
        let dir = tempdir().unwrap();
        apply_tool_policy_serve_overlay(dir.path(), "deny_all").unwrap();
        assert_eq!(
            read_tool_policy_from_serve_overlay(dir.path()),
            Some(FinclawToolPolicy::DenyAll)
        );
    }
}
