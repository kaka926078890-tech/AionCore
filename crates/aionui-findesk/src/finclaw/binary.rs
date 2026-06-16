use std::path::PathBuf;

use crate::config::FindeskConfig;

fn same_executable(left: &PathBuf, right: &PathBuf) -> bool {
    left.canonicalize()
        .ok()
        .zip(right.canonicalize().ok())
        .is_some_and(|(a, b)| a == b)
}

/// Resolve bundled or PATH `finclaw` binary (Desktop injects `AIONCORE_FINCLAW_BIN`).
///
/// User-supplied `override_path` is accepted only when it resolves to the same
/// executable as `AIONCORE_FINCLAW_BIN`.
pub fn resolve_finclaw_binary(config: &FindeskConfig, override_path: Option<&str>) -> Option<PathBuf> {
    let configured = config.finclaw_bin.as_ref().filter(|p| p.is_file());

    if let Some(path) = override_path.filter(|p| !p.is_empty()) {
        let candidate = PathBuf::from(path);
        if candidate.is_file() {
            if let Some(configured) = configured {
                if same_executable(&candidate, configured) {
                    return Some(candidate);
                }
            }
            return None;
        }
    }

    if let Some(configured) = configured {
        return Some(configured.clone());
    }

    which::which("finclaw").ok()
}
