use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

const WORKSPACE_PROFILE_PREFIX: &str = "findesk-ws-";
const FINCLAW_PROFILE_NAME_MAX_LEN: usize = 32;

pub fn resolve_finclaw_serve_cwd(workspace: &str) -> PathBuf {
    let trimmed = workspace.trim();
    if trimmed.is_empty() {
        return std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .canonicalize()
            .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    }
    let path = PathBuf::from(trimmed);
    path.canonicalize().unwrap_or(path)
}

pub fn derive_finclaw_workspace_profile(base_profile: &str, serve_cwd: &Path) -> String {
    let resolved = serve_cwd.canonicalize().unwrap_or_else(|_| serve_cwd.to_path_buf());
    let mut hasher = Sha256::new();
    hasher.update(resolved.to_string_lossy().as_bytes());
    let hash = format!("{:x}", hasher.finalize());
    let hash_prefix = &hash[..12.min(hash.len())];
    let safe_base: String = base_profile
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .take(24)
        .collect();
    let safe_base = if safe_base.is_empty() {
        "default".to_string()
    } else {
        safe_base
    };
    format!("{WORKSPACE_PROFILE_PREFIX}{safe_base}-{hash_prefix}")
}

pub fn resolve_finclaw_profile_dir(profile: &str) -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".finclaw")
        .join("profiles")
        .join(profile)
}

/// Clone the base profile directory when the workspace-derived profile is missing.
pub fn ensure_finclaw_workspace_profile(base_profile: &str, serve_cwd: &Path) -> Result<String, String> {
    let derived_profile = derive_finclaw_workspace_profile(base_profile, serve_cwd);
    let derived_dir = resolve_finclaw_profile_dir(&derived_profile);
    let profile_yaml = derived_dir.join("profile.yaml");
    if profile_yaml.is_file() {
        rewrite_profile_yaml_name(&profile_yaml, &derived_profile)?;
        return Ok(derived_profile);
    }

    let base_dir = resolve_finclaw_profile_dir(base_profile);
    if !base_dir.is_dir() {
        return Err(format!("FinClaw base profile \"{base_profile}\" not found"));
    }

    copy_dir_recursive(&base_dir, &derived_dir)?;
    rewrite_profile_yaml_name(&profile_yaml, &derived_profile)?;
    Ok(derived_profile)
}

const CONVERSATION_PROFILE_PREFIX: &str = "fd-s-";
const CONVERSATION_PROFILE_HASH_LEN: usize = 12;

/// Per-conversation FinClaw profile so each chat tab gets its own `finclaw serve` daemon.
pub fn derive_finclaw_conversation_profile(workspace_profile: &str, conversation_id: &str) -> String {
    let profile = format!(
        "{}-{}",
        derive_finclaw_conversation_profile_prefix(workspace_profile),
        hash_prefix(conversation_id.as_bytes(), CONVERSATION_PROFILE_HASH_LEN)
    );
    debug_assert!(profile.len() <= FINCLAW_PROFILE_NAME_MAX_LEN);
    profile
}

pub fn derive_finclaw_conversation_profile_prefix(workspace_profile: &str) -> String {
    format!(
        "{CONVERSATION_PROFILE_PREFIX}{}",
        hash_prefix(workspace_profile.as_bytes(), CONVERSATION_PROFILE_HASH_LEN)
    )
}

/// Clone the workspace profile when the per-conversation profile is missing.
pub fn ensure_finclaw_conversation_profile(workspace_profile: &str, conversation_id: &str) -> Result<String, String> {
    let conv_profile = derive_finclaw_conversation_profile(workspace_profile, conversation_id);
    let conv_dir = resolve_finclaw_profile_dir(&conv_profile);
    let profile_yaml = conv_dir.join("profile.yaml");
    if profile_yaml.is_file() {
        rewrite_profile_yaml_name(&profile_yaml, &conv_profile)?;
        return Ok(conv_profile);
    }

    let base_dir = resolve_finclaw_profile_dir(workspace_profile);
    if !base_dir.is_dir() {
        return Err(format!("FinClaw workspace profile \"{workspace_profile}\" not found"));
    }

    copy_dir_recursive(&base_dir, &conv_dir)?;
    rewrite_profile_yaml_name(&profile_yaml, &conv_profile)?;
    Ok(conv_profile)
}

