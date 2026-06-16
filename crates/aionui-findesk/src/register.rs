use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use aionui_runtime::register_spawn_policy;
use tracing::info;

use crate::config::FindeskConfig;
use crate::finsafe::{FinsafeSpawnPolicy, set_finsafe_enabled, shared_finsafe_enabled};

/// Handles returned to the app layer for syncing persisted settings.
#[derive(Debug, Clone)]
pub struct FindeskHandles {
    pub finsafe_enabled: Arc<AtomicBool>,
}

/// Register FinSAFE spawn policy from process environment.
pub fn register_from_env() -> FindeskHandles {
    let config = FindeskConfig::from_env();
    let flag = shared_finsafe_enabled();
    register_spawn_policy(Box::new(FinsafeSpawnPolicy::new(config)));
    info!("[findesk] registered FinSAFE spawn policy");
    FindeskHandles { finsafe_enabled: flag }
}

/// Sync the in-memory flag from persisted backend settings.
pub fn sync_finsafe_enabled_from_settings(enabled: bool) {
    set_finsafe_enabled(enabled);
    shared_finsafe_enabled().store(enabled, Ordering::Relaxed);
}
