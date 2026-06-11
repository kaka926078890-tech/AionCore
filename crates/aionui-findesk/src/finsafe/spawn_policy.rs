use std::ffi::{OsStr, OsString};
use std::path::Path;

use aionui_runtime::{ResolvedSpawn, SpawnIntent, SpawnPolicy, SpawnWrapperMode};
use tracing::warn;

use crate::config::{FindeskConfig, path_exists};
use crate::finsafe::enabled::finsafe_enabled;
use crate::finsafe::policy::{pick_env_for_sandbox, resolve_runtime_policy_path};
use crate::finsafe::profiles::{self, ProfileContext};

#[derive(Debug, Clone)]
pub struct FinsafeSpawnPolicy {
    config: FindeskConfig,
}

impl FinsafeSpawnPolicy {
    pub fn new(config: FindeskConfig) -> Self {
        Self { config }
    }
}

fn program_basename(program: &OsStr) -> String {
    Path::new(program)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
        .trim_end_matches(".exe")
        .to_ascii_lowercase()
}

pub fn is_finclaw_executable(backend: Option<&str>, program: &OsStr) -> bool {
    if backend == Some("finclaw") {
        return true;
    }
    program_basename(program) == "finclaw"
}

fn should_wrap(intent: &SpawnIntent) -> bool {
    if !finsafe_enabled() {
        return false;
    }
    if intent.wrapper_mode == SpawnWrapperMode::InteractiveSelfConfine {
        return true;
    }
    if intent.wrapper_mode == SpawnWrapperMode::ShortLived {
        return intent
            .backend
            .as_deref()
            .is_some_and(|backend| profiles::resolve_profile_id(backend) != "finclaw-interactive");
    }
    is_finclaw_executable(intent.backend.as_deref(), &intent.program)
}

fn env_command_path() -> &'static str {
    if cfg!(target_os = "windows") {
        "cmd.exe"
    } else {
        "/usr/bin/env"
    }
}

impl SpawnPolicy for FinsafeSpawnPolicy {
    fn wrap(&self, intent: &SpawnIntent) -> ResolvedSpawn {
        if !should_wrap(intent) {
            return intent.resolve_unchanged();
        }

        let Some(finsafe_bin) = self.config.finsafe_bin.as_ref().filter(|p| path_exists(p)) else {
            warn!("[FinSAFE] finsafe binary not configured; spawning without wrapper");
            return intent.resolve_unchanged();
        };

        let ctx = ProfileContext::from_intent(
            intent.backend.as_deref(),
            Path::new(&intent.program),
            intent.cwd.as_deref(),
            &self.config,
        );

        let policy_path = match resolve_runtime_policy_path(&ctx, &self.config) {
            Ok(path) => path,
            Err(error) => {
                warn!(?error, "[FinSAFE] failed to write runtime policy; spawning without wrapper");
                return intent.resolve_unchanged();
            }
        };

        let finsafe_verb = if intent.wrapper_mode == SpawnWrapperMode::InteractiveSelfConfine
            || ctx.backend == "finclaw"
        {
            "self-confine"
        } else {
            "run"
        };

        let mut run_target: Vec<OsString> = vec![intent.program.clone()];
        run_target.extend(intent.args.iter().cloned());

        let mut finsafe_args = vec![
            OsString::from("--policy"),
            policy_path.into_os_string(),
            OsString::from(finsafe_verb),
        ];

        let env_pairs = pick_env_for_sandbox(&intent.child_env);
        if !env_pairs.is_empty() && !cfg!(target_os = "windows") {
            finsafe_args.push(OsString::from(env_command_path()));
            finsafe_args.extend(env_pairs.into_iter().map(OsString::from));
        }
        finsafe_args.extend(run_target);

        ResolvedSpawn {
            program: finsafe_bin.as_os_str().to_os_string(),
            args: finsafe_args,
            cwd: intent.cwd.clone(),
            child_env: intent.child_env.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::fs;
    use std::sync::atomic::Ordering;
    use tempfile::tempdir;

    use crate::finsafe::enabled::shared_finsafe_enabled;

    #[test]
    fn wraps_finclaw_serve_with_self_confine() {
        let dir = tempdir().unwrap();
        let finsafe = dir.path().join("finsafe");
        fs::write(&finsafe, b"").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&finsafe, fs::Permissions::from_mode(0o755)).unwrap();
        }

        shared_finsafe_enabled().store(true, Ordering::Relaxed);

        let policy = FinsafeSpawnPolicy::new(FindeskConfig {
            finsafe_bin: Some(finsafe),
            finsafe_policies_dir: None,
            finclaw_bin: None,
            cache_dir: Some(dir.path().join("cache")),
            work_dir: Some(dir.path().join("work")),
            log_dir: None,
        });

        let intent = SpawnIntent {
            backend: Some("finclaw".into()),
            program: OsString::from("/bin/finclaw"),
            args: vec![OsString::from("serve"), OsString::from("--profile"), OsString::from("default")],
            cwd: Some(dir.path().join("workspace")),
            child_env: HashMap::new(),
            wrapper_mode: SpawnWrapperMode::InteractiveSelfConfine,
        };

        let resolved = policy.wrap(&intent);
        assert_ne!(resolved.program, intent.program);
        assert_eq!(resolved.program.to_string_lossy(), dir.path().join("finsafe").to_string_lossy());
        assert!(resolved.args.iter().any(|a| a == "self-confine"));
    }

    #[test]
    fn wraps_short_lived_acp_backend() {
        let dir = tempdir().unwrap();
        let finsafe = dir.path().join("finsafe");
        fs::write(&finsafe, b"").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&finsafe, fs::Permissions::from_mode(0o755)).unwrap();
        }

        shared_finsafe_enabled().store(true, Ordering::Relaxed);

        let policy = FinsafeSpawnPolicy::new(FindeskConfig {
            finsafe_bin: Some(finsafe),
            finsafe_policies_dir: None,
            finclaw_bin: None,
            cache_dir: Some(dir.path().join("cache")),
            work_dir: Some(dir.path().join("work")),
            log_dir: None,
        });

        let intent = SpawnIntent {
            backend: Some("hermes".into()),
            program: OsString::from("/bin/hermes"),
            args: vec![OsString::from("--help")],
            cwd: Some(dir.path().join("workspace")),
            child_env: HashMap::new(),
            wrapper_mode: SpawnWrapperMode::ShortLived,
        };

        let resolved = policy.wrap(&intent);
        assert_ne!(resolved.program, intent.program);
        assert!(resolved.args.iter().any(|a| a == "run"));
    }

    #[test]
    fn noop_when_disabled() {
        shared_finsafe_enabled().store(false, Ordering::Relaxed);
        let policy = FinsafeSpawnPolicy::new(FindeskConfig::from_env());
        let intent = SpawnIntent {
            backend: Some("finclaw".into()),
            program: OsString::from("finclaw"),
            args: vec![],
            cwd: None,
            child_env: HashMap::new(),
            wrapper_mode: SpawnWrapperMode::InteractiveSelfConfine,
        };
        let resolved = policy.wrap(&intent);
        assert_eq!(resolved.program, intent.program);
    }
}
