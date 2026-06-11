//! Extensible spawn policy chain for wrapping child processes before launch.
//!
//! Policies are registered at startup (e.g. FinSAFE in `aionui-findesk`) and
//! applied by [`crate::spawn::Builder`] immediately before `spawn()` / `output()`.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// How a spawn policy may wrap the target program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpawnWrapperMode {
    #[default]
    None,
    ShortLived,
    InteractiveSelfConfine,
}

/// Inputs collected from a [`crate::spawn::Builder`] before policies run.
#[derive(Debug, Clone)]
pub struct SpawnIntent {
    pub backend: Option<String>,
    pub program: OsString,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
    pub child_env: HashMap<String, String>,
    pub wrapper_mode: SpawnWrapperMode,
}

impl SpawnIntent {
    pub fn resolve_unchanged(&self) -> ResolvedSpawn {
        ResolvedSpawn {
            program: self.program.clone(),
            args: self.args.clone(),
            cwd: self.cwd.clone(),
            child_env: self.child_env.clone(),
        }
    }
}

/// Output of a single policy pass — becomes the next policy's input.
#[derive(Debug, Clone)]
pub struct ResolvedSpawn {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
    pub child_env: HashMap<String, String>,
}

/// Wrap or rewrite a spawn before the process is created.
pub trait SpawnPolicy: Send + Sync {
    fn wrap(&self, intent: &SpawnIntent) -> ResolvedSpawn;
}

struct NoopPolicy;

impl SpawnPolicy for NoopPolicy {
    fn wrap(&self, intent: &SpawnIntent) -> ResolvedSpawn {
        intent.resolve_unchanged()
    }
}

struct Registry {
    policies: Vec<Box<dyn SpawnPolicy>>,
}

static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();

fn registry() -> &'static Mutex<Registry> {
    REGISTRY.get_or_init(|| Mutex::new(Registry { policies: Vec::new() }))
}

/// Register a spawn policy. Policies run in registration order before the implicit noop pass.
pub fn register_spawn_policy(policy: Box<dyn SpawnPolicy>) {
    let mut guard = registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.policies.push(policy);
}

/// Apply all registered policies to `intent`.
pub fn apply_spawn_policies(intent: &SpawnIntent) -> ResolvedSpawn {
    let guard = registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let mut current = intent.clone();
    for policy in &guard.policies {
        let resolved = policy.wrap(&current);
        current = SpawnIntent {
            backend: current.backend.clone(),
            program: resolved.program,
            args: resolved.args,
            cwd: resolved.cwd.or(current.cwd),
            child_env: resolved.child_env,
            wrapper_mode: current.wrapper_mode,
        };
    }

    let noop = NoopPolicy;
    noop.wrap(&current)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct PrefixArgPolicy {
        marker: &'static str,
    }

    impl SpawnPolicy for PrefixArgPolicy {
        fn wrap(&self, intent: &SpawnIntent) -> ResolvedSpawn {
            let mut args = vec![OsString::from(self.marker)];
            args.extend(intent.args.iter().cloned());
            ResolvedSpawn {
                program: intent.program.clone(),
                args,
                cwd: intent.cwd.clone(),
                child_env: intent.child_env.clone(),
            }
        }
    }

    fn sample_intent() -> SpawnIntent {
        SpawnIntent {
            backend: None,
            program: OsString::from("echo"),
            args: vec![OsString::from("hello")],
            cwd: None,
            child_env: HashMap::new(),
            wrapper_mode: SpawnWrapperMode::None,
        }
    }

    #[test]
    fn noop_policy_leaves_intent_unchanged() {
        let intent = sample_intent();
        let resolved = apply_spawn_policies(&intent);
        assert_eq!(resolved.program, OsString::from("echo"));
        assert_eq!(resolved.args, vec![OsString::from("hello")]);
    }

    #[test]
    fn prefix_policy_inserts_marker_arg() {
        let policy = PrefixArgPolicy { marker: "wrapped" };
        let resolved = policy.wrap(&sample_intent());
        assert_eq!(
            resolved.args,
            vec![OsString::from("wrapped"), OsString::from("hello")]
        );
    }

    #[test]
    fn register_spawn_policy_runs_before_implicit_noop() {
        register_spawn_policy(Box::new(PrefixArgPolicy { marker: "policy" }));
        let resolved = apply_spawn_policies(&sample_intent());
        assert_eq!(resolved.args.first().map(|s| s.to_string_lossy().into_owned()), Some("policy".into()));
    }
}
