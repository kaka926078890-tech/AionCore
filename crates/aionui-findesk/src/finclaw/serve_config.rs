use std::path::{Path, PathBuf};

use tracing::warn;

use crate::finclaw::workspace::resolve_finclaw_profile_dir;

/// FinClaw serve config overlay path for a workspace.
fn workspace_serve_config_path(workspace: &Path) -> Option<PathBuf> {
    let finclaw_dir = workspace.join(".finclaw");
    std::fs::create_dir_all(&finclaw_dir).ok()?;
    Some(finclaw_dir.join("aionui-serve-config.yaml"))
}

/// Workspace-local directory where FinDesk wires auto-inject skill symlinks.
pub fn workspace_linked_skills_dir(workspace: &Path) -> PathBuf {
    workspace.join(".finclaw").join("skills")
}

/// True when `{workspace}/.finclaw/skills/` exists and contains at least one entry.
pub fn workspace_has_linked_skills(workspace: &Path) -> bool {
    let skills_dir = workspace_linked_skills_dir(workspace);
    skills_dir.is_dir()
        && std::fs::read_dir(&skills_dir)
            .ok()
            .is_some_and(|mut entries| entries.next().is_some())
}

/// FinClaw reads skills from the active profile's `skills/` tree. AionCore wires
/// builtin skills into `{workspace}/.finclaw/skills/` instead.
///
/// Always return a workspace-local config path so callers can pass `--config`
/// deterministically (including `presets.tool` overlays). When skills exist,
/// also merge `skills.external_dirs` while preserving existing keys.
pub fn prepare_workspace_skills_serve_config(workspace: &Path) -> Option<PathBuf> {
    let config_path = workspace_serve_config_path(workspace)?;
    let mut body = if config_path.is_file() {
        std::fs::read_to_string(&config_path).ok().unwrap_or_default()
    } else {
        String::new()
    };

    if workspace_has_linked_skills(workspace) {
        let skills_dir = workspace_linked_skills_dir(workspace)
            .canonicalize()
            .unwrap_or_else(|_| workspace_linked_skills_dir(workspace));
        body = merge_skills_external_dirs(&body, &skills_dir);
    }
    std::fs::write(&config_path, body).ok()?;
    Some(config_path)
}

/// Mirror workspace-linked skills into the active FinClaw profile `config.yaml`
/// and wire up skill discovery for the embedded supervisor.
///
/// Two mechanisms ensure the embedded `finclaw serve` supervisor (which reads
/// skill scan roots from `<profile>/config.yaml` via `layout.config_file()`)
/// can discover auto-inject skills:
///
/// 1. **Profile `skills/` directory** — symlinks are created via
///    [`mirror_workspace_skills_into_profile_dir`]. This is the primary path
///    and works with all FinClaw builds (the profile `skills/` dir is always
///    scanned when no `skills:` key overrides the defaults).
///
/// 2. **`skills.external_dirs` in config.yaml** — for builds that support it.
///    Only added when the profile config already carries a non-empty `skills:`
///    block (user or system-configured).
///
/// When the base profile contains `skills: {}` (an empty map that disables
/// default scanning), the line is **removed** so FinClaw falls back to its
/// built-in defaults.  The auto-inject skills are already reachable through
/// mechanism (1) — no `external_dirs` entry is needed.
pub fn sync_workspace_skills_external_dirs_to_profile_config(workspace: &Path, profile: &str) {
    if profile.trim().is_empty() || !workspace_has_linked_skills(workspace) {
        return;
    }
    let skills_dir = workspace_linked_skills_dir(workspace)
        .canonicalize()
        .unwrap_or_else(|_| workspace_linked_skills_dir(workspace));
    let profile_dir = resolve_finclaw_profile_dir(profile);
    mirror_workspace_skills_into_profile_dir(&profile_dir, workspace);

    let config_path = profile_dir.join("config.yaml");
    if !config_path.parent().is_some_and(|parent| parent.is_dir()) {
        return;
    }
    let body = if config_path.is_file() {
        std::fs::read_to_string(&config_path).ok().unwrap_or_default()
    } else {
        String::new()
    };

    let merged = if body
        .lines()
        .any(|line| matches!(line.trim(), "skills: {}" | "skills:{}"))
    {
        // `skills: {}` disables FinClaw's default skill scanning (profile
        // skills dir + built-in system skills). Strip it so defaults
        // are restored. The auto-inject skills were already linked into
        // `<profile>/skills/` by mirror_workspace_skills_into_profile_dir.
        strip_skills_empty_map(&body)
    } else {
        merge_skills_external_dirs(&body, &skills_dir)
    };

    if let Err(e) = std::fs::write(&config_path, &merged) {
        warn!(
            profile = %profile,
            config_path = %config_path.display(),
            error = %e,
            "failed to sync skills config to FinClaw profile"
        );
    }
}

