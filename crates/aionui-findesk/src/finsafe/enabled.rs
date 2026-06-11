use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

static FINSAFE_ENABLED: OnceLock<Arc<AtomicBool>> = OnceLock::new();

fn flag() -> &'static Arc<AtomicBool> {
    FINSAFE_ENABLED.get_or_init(|| Arc::new(AtomicBool::new(default_enabled_for_platform())))
}

fn default_enabled_for_platform() -> bool {
    #[cfg(target_os = "windows")]
    {
        false
    }
    #[cfg(not(target_os = "windows"))]
    {
        true
    }
}

/// Shared flag wired to system settings and spawn policy.
pub fn shared_finsafe_enabled() -> Arc<AtomicBool> {
    Arc::clone(flag())
}

pub fn set_finsafe_enabled(enabled: bool) {
    flag().store(enabled, Ordering::Relaxed);
}

pub fn finsafe_enabled() -> bool {
    flag().load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_updates_flag() {
        set_finsafe_enabled(false);
        assert!(!finsafe_enabled());
        set_finsafe_enabled(true);
        assert!(finsafe_enabled());
    }
}