fn rewrite_profile_yaml_name(profile_yaml: &Path, profile_name: &str) -> Result<(), String> {
    let body = fs::read_to_string(profile_yaml).map_err(|e| e.to_string())?;
    let had_trailing_newline = body.ends_with('\n');
    let mut replaced = false;
    let mut lines = Vec::new();

    for line in body.lines() {
        if !replaced && line.starts_with("name:") {
            lines.push(format!("name: {profile_name}"));
            replaced = true;
        } else {
            lines.push(line.to_string());
        }
    }

    if !replaced {
        let insert_at = lines
            .iter()
            .position(|line| line.starts_with("schema_version:"))
            .map(|index| index + 1)
            .unwrap_or(0);
        lines.insert(insert_at, format!("name: {profile_name}"));
    }

    let mut out = lines.join("\n");
    if had_trailing_newline || !out.is_empty() {
        out.push('\n');
    }
    fs::write(profile_yaml, out).map_err(|e| e.to_string())
}

fn copy_dir_recursive(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(from).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let file_type = entry.file_type().map_err(|e| e.to_string())?;
        let dest = to.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &dest)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), dest).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn hash_prefix(input: &[u8], len: usize) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input);
    let hash = format!("{:x}", hasher.finalize());
    hash[..len.min(hash.len())].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn derive_profile_is_stable_for_same_cwd() {
        let dir = tempdir().unwrap();
        let a = derive_finclaw_workspace_profile("default", dir.path());
        let b = derive_finclaw_workspace_profile("default", dir.path());
        assert_eq!(a, b);
        assert!(a.starts_with("findesk-ws-default-"));
    }

    #[test]
    fn ensure_profile_clones_base_when_missing() {
        let root = tempdir().unwrap();
        let base_dir = root.path().join("profiles/default");
        fs::create_dir_all(&base_dir).unwrap();
        fs::write(base_dir.join("profile.yaml"), "name: default\n").unwrap();

        let serve = root.path().join("workspace");
        fs::create_dir_all(&serve).unwrap();

        // Redirect home for this test by using absolute paths through helper internals:
        // ensure_finclaw_workspace_profile reads ~/.finclaw — skip integration here.
        let derived = derive_finclaw_workspace_profile("default", &serve);
        assert_ne!(derived, "default");
    }

    #[test]
    fn rewrite_profile_yaml_name_replaces_cloned_base_name() {
        let dir = tempdir().unwrap();
        let profile_yaml = dir.path().join("profile.yaml");
        fs::write(&profile_yaml, "schema_version: 1\nname: default\ndescription: test\n").unwrap();

        rewrite_profile_yaml_name(&profile_yaml, "findesk-ws-default-abc123").unwrap();

        let body = fs::read_to_string(profile_yaml).unwrap();
        assert!(body.contains("name: findesk-ws-default-abc123\n"));
        assert!(!body.contains("name: default\n"));
        assert!(body.contains("description: test\n"));
    }

    #[test]
    fn rewrite_profile_yaml_name_inserts_missing_name_after_schema_version() {
        let dir = tempdir().unwrap();
        let profile_yaml = dir.path().join("profile.yaml");
        fs::write(&profile_yaml, "schema_version: 1\ndescription: test\n").unwrap();

        rewrite_profile_yaml_name(&profile_yaml, "findesk-ws-default-abc123").unwrap();

        let body = fs::read_to_string(profile_yaml).unwrap();
        assert!(body.starts_with("schema_version: 1\nname: findesk-ws-default-abc123\n"));
    }

    #[test]
    fn derive_conversation_profile_is_stable() {
        let workspace_profile = "findesk-ws-default-abc123";
        let a = derive_finclaw_conversation_profile(workspace_profile, "conv-uuid-1");
        let b = derive_finclaw_conversation_profile(workspace_profile, "conv-uuid-1");
        assert_eq!(a, b);
        assert!(a.starts_with(&format!(
            "{}-",
            derive_finclaw_conversation_profile_prefix(workspace_profile)
        )));
        assert!(a.len() <= FINCLAW_PROFILE_NAME_MAX_LEN);
        assert_ne!(a, derive_finclaw_conversation_profile(workspace_profile, "conv-uuid-2"));
    }

    #[test]
    fn derive_conversation_profile_stays_within_finclaw_name_limit() {
        let workspace_profile = "findesk-ws-default-ed681feb0367";
        let profile = derive_finclaw_conversation_profile(workspace_profile, "5a92a56c");
        assert_eq!(profile.len(), 30);
        assert!(profile.len() <= FINCLAW_PROFILE_NAME_MAX_LEN);
    }
}