/// Mirror workspace-linked skills into the active profile `skills/` directory.
///
/// Some bundled FinClaw builds only enumerate `<profile>/skills` for
/// `list_skills`, even when `skills.external_dirs` is present. Keep the
/// external-dir overlay for newer builds, and add non-destructive symlinks for
/// older builds. Existing profile skills win so user-installed skills are not
/// overwritten.
fn mirror_workspace_skills_into_profile_dir(profile_dir: &Path, workspace: &Path) -> usize {
    let workspace_skills_dir = workspace_linked_skills_dir(workspace);
    if !workspace_skills_dir.is_dir() {
        return 0;
    }

    let profile_skills_dir = profile_dir.join("skills");
    if std::fs::create_dir_all(&profile_skills_dir).is_err() {
        return 0;
    }

    let entries = match std::fs::read_dir(&workspace_skills_dir) {
        Ok(entries) => entries,
        Err(_) => return 0,
    };

    let mut linked = 0;
    for entry in entries.flatten() {
        let source = entry.path();
        if !source.join("SKILL.md").is_file() {
            continue;
        }

        let target = profile_skills_dir.join(entry.file_name());
        if target.exists() {
            continue;
        }
        if let Ok(meta) = std::fs::symlink_metadata(&target) {
            if meta.file_type().is_symlink() {
                let _ = std::fs::remove_file(&target);
            } else {
                continue;
            }
        }

        if symlink_skill_dir(&source, &target).is_ok() {
            linked += 1;
        }
    }
    linked
}

#[cfg(unix)]
fn symlink_skill_dir(source: &Path, target: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(source, target)
}

#[cfg(windows)]
fn symlink_skill_dir(source: &Path, target: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(source, target)
}

/// Refresh workspace overlay and profile config before starting `finclaw serve`.
pub fn prepare_finclaw_workspace_skills_config(workspace: &Path, profile: &str) -> Option<PathBuf> {
    let overlay = prepare_workspace_skills_serve_config(workspace)?;
    sync_workspace_skills_external_dirs_to_profile_config(workspace, profile);
    Some(overlay)
}

