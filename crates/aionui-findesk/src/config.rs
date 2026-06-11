use std::path::{Path, PathBuf};

/// Paths and flags parsed from the Desktop → AionCore env contract.
#[derive(Debug, Clone)]
pub struct FindeskConfig {
    pub finsafe_bin: Option<PathBuf>,
    pub finsafe_policies_dir: Option<PathBuf>,
    pub finclaw_bin: Option<PathBuf>,
    pub cache_dir: Option<PathBuf>,
    pub work_dir: Option<PathBuf>,
    pub log_dir: Option<PathBuf>,
}

impl FindeskConfig {
    pub fn from_env() -> Self {
        Self {
            finsafe_bin: env_path("AIONCORE_FINSAFE_BIN"),
            finsafe_policies_dir: env_path("AIONCORE_FINSAFE_POLICIES_DIR"),
            finclaw_bin: env_path("AIONCORE_FINCLAW_BIN"),
            cache_dir: env_path("AIONUI_CACHE_DIR"),
            work_dir: env_path("AIONUI_WORK_DIR"),
            log_dir: env_path("AIONUI_LOG_DIR"),
        }
    }

    pub fn is_findesk_enabled() -> bool {
        std::env::var("AIONCORE_FINDESK")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    }
}

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var(key)
        .ok()
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

pub fn path_exists(path: &Path) -> bool {
    path.exists()
}
