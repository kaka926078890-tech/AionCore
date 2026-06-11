use std::collections::HashSet;
use std::env;
use std::path::{Path, PathBuf};

use crate::config::FindeskConfig;

#[derive(Debug, Clone)]
pub struct ProfileContext {
    pub backend: String,
    #[allow(dead_code)]
    pub executable: PathBuf,
    pub cwd: Option<PathBuf>,
    pub home_dir: PathBuf,
}

impl ProfileContext {
    pub fn from_intent(
        backend: Option<&str>,
        program: &Path,
        cwd: Option<&Path>,
        _config: &FindeskConfig,
    ) -> Self {
        let home_dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            backend: backend.unwrap_or("node-acp").to_string(),
            executable: program.to_path_buf(),
            cwd: cwd.map(Path::to_path_buf),
            home_dir,
        }
    }
}

pub fn resolve_profile_id(backend: &str) -> &'static str {
    match backend {
        "finclaw" => "finclaw-interactive",
        "hermes" => "hermes",
        "claude" | "codex" | "codebuddy" => "npx-bun-acp",
        "openclaw" | "openclaw-gateway" => "openclaw",
        "nanobot" => "nanobot",
        "aionrs" => "aionrs",
        _ => "node-acp",
    }
}

fn unique_paths(paths: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    paths
        .into_iter()
        .filter(|p| !p.as_os_str().is_empty())
        .filter(|p| seen.insert(p.clone()))
        .collect()
}

fn join_home(home: &Path, relative: &str) -> PathBuf {
    home.join(relative.trim_start_matches("./"))
}

fn workspace_paths(ctx: &ProfileContext) -> Vec<PathBuf> {
    let Some(cwd) = ctx.cwd.as_ref() else {
        return Vec::new();
    };
    let absolute = cwd.canonicalize().unwrap_or_else(|_| cwd.clone());
    if is_unsafe_workspace_root(&absolute, &ctx.home_dir) {
        return Vec::new();
    }
    vec![absolute]
}

fn is_unsafe_workspace_root(absolute: &Path, home_dir: &Path) -> bool {
    if absolute == Path::new("/") {
        return true;
    }
    if absolute == home_dir {
        return true;
    }
    false
}

fn findesk_base_read_write(ctx: &ProfileContext, config: &FindeskConfig) -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from(".")];
    paths.extend(workspace_paths(ctx));
    if let Some(dir) = config.work_dir.as_ref() {
        paths.push(dir.clone());
    }
    if let Some(dir) = config.cache_dir.as_ref() {
        paths.push(dir.clone());
    }
    if let Some(dir) = config.log_dir.as_ref() {
        paths.push(dir.clone());
    }
    paths
}

fn system_read_only() -> Vec<PathBuf> {
    vec![
        PathBuf::from("."),
        PathBuf::from("/usr"),
        PathBuf::from("/lib"),
        PathBuf::from("/lib64"),
        PathBuf::from("/bin"),
        PathBuf::from("/sbin"),
    ]
}

fn finclaw_read_only(ctx: &ProfileContext) -> Vec<PathBuf> {
    let mut paths = system_read_only();
    paths.push(join_home(&ctx.home_dir, ".local/bin"));
    #[cfg(target_os = "macos")]
    paths.push(PathBuf::from("/opt/homebrew/bin"));
    paths.push(join_home(&ctx.home_dir, ".config"));
    paths
}

fn finclaw_read_write(ctx: &ProfileContext, config: &FindeskConfig) -> Vec<PathBuf> {
    let mut paths = findesk_base_read_write(ctx, config);
    paths.push(join_home(&ctx.home_dir, ".finclaw"));
    paths.push(join_home(&ctx.home_dir, ".config"));
    paths
}

fn hermes_read_only(ctx: &ProfileContext) -> Vec<PathBuf> {
    let mut paths = system_read_only();
    paths.push(join_home(&ctx.home_dir, ".config"));
    paths.push(join_home(&ctx.home_dir, ".hermes"));
    paths.push(join_home(&ctx.home_dir, ".local/share/uv"));
    paths
}

