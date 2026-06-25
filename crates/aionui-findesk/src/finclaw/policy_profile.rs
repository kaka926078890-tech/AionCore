use std::path::Path;

use crate::finclaw::workspace::resolve_finclaw_profile_dir;

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

/// Read `presets.tool` from a FinClaw profile's `profile.yaml` when present.
pub fn read_tool_policy_from_profile(profile: &str) -> Option<FinclawToolPolicy> {
    let profile_path = resolve_finclaw_profile_dir(profile).join("profile.yaml");
    if !profile_path.is_file() {
        return None;
    }
    let body = std::fs::read_to_string(&profile_path).ok()?;
    read_tool_policy_from_presets_body(&body)
}

/// Resolve effective tool policy: conversation extra > serve overlay > workspace profile > `auto_all`.
pub fn resolve_effective_tool_policy(
    conversation_policy: Option<&str>,
    workspace: &Path,
    workspace_profile: Option<&str>,
) -> FinclawToolPolicy {
    if let Some(policy) = conversation_policy.and_then(FinclawToolPolicy::from_wire) {
        return policy;
    }
    if let Some(policy) = read_tool_policy_from_serve_overlay(workspace) {
        return policy;
    }
    if let Some(profile) = workspace_profile {
        if let Some(policy) = read_tool_policy_from_profile(profile) {
            return policy;
        }
    }
    FinclawToolPolicy::AutoAll
}

/// Write `presets.tool` into a FinClaw profile's `profile.yaml` and drop any overriding policy file.
pub fn apply_tool_policy_to_profile(profile: &str, policy: FinclawToolPolicy) -> Result<(), String> {
    let profile_dir = resolve_finclaw_profile_dir(profile);
    let profile_path = profile_dir.join("profile.yaml");
    if !profile_path.is_file() {
        return Err(format!("FinClaw profile \"{profile}\" is missing profile.yaml"));
    }

    let preset = policy.as_preset();
    let mut content = std::fs::read_to_string(&profile_path).map_err(|e| e.to_string())?;
    if content.lines().any(|line| line.trim_start().starts_with("tool:")) {
        content = replace_yaml_scalar(&content, "tool:", preset);
    } else if content.contains("presets:") {
        if content.contains("presets:\n") {
            content = content.replace("presets:\n", &format!("presets:\n  tool: {preset}\n"));
        } else {
            content.push_str(&format!("\npresets:\n  tool: {preset}\n"));
        }
    } else if content.trim().is_empty() {
        content = format!("presets:\n  tool: {preset}\n");
    } else {
        content = format!("{}\npresets:\n  tool: {preset}\n", content.trim_end());
    }

    std::fs::write(&profile_path, content).map_err(|e| e.to_string())?;

    let policy_path = profile_dir.join("policies/tool-invocation-policy.yaml");
    if policy_path.is_file() {
        let _ = std::fs::remove_file(policy_path);
    }
    Ok(())
}

fn try_apply_tool_policy_to_profile(profile: &str, policy: FinclawToolPolicy) -> Result<(), String> {
    let profile_path = resolve_finclaw_profile_dir(profile).join("profile.yaml");
    if !profile_path.is_file() {
        return Ok(());
    }
    apply_tool_policy_to_profile(profile, policy)
}

/// Sync overlay + workspace/conversation profiles before `finclaw serve` starts.
pub fn sync_tool_policy_for_serve(
    workspace: &Path,
    workspace_profile: &str,
    serve_profile: &str,
    conversation_policy: Option<&str>,
) -> Result<FinclawToolPolicy, String> {
    let policy = resolve_effective_tool_policy(conversation_policy, workspace, Some(workspace_profile));
    apply_tool_policy_serve_overlay(workspace, policy.as_preset())?;
    try_apply_tool_policy_to_profile(workspace_profile, policy)?;
    try_apply_tool_policy_to_profile(serve_profile, policy)?;
    Ok(policy)
}

/// Apply `finclaw_tool_policy` from conversation extra before serve starts.
/// When extra is missing, preserves an existing workspace overlay/profile policy.
pub fn apply_tool_policy_from_extra(workspace: &Path, finclaw_tool_policy: Option<&str>) -> Result<(), String> {
    let policy = resolve_effective_tool_policy(finclaw_tool_policy, workspace, None);
    apply_tool_policy_serve_overlay(workspace, policy.as_preset())
}

/// Read `presets.tool` from workspace serve overlay when present.
pub fn read_tool_policy_from_serve_overlay(workspace: &Path) -> Option<FinclawToolPolicy> {
    let config_path = workspace.join(".finclaw/aionui-serve-config.yaml");
    if !config_path.is_file() {
        return None;
    }
    let body = std::fs::read_to_string(&config_path).ok()?;
    read_tool_policy_from_presets_body(&body)
}

fn read_tool_policy_from_presets_body(body: &str) -> Option<FinclawToolPolicy> {
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
    fn apply_from_extra_preserves_existing_overlay_when_extra_missing() {
        let dir = tempdir().unwrap();
        apply_tool_policy_serve_overlay(dir.path(), "deny_all").unwrap();
        apply_tool_policy_from_extra(dir.path(), None).unwrap();
        assert_eq!(
            read_tool_policy_from_serve_overlay(dir.path()),
            Some(FinclawToolPolicy::DenyAll)
        );
    }

    #[test]
    fn resolve_effective_prefers_conversation_then_overlay() {
        let dir = tempdir().unwrap();
        apply_tool_policy_serve_overlay(dir.path(), "ask_for_writes").unwrap();
        assert_eq!(
            resolve_effective_tool_policy(Some("deny_all"), dir.path(), None),
            FinclawToolPolicy::DenyAll
        );
        assert_eq!(
            resolve_effective_tool_policy(None, dir.path(), None),
            FinclawToolPolicy::AskForWrites
        );
    }

    #[test]
    fn sync_tool_policy_writes_overlay_and_returns_resolved_policy() {
        let dir = tempdir().unwrap();
        let policy = sync_tool_policy_for_serve(dir.path(), "missing-profile", "missing-serve", Some("deny_all"))
            .expect("sync policy");
        assert_eq!(policy, FinclawToolPolicy::DenyAll);
        assert_eq!(
            read_tool_policy_from_serve_overlay(dir.path()),
            Some(FinclawToolPolicy::DenyAll)
        );
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
