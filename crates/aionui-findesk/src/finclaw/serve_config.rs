use std::path::{Path, PathBuf};

/// FinClaw reads skills from the active profile's `skills/` tree. AionCore wires
/// builtin skills into `{workspace}/.finclaw/skills/` instead. When that directory
/// exists, write a workspace-local config overlay that registers it via
/// `skills.external_dirs` and pass it to `finclaw serve --config`.
pub fn prepare_workspace_skills_serve_config(workspace: &Path) -> Option<PathBuf> {
    let skills_dir = workspace.join(".finclaw").join("skills");
    if !skills_dir.is_dir() {
        return None;
    }
    let has_entries = std::fs::read_dir(&skills_dir)
        .ok()
        .is_some_and(|mut entries| entries.next().is_some());
    if !has_entries {
        return None;
    }

    let finclaw_dir = workspace.join(".finclaw");
    std::fs::create_dir_all(&finclaw_dir).ok()?;
    let config_path = finclaw_dir.join("aionui-serve-config.yaml");
    let quoted = yaml_double_quoted_path(&skills_dir);
    let yaml = format!("skills:\n  external_dirs:\n    - {quoted}\n");
    std::fs::write(&config_path, yaml).ok()?;
    Some(config_path)
}

fn yaml_double_quoted_path(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let escaped = raw.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn returns_none_when_workspace_skills_dir_missing() {
        let tmp = TempDir::new().unwrap();
        assert!(prepare_workspace_skills_serve_config(tmp.path()).is_none());
    }

    #[test]
    fn writes_external_dirs_overlay_when_skills_present() {
        let tmp = TempDir::new().unwrap();
        let skills = tmp.path().join(".finclaw").join("skills").join("cron");
        std::fs::create_dir_all(&skills).unwrap();
        std::fs::write(skills.join("SKILL.md"), "---\nname: cron\n---\n").unwrap();

        let config = prepare_workspace_skills_serve_config(tmp.path()).expect("config");
        assert_eq!(
            config,
            tmp.path().join(".finclaw").join("aionui-serve-config.yaml")
        );
        let body = std::fs::read_to_string(&config).unwrap();
        assert!(body.contains("external_dirs:"));
        assert!(body.contains(".finclaw/skills"));
    }
}
