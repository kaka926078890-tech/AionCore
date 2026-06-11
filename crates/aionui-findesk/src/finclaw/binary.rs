use std::path::PathBuf;

use crate::config::FindeskConfig;

/// Resolve bundled or PATH `finclaw` binary (Desktop injects `AIONCORE_FINCLAW_BIN`).
pub fn resolve_finclaw_binary(config: &FindeskConfig, override_path: Option<&str>) -> Option<PathBuf> {
    if let Some(path) = override_path.filter(|p| !p.is_empty()) {
        let candidate = PathBuf::from(path);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    if let Some(path) = config.finclaw_bin.as_ref().filter(|p| p.is_file()) {
        return Some(path.clone());
    }
    which::which("finclaw").ok()
}
