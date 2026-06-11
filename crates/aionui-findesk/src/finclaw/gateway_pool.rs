use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use tokio::sync::Mutex;

use crate::config::FindeskConfig;
use crate::finclaw::gateway::{FinclawGateway, FinclawGatewayConfig};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PoolKey {
    profile: String,
    cwd: PathBuf,
}

struct PoolEntry {
    gateway: Arc<FinclawGateway>,
    ref_count: usize,
}

/// Profile + workspace keyed pool sharing [`FinclawGateway`] instances.
pub struct FinclawGatewayPool {
    findesk: FindeskConfig,
    entries: Mutex<HashMap<PoolKey, PoolEntry>>,
}

impl FinclawGatewayPool {
    pub fn new(findesk: FindeskConfig) -> Self {
        Self {
            findesk,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Acquire or create a gateway for `config`. Returns the shared gateway and
    /// the ref count for this pool key (used by tool-policy UI).
    pub async fn acquire(&self, config: FinclawGatewayConfig) -> (Arc<FinclawGateway>, usize) {
        let key = pool_key(&config.profile, &config.serve_cwd);
        let mut entries = self.entries.lock().await;
        let entry = entries.entry(key).or_insert_with(|| PoolEntry {
            gateway: Arc::new(FinclawGateway::new(self.findesk.clone(), config.clone())),
            ref_count: 0,
        });
        entry.ref_count += 1;
        let count = entry.ref_count;
        (Arc::clone(&entry.gateway), count)
    }

    /// Release one holder. When the last holder drops, the entry is removed.
    pub async fn release(&self, profile: &str, serve_cwd: &Path) {
        let key = pool_key(profile, serve_cwd);
        let mut entries = self.entries.lock().await;
        if let Some(entry) = entries.get_mut(&key) {
            if entry.ref_count > 0 {
                entry.ref_count -= 1;
            }
            if entry.ref_count == 0 {
                let gateway = Arc::clone(&entry.gateway);
                entries.remove(&key);
                drop(entries);
                gateway.shutdown().await;
            }
        }
    }

    pub async fn ref_count_for(&self, profile: &str, serve_cwd: &Path) -> usize {
        let key = pool_key(profile, serve_cwd);
        self.entries
            .lock()
            .await
            .get(&key)
            .map(|entry| entry.ref_count)
            .unwrap_or(0)
    }

    pub async fn total_ref_count(&self) -> usize {
        self.entries
            .lock()
            .await
            .values()
            .map(|entry| entry.ref_count)
            .sum()
    }

    /// Shut down the pooled gateway for `profile` + `serve_cwd` so the next acquire respawns with fresh config.
    /// Returns `(restarted, ref_count_before_restart)`.
    pub async fn restart_gateway(&self, profile: &str, serve_cwd: &Path) -> (bool, usize) {
        let key = pool_key(profile, serve_cwd);
        let mut entries = self.entries.lock().await;
        let Some(entry) = entries.remove(&key) else {
            return (false, 0);
        };
        let ref_count = entry.ref_count;
        let gateway = Arc::clone(&entry.gateway);
        drop(entries);
        gateway.shutdown().await;
        (true, ref_count)
    }
}

static SHARED_POOL: OnceLock<FinclawGatewayPool> = OnceLock::new();

pub fn shared_gateway_pool() -> &'static FinclawGatewayPool {
    SHARED_POOL.get_or_init(|| FinclawGatewayPool::new(FindeskConfig::from_env()))
}

fn pool_key(profile: &str, serve_cwd: &Path) -> PoolKey {
    let cwd = serve_cwd
        .canonicalize()
        .unwrap_or_else(|_| serve_cwd.to_path_buf());
    PoolKey {
        profile: profile.to_string(),
        cwd,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn acquire_increments_ref_count_per_key() {
        let pool = FinclawGatewayPool::new(FindeskConfig::from_env());
        let dir = tempdir().unwrap();
        let config = FinclawGatewayConfig {
            cli_path: None,
            profile: "default".into(),
            security_mode: None,
            serve_cwd: dir.path().to_path_buf(),
        };

        let (_, count1) = pool.acquire(config.clone()).await;
        assert_eq!(count1, 1);

        let (_, count2) = pool.acquire(config.clone()).await;
        assert_eq!(count2, 2);
        assert_eq!(pool.ref_count_for("default", dir.path()).await, 2);

        pool.release("default", dir.path()).await;
        assert_eq!(pool.ref_count_for("default", dir.path()).await, 1);

        pool.release("default", dir.path()).await;
        assert_eq!(pool.ref_count_for("default", dir.path()).await, 0);
    }

    #[tokio::test]
    async fn restart_gateway_removes_entry_and_zeros_ref_count() {
        let pool = FinclawGatewayPool::new(FindeskConfig::from_env());
        let dir = tempdir().unwrap();
        let config = FinclawGatewayConfig {
            cli_path: None,
            profile: "default".into(),
            security_mode: None,
            serve_cwd: dir.path().to_path_buf(),
        };

        pool.acquire(config).await;
        assert_eq!(pool.ref_count_for("default", dir.path()).await, 1);

        let (restarted, refs) = pool.restart_gateway("default", dir.path()).await;
        assert!(restarted);
        assert_eq!(refs, 1);
        assert_eq!(pool.ref_count_for("default", dir.path()).await, 0);
    }

    #[tokio::test]
    async fn different_cwd_gets_separate_entries() {
        let pool = FinclawGatewayPool::new(FindeskConfig::from_env());
        let dir_a = tempdir().unwrap();
        let dir_b = tempdir().unwrap();

        pool.acquire(FinclawGatewayConfig {
            cli_path: None,
            profile: "default".into(),
            security_mode: None,
            serve_cwd: dir_a.path().to_path_buf(),
        })
        .await;
        pool.acquire(FinclawGatewayConfig {
            cli_path: None,
            profile: "default".into(),
            security_mode: None,
            serve_cwd: dir_b.path().to_path_buf(),
        })
        .await;

        assert_eq!(pool.ref_count_for("default", dir_a.path()).await, 1);
        assert_eq!(pool.ref_count_for("default", dir_b.path()).await, 1);
        assert_eq!(pool.total_ref_count().await, 2);
    }
}