fn hermes_read_write(ctx: &ProfileContext, config: &FindeskConfig) -> Vec<PathBuf> {
    let mut paths = findesk_base_read_write(ctx, config);
    paths.push(join_home(&ctx.home_dir, ".config"));
    paths.push(join_home(&ctx.home_dir, ".hermes"));
    paths
}

fn openclaw_read_only(ctx: &ProfileContext) -> Vec<PathBuf> {
    let mut paths = system_read_only();
    paths.push(join_home(&ctx.home_dir, ".config"));
    paths.push(join_home(&ctx.home_dir, ".openclaw"));
    paths
}

fn openclaw_read_write(ctx: &ProfileContext, config: &FindeskConfig) -> Vec<PathBuf> {
    let mut paths = findesk_base_read_write(ctx, config);
    paths.push(join_home(&ctx.home_dir, ".config"));
    paths.push(join_home(&ctx.home_dir, ".openclaw"));
    paths
}

fn nanobot_read_only(ctx: &ProfileContext) -> Vec<PathBuf> {
    let mut paths = system_read_only();
    paths.push(join_home(&ctx.home_dir, ".config"));
    paths.push(join_home(&ctx.home_dir, ".nanobot"));
    paths
}

fn nanobot_read_write(ctx: &ProfileContext, config: &FindeskConfig) -> Vec<PathBuf> {
    let mut paths = findesk_base_read_write(ctx, config);
    paths.push(join_home(&ctx.home_dir, ".config"));
    paths.push(join_home(&ctx.home_dir, ".nanobot"));
    paths
}

fn npx_bun_acp_read_only(ctx: &ProfileContext) -> Vec<PathBuf> {
    let mut paths = system_read_only();
    paths.push(join_home(&ctx.home_dir, ".config"));
    paths.push(join_home(&ctx.home_dir, ".claude"));
    paths.push(join_home(&ctx.home_dir, ".codex"));
    paths.push(join_home(&ctx.home_dir, ".codebuddy"));
    paths
}

fn npx_bun_acp_read_write(ctx: &ProfileContext, config: &FindeskConfig) -> Vec<PathBuf> {
    let mut paths = findesk_base_read_write(ctx, config);
    paths.push(join_home(&ctx.home_dir, ".config"));
    paths.push(join_home(&ctx.home_dir, ".claude"));
    paths.push(join_home(&ctx.home_dir, ".codex"));
    paths.push(join_home(&ctx.home_dir, ".codebuddy"));
    paths
}

pub fn resolve_read_only_paths(ctx: &ProfileContext, _config: &FindeskConfig) -> Vec<PathBuf> {
    let profile = resolve_profile_id(&ctx.backend);
    let paths = match profile {
        "finclaw-interactive" => finclaw_read_only(ctx),
        "hermes" => hermes_read_only(ctx),
        "openclaw" => openclaw_read_only(ctx),
        "nanobot" => nanobot_read_only(ctx),
        "npx-bun-acp" => npx_bun_acp_read_only(ctx),
        _ => system_read_only(),
    };
    unique_paths(paths)
}

pub fn resolve_read_write_paths(ctx: &ProfileContext, config: &FindeskConfig) -> Vec<PathBuf> {
    let profile = resolve_profile_id(&ctx.backend);
    let paths = match profile {
        "finclaw-interactive" => finclaw_read_write(ctx, config),
        "hermes" => hermes_read_write(ctx, config),
        "openclaw" => openclaw_read_write(ctx, config),
        "nanobot" => nanobot_read_write(ctx, config),
        "npx-bun-acp" => npx_bun_acp_read_write(ctx, config),
        _ => findesk_base_read_write(ctx, config),
    };
    unique_paths(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_profile_id_maps_p1_backends() {
        assert_eq!(resolve_profile_id("hermes"), "hermes");
        assert_eq!(resolve_profile_id("claude"), "npx-bun-acp");
        assert_eq!(resolve_profile_id("openclaw-gateway"), "openclaw");
        assert_eq!(resolve_profile_id("nanobot"), "nanobot");
    }
}

pub fn macos_temp_paths() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let mut paths = vec![PathBuf::from("/private/var/folders")];
        if let Ok(tmpdir) = env::var("TMPDIR") {
            paths.push(PathBuf::from(tmpdir));
        }
        paths
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}