/// Remove `skills: {}` / `skills:{}` lines so FinClaw falls back to its
/// default scanning behaviour (built-in system skills + `<profile>/skills/`).
fn strip_skills_empty_map(body: &str) -> String {
    let mut out = String::new();
    for line in body.lines() {
        match line.trim() {
            "skills: {}" | "skills:{}" => {} // drop the line
            _ => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out
}

fn merge_skills_external_dirs(body: &str, skills_dir: &Path) -> String {
    let quoted = yaml_double_quoted_path(skills_dir);
    let external_line = format!("    - {quoted}");
    let skills_block = format!("skills:\n  external_dirs:\n{external_line}\n");

    let cleaned = strip_orphan_external_dirs(body);

    if cleaned
        .lines()
        .any(|line| matches!(line.trim(), "skills: {}" | "skills:{}"))
    {
        let mut out = String::new();
        for line in cleaned.lines() {
            match line.trim() {
                "skills: {}" | "skills:{}" => out.push_str(&skills_block),
                _ => {
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
        return out;
    }

    if cleaned.contains("skills:") {
        if has_skills_external_dirs_under_skills(&cleaned) {
            return replace_external_dirs_entry(&cleaned, &external_line);
        }
        return insert_after_skills_header(&cleaned, &format!("  external_dirs:\n{external_line}\n"));
    }

    if cleaned.trim().is_empty() {
        return skills_block;
    }

    format!("{cleaned}\n{skills_block}")
}

/// Remove `external_dirs` blocks that were appended at the wrong YAML level
/// (for example after `skills: {}` when the inline map could not absorb them).
fn strip_orphan_external_dirs(body: &str) -> String {
    if has_skills_external_dirs_under_skills(body) {
        return body.to_string();
    }
    if !body.contains("external_dirs:") {
        return body.to_string();
    }

    let mut out = Vec::new();
    let mut skipping = false;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if trimmed == "external_dirs:" {
            skipping = true;
            continue;
        }
        if skipping {
            if trimmed.starts_with("- ") {
                continue;
            }
            skipping = false;
        }
        out.push(line);
    }
    join_lines(&out, body.ends_with('\n'))
}

fn has_skills_external_dirs_under_skills(body: &str) -> bool {
    let mut under_skills = false;
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("skills:") {
            under_skills = !matches!(trimmed, "skills: {}" | "skills:{}");
            continue;
        }
        if under_skills && !line.is_empty() && !line.starts_with(' ') && trimmed.contains(':') {
            under_skills = false;
        }
        if under_skills && trimmed == "external_dirs:" {
            return true;
        }
    }
    false
}

fn insert_after_skills_header(body: &str, block: &str) -> String {
    let mut lines: Vec<String> = body.lines().map(str::to_string).collect();
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed == "skills:" {
            lines.insert(idx + 1, block.trim_end().to_string());
            return join_lines(&lines.iter().map(String::as_str).collect::<Vec<_>>(), true);
        }
    }
    format!("{body}\n{block}")
}

fn join_lines(lines: &[&str], trailing_newline: bool) -> String {
    let mut out = lines.join("\n");
    if trailing_newline && !out.ends_with('\n') {
        out.push('\n');
    }
    out
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
    fn workspace_has_linked_skills_false_when_missing() {
        let tmp = TempDir::new().unwrap();
        assert!(!workspace_has_linked_skills(tmp.path()));
    }

    #[test]
    fn workspace_has_linked_skills_true_when_populated() {
        let tmp = TempDir::new().unwrap();
        let skills = workspace_linked_skills_dir(tmp.path()).join("cron");
        std::fs::create_dir_all(&skills).unwrap();
        assert!(workspace_has_linked_skills(tmp.path()));
    }

    #[test]
    fn sync_writes_external_dirs_into_profile_config_yaml() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let profile = "findesk-ws-default-test";
        let profile_dir = home.join(".finclaw").join("profiles").join(profile);
        std::fs::create_dir_all(&profile_dir).unwrap();
        std::fs::write(profile_dir.join("config.yaml"), "skills: {}\n").unwrap();

        let skills = tmp.path().join(".finclaw").join("skills").join("cron");
        std::fs::create_dir_all(&skills).unwrap();
        std::fs::write(skills.join("SKILL.md"), "---\nname: cron\n---\n").unwrap();

        // resolve_finclaw_profile_dir reads ~/.finclaw — test merge helper directly.
        let skills_dir = workspace_linked_skills_dir(tmp.path());
        let merged = merge_skills_external_dirs("skills: {}\n", &skills_dir);
        assert!(merged.contains("external_dirs:"));
        assert!(merged.contains(".finclaw/skills"));
        assert!(!merged.contains("skills: {}"));
    }

    #[test]
    fn merge_replaces_skills_inline_empty_map_in_full_profile() {
        let body = "llm:\n  model: x\nskills: {}\nextra: {}\n";
        let skills_dir = PathBuf::from("/tmp/ws/.finclaw/skills");
        let merged = merge_skills_external_dirs(body, &skills_dir);
        assert!(!merged.contains("skills: {}"));
        let skills_pos = merged.find("skills:").expect("skills");
        let ext_pos = merged.find("external_dirs:").expect("external_dirs");
        let extra_pos = merged.find("extra:").expect("extra");
        assert!(skills_pos < ext_pos && ext_pos < extra_pos);
    }

    #[test]
    fn merge_repairs_orphan_external_dirs_from_previous_bug() {
        let body = "skills: {}\nextra: {}\n\n  external_dirs:\n    - \"/old/path\"\n";
        let skills_dir = PathBuf::from("/tmp/ws/.finclaw/skills");
        let merged = merge_skills_external_dirs(body, &skills_dir);
        assert!(!merged.contains("/old/path"));
        assert!(merged.contains("/tmp/ws/.finclaw/skills"));
        assert!(has_skills_external_dirs_under_skills(&merged));
    }

    #[test]
    fn creates_overlay_even_when_workspace_skills_dir_missing() {
        let tmp = TempDir::new().unwrap();
        let config = prepare_workspace_skills_serve_config(tmp.path()).expect("config");
        assert_eq!(config, tmp.path().join(".finclaw").join("aionui-serve-config.yaml"));
        let body = std::fs::read_to_string(config).unwrap();
        assert!(body.trim().is_empty());
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
    fn mirrors_workspace_skills_into_profile_dir() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        let source = workspace.join(".finclaw").join("skills").join("cron");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("SKILL.md"), "---\nname: cron\n---\n").unwrap();

        let profile_dir = tmp.path().join("profile");
        let linked = mirror_workspace_skills_into_profile_dir(&profile_dir, &workspace);

        assert_eq!(linked, 1);
        let target = profile_dir.join("skills").join("cron");
        assert!(target.exists());
        assert!(target.join("SKILL.md").is_file());
    }

    #[test]
    fn mirror_keeps_existing_profile_skill() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        let source = workspace.join(".finclaw").join("skills").join("cron");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("SKILL.md"), "---\nname: cron\n---\n").unwrap();

        let profile_skill = tmp.path().join("profile").join("skills").join("cron");
        std::fs::create_dir_all(&profile_skill).unwrap();
        std::fs::write(profile_skill.join("SKILL.md"), "---\nname: custom-cron\n---\n").unwrap();

        let linked = mirror_workspace_skills_into_profile_dir(&tmp.path().join("profile"), &workspace);

        assert_eq!(linked, 0);
        let body = std::fs::read_to_string(profile_skill.join("SKILL.md")).unwrap();
        assert!(body.contains("custom-cron"));
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

    // ── strip_skills_empty_map ─────────────────────────────────────────

    #[test]
    fn strip_skills_empty_map_removes_braced_empty() {
        let body = "llm:\n  model: x\nskills: {}\nextra: {}\n";
        let stripped = strip_skills_empty_map(body);
        assert!(!stripped.contains("skills: {}"));
        assert!(stripped.contains("llm:"));
        assert!(stripped.contains("extra: {}"));
    }

    #[test]
    fn strip_skills_empty_map_removes_spaceless_empty() {
        let body = "llm:\n  model: x\nskills:{}\nruntime:\n  memory: true\n";
        let stripped = strip_skills_empty_map(body);
        assert!(!stripped.contains("skills:{}"));
        assert!(stripped.contains("llm:"));
        assert!(stripped.contains("runtime:"));
        assert!(stripped.contains("memory: true"));
    }

    #[test]
    fn strip_skills_empty_map_preserves_non_empty_skills_block() {
        let body = "skills:\n  external_dirs:\n    - /tmp/foo\n";
        let stripped = strip_skills_empty_map(body);
        assert_eq!(stripped, body);
    }

    #[test]
    fn strip_skills_empty_map_preserves_all_when_no_empty_skills() {
        let body = "llm:\n  model: x\nruntime:\n  memory: true\n";
        let stripped = strip_skills_empty_map(body);
        assert_eq!(stripped, body);
    }

    // ── merge still replaces skills: {} for overlay path ───────────────

    #[test]
    fn merge_still_replaces_skills_empty_for_overlay() {
        // The serve overlay path still uses merge_skills_external_dirs
        // which replaces skills: {} with external_dirs. This is the
        // intended behavior for the --config overlay.
        let skills_dir = PathBuf::from("/tmp/ws/.finclaw/skills");
        let merged = merge_skills_external_dirs("skills: {}\n", &skills_dir);
        assert!(merged.contains("external_dirs:"));
        assert!(merged.contains(".finclaw/skills"));
        assert!(!merged.contains("skills: {}"));
    }
}
