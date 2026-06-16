use std::path::Path;

/// FinClaw tool-invocation autonomy preset exposed in Findesk UI (maps to profile `presets.tool`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinclawToolPolicy {
    Supervised,
    Auto,
    Readonly,
}

pub fn is_finclaw_tool_policy(value: &str) -> bool {
    matches!(value.trim(), "supervised" | "auto" | "readonly")
}

pub fn finclaw_tool_policy_to_preset(policy: &str) -> Option<&'static str> {
    match policy.trim() {
        "supervised" => Some("ask_for_writes"),
        "auto" => Some("auto_all"),
        "readonly" => Some("deny_all"),
        _ => None,
    }
}

pub fn finclaw_preset_to_tool_policy(preset: &str) -> Option<FinclawToolPolicy> {
    match preset.trim() {
        "ask_for_writes" => Some(FinclawToolPolicy::Supervised),
        "auto_all" => Some(FinclawToolPolicy::Auto),
        "deny_all" => Some(FinclawToolPolicy::Readonly),
        _ => None,
    }
}

/// Prefer conversation-stored policy over workspace profile disk read.
pub fn resolve_finclaw_tool_policy_display(
    conversation_policy: Option<&str>,
    profile_policy: Option<FinclawToolPolicy>,
) -> FinclawToolPolicy {
    if let Some(policy) = conversation_policy.filter(|p| is_finclaw_tool_policy(p)) {
        return match policy {
            "supervised" => FinclawToolPolicy::Supervised,
            "auto" => FinclawToolPolicy::Auto,
            _ => FinclawToolPolicy::Readonly,
        };
    }
    profile_policy.unwrap_or(FinclawToolPolicy::Auto)
}

/// Write `presets.tool` into the workspace serve overlay consumed by `finclaw serve --config`.
pub fn apply_tool_policy_serve_overlay(workspace: &Path, policy: &str) -> Result<(), String> {
    let preset =
        finclaw_tool_policy_to_preset(policy).ok_or_else(|| format!("unknown finclaw tool policy: {policy}"))?;

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
/// Defaults to `auto` (direct tool use) when the conversation has no stored policy.
pub fn apply_tool_policy_from_extra(workspace: &Path, finclaw_tool_policy: Option<&str>) -> Result<(), String> {
    let policy = finclaw_tool_policy
        .filter(|p| !p.is_empty() && is_finclaw_tool_policy(p))
        .unwrap_or("auto");
    apply_tool_policy_serve_overlay(workspace, policy)
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
            return finclaw_preset_to_tool_policy(preset);
        }
    }
    None
}

pub fn finclaw_tool_policy_to_wire(policy: FinclawToolPolicy) -> &'static str {
    match policy {
        FinclawToolPolicy::Supervised => "supervised",
        FinclawToolPolicy::Auto => "auto",
        FinclawToolPolicy::Readonly => "readonly",
    }
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
    fn maps_policy_to_preset() {
        assert_eq!(finclaw_tool_policy_to_preset("supervised"), Some("ask_for_writes"));
        assert_eq!(finclaw_tool_policy_to_preset("auto"), Some("auto_all"));
        assert_eq!(finclaw_tool_policy_to_preset("readonly"), Some("deny_all"));
    }

    #[test]
    fn resolve_display_prefers_conversation_policy() {
        assert_eq!(
            resolve_finclaw_tool_policy_display(Some("readonly"), Some(FinclawToolPolicy::Auto)),
            FinclawToolPolicy::Readonly
        );
    }

    #[test]
    fn writes_presets_tool_overlay() {
        let dir = tempdir().unwrap();
        apply_tool_policy_serve_overlay(dir.path(), "supervised").unwrap();
        let body = std::fs::read_to_string(dir.path().join(".finclaw/aionui-serve-config.yaml")).unwrap();
        assert!(body.contains("presets:"));
        assert!(body.contains("tool: ask_for_writes"));
    }

    #[test]
    fn apply_from_extra_defaults_to_auto_when_missing() {
        let dir = tempdir().unwrap();
        apply_tool_policy_from_extra(dir.path(), None).unwrap();
        let body = std::fs::read_to_string(dir.path().join(".finclaw/aionui-serve-config.yaml")).unwrap();
        assert!(body.contains("tool: auto_all"));
    }

    #[test]
    fn reads_tool_policy_from_serve_overlay() {
        let dir = tempdir().unwrap();
        apply_tool_policy_serve_overlay(dir.path(), "readonly").unwrap();
        assert_eq!(
            read_tool_policy_from_serve_overlay(dir.path()),
            Some(FinclawToolPolicy::Readonly)
        );
    }
}
