use std::path::{Path, PathBuf};

/// FinClaw reads skills from the active profile's `skills/` tree. AionCore wires
/// builtin skills into `{workspace}/.finclaw/skills/` instead. When that directory
/// exists, merge a workspace-local config overlay that registers it via
/// `skills.external_dirs` (preserving existing keys such as `presets.tool`).
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

    let mut body = if config_path.is_file() {
        std::fs::read_to_string(&config_path).ok().unwrap_or_default()
    } else {
        String::new()
    };

    body = merge_skills_external_dirs(&body, &skills_dir);
    std::fs::write(&config_path, body).ok()?;
    Some(config_path)
}

fn merge_skills_external_dirs(body: &str, skills_dir: &Path) -> String {
    let quoted = yaml_double_quoted_path(skills_dir);
    let external_line = format!("    - {quoted}");

    if body.contains("skills:") {
        if body.contains("external_dirs:") {
            return replace_external_dirs_entry(body, &external_line);
        }
        return insert_after_skills_header(body, &format!("  external_dirs:\n{external_line}\n"));
    }

    if body.trim().is_empty() {
        return format!("skills:\n  external_dirs:\n{external_line}\n");
    }

    format!("{body}\nskills:\n  external_dirs:\n{external_line}\n")
}

fn insert_after_skills_header(body: &str, block: &str) -> String {
    let mut lines: Vec<String> = body.lines().map(str::to_string).collect();
    for (idx, line) in lines.iter().enumerate() {
        if line.trim_start() == "skills:" {
            lines.insert(idx + 1, block.trim_end().to_string());
            return lines.join("\n") + "\n";
        }
    }
    format!("{body}\n{block}")
}

fn replace_external_dirs_entry(body: &str, external_line: &str) -> String {
    let mut lines: Vec<String> = body.lines().map(str::to_string).collect();
    let mut in_external = false;
    let mut replaced = false;

    for line in &mut lines {
        let trimmed = line.trim_start();
        if trimmed == "external_dirs:" {
            in_external = true;
            continue;
        }
        if in_external {
            if trimmed.starts_with("- ") {
                if !replaced {
                    let indent = line.len() - trimmed.len();
                    *line = format!("{}{external_line}", " ".repeat(indent));
                    replaced = true;
                }
                continue;
            }
            if !trimmed.is_empty() && !trimmed.starts_with('#') {
                in_external = false;
            }
        }
    }

    if !replaced {
        return insert_after_skills_header(body, &format!("  external_dirs:\n{external_line}\n"));
    }

    lines.join("\n") + "\n"
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
        assert_eq!(config, tmp.path().join(".finclaw").join("aionui-serve-config.yaml"));
        let body = std::fs::read_to_string(&config).unwrap();
        assert!(body.contains("external_dirs:"));
        assert!(body.contains(".finclaw/skills"));
    }

    #[test]
    fn preserves_presets_tool_when_merging_skills_overlay() {
        let tmp = TempDir::new().unwrap();
        let finclaw_dir = tmp.path().join(".finclaw");
        std::fs::create_dir_all(&finclaw_dir).unwrap();
        std::fs::write(
            finclaw_dir.join("aionui-serve-config.yaml"),
            "presets:\n  tool: deny_all\n",
        )
        .unwrap();

        let skills = finclaw_dir.join("skills").join("cron");
        std::fs::create_dir_all(&skills).unwrap();
        std::fs::write(skills.join("SKILL.md"), "---\nname: cron\n---\n").unwrap();

        let config = prepare_workspace_skills_serve_config(tmp.path()).expect("config");
        let body = std::fs::read_to_string(&config).unwrap();
        assert!(body.contains("presets:"));
        assert!(body.contains("tool: deny_all"));
        assert!(body.contains("external_dirs:"));
    }
}
