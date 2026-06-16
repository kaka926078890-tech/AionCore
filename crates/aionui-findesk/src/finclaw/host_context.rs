use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use aionui_api_types::{FinclawHostClaw, FinclawHostContext};

/// Agent row shape consumed by [`resolve_finclaw_host_context`].
#[derive(Debug, Clone)]
pub struct HostAgentRow {
    pub agent_type: String,
    pub backend: Option<String>,
    pub name: String,
    pub enabled: bool,
    pub native_skills_dirs: Option<Vec<String>>,
}

const EXTRA_CLAW_SKILLS_DIRS: &[(&str, &str)] = &[
    ("hermes", ".hermes/skills"),
    ("openclaw-gateway", ".openclaw/workspace/skills"),
    ("openclaw", ".openclaw/workspace/skills"),
    ("nanobot", ".nanobot/skills"),
];

fn ensure_skills_dir(skills_dir: &Path) -> PathBuf {
    let _ = fs::create_dir_all(skills_dir);
    skills_dir.to_path_buf()
}

fn expand_home_skills_dir(relative_path: &str) -> (PathBuf, PathBuf) {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let skills_dir = ensure_skills_dir(&home.join(relative_path.trim_start_matches("./")));
    let home_dir = skills_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home.clone());
    (skills_dir, home_dir)
}

fn resolve_global_claw_skills_dir(agent: &HostAgentRow) -> Option<(PathBuf, PathBuf)> {
    let backend = agent.backend.as_deref().unwrap_or(agent.agent_type.as_str());
    if let Some((_, relative)) = EXTRA_CLAW_SKILLS_DIRS.iter().find(|(key, _)| *key == backend) {
        return Some(expand_home_skills_dir(relative));
    }

    agent
        .native_skills_dirs
        .as_ref()
        .and_then(|dirs| dirs.first())
        .map(|primary| expand_home_skills_dir(primary))
}

fn build_finclaw_shared_skills_claw(display_name: &str, base_profile: &str) -> FinclawHostClaw {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let home_dir = home.join(".finclaw").join("profiles").join(base_profile);
    let skills_dir = ensure_skills_dir(&home_dir.join("skills"));
    FinclawHostClaw {
        id: base_profile.to_string(),
        name: display_name.to_string(),
        skills_dir: skills_dir.to_string_lossy().into_owned(),
        home_dir: Some(home_dir.to_string_lossy().into_owned()),
    }
}

fn build_global_claw_entry(agent: &HostAgentRow) -> Option<FinclawHostClaw> {
    let (skills_dir, home_dir) = resolve_global_claw_skills_dir(agent)?;
    let backend = agent.backend.as_deref().unwrap_or(agent.agent_type.as_str());
    Some(FinclawHostClaw {
        id: format!("{backend}-default"),
        name: agent.name.clone(),
        skills_dir: skills_dir.to_string_lossy().into_owned(),
        home_dir: Some(home_dir.to_string_lossy().into_owned()),
    })
}

fn is_local_claw_install_target(agent: &HostAgentRow) -> bool {
    if agent.agent_type == "remote" {
        return false;
    }
    if agent.agent_type == "finclaw" || agent.backend.as_deref() == Some("finclaw") {
        return true;
    }
    resolve_global_claw_skills_dir(agent).is_some()
}

fn dedupe_claws_by_skills_dir(claws: Vec<FinclawHostClaw>) -> Vec<FinclawHostClaw> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();

    for claw in claws {
        let key = PathBuf::from(&claw.skills_dir)
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(&claw.skills_dir));
        let key = key.to_string_lossy().into_owned();
        if !seen.insert(key.clone()) {
            continue;
        }
        result.push(FinclawHostClaw {
            skills_dir: key,
            ..claw
        });
    }

    result.sort_by(|a, b| a.name.cmp(&b.name));
    result
}

/// Build the host claw list for FinSkills hub injection.
pub fn resolve_finclaw_host_context(agents: &[HostAgentRow]) -> FinclawHostContext {
    let local_agents: Vec<&HostAgentRow> = agents
        .iter()
        .filter(|agent| agent.enabled && is_local_claw_install_target(agent))
        .collect();

    let mut claws = Vec::new();

    if let Some(finclaw_agent) = local_agents
        .iter()
        .find(|agent| agent.agent_type == "finclaw" || agent.backend.as_deref() == Some("finclaw"))
    {
        claws.push(build_finclaw_shared_skills_claw(&finclaw_agent.name, "default"));
    }

    for agent in local_agents {
        if agent.agent_type == "finclaw" || agent.backend.as_deref() == Some("finclaw") {
            continue;
        }
        if let Some(claw) = build_global_claw_entry(agent) {
            claws.push(claw);
        }
    }

    FinclawHostContext {
        source: "finclaw".into(),
        claws: dedupe_claws_by_skills_dir(claws),
        user: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(agent_type: &str, backend: Option<&str>, name: &str) -> HostAgentRow {
        HostAgentRow {
            agent_type: agent_type.into(),
            backend: backend.map(str::to_string),
            name: name.into(),
            enabled: true,
            native_skills_dirs: None,
        }
    }

    #[test]
    fn includes_finclaw_shared_profile() {
        let ctx = resolve_finclaw_host_context(&[row("finclaw", Some("finclaw"), "FinClaw")]);
        assert_eq!(ctx.claws.len(), 1);
        assert_eq!(ctx.claws[0].id, "default");
        assert!(ctx.claws[0].skills_dir.contains(".finclaw/profiles/default/skills"));
    }

    #[test]
    fn includes_hermes_backend_claw() {
        let ctx = resolve_finclaw_host_context(&[row("acp", Some("hermes"), "Hermes")]);
        assert_eq!(ctx.claws.len(), 1);
        assert_eq!(ctx.claws[0].id, "hermes-default");
        assert!(ctx.claws[0].skills_dir.contains(".hermes/skills"));
    }

    #[test]
    fn skips_remote_agents() {
        let ctx = resolve_finclaw_host_context(&[row("remote", None, "Cloud")]);
        assert!(ctx.claws.is_empty());
    }

    #[test]
    fn dedupes_same_skills_dir() {
        let ctx = resolve_finclaw_host_context(&[
            row("acp", Some("hermes"), "Hermes A"),
            row("acp", Some("hermes"), "Hermes B"),
        ]);
        assert_eq!(ctx.claws.len(), 1);
    }
}
