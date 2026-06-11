use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use aionui_runtime::{Builder, SpawnWrapperMode};
use tokio::process::Child;
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::config::FindeskConfig;
use crate::finsafe::finsafe_enabled;
use crate::finclaw::binary::resolve_finclaw_binary;
use crate::finclaw::port_json::read_claw_port;

#[derive(Debug, Clone)]
pub struct FinclawGatewayConfig {
    pub cli_path: Option<String>,
    pub profile: String,
    pub security_mode: Option<String>,
    pub serve_cwd: PathBuf,
}

pub struct FinclawGateway {
    config: FinclawGatewayConfig,
    findesk: FindeskConfig,
    child: Arc<Mutex<Option<Child>>>,
    claw_port: Arc<Mutex<Option<u16>>>,
}

impl FinclawGateway {
    pub fn new(findesk: FindeskConfig, config: FinclawGatewayConfig) -> Self {
        Self {
            config,
            findesk,
            child: Arc::new(Mutex::new(None)),
            claw_port: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn ensure_started(&self) -> Result<u16, String> {
        if let Some(port) = *self.claw_port.lock().await {
            return Ok(port);
        }
        if let Some(port) = read_claw_port(&self.config.profile) {
            *self.claw_port.lock().await = Some(port);
            return Ok(port);
        }

        let binary = resolve_finclaw_binary(&self.findesk, self.config.cli_path.as_deref())
            .ok_or_else(|| "finclaw binary not found (set AIONCORE_FINCLAW_BIN)".to_string())?;

        let mut args = vec![
            "serve".to_string(),
            "--profile".to_string(),
            self.config.profile.clone(),
        ];
        if !finsafe_enabled()
            && let Some(mode) = self.config.security_mode.as_deref().filter(|m| !m.is_empty())
        {
            args.push("--security".to_string());
            args.push(mode.to_string());
        }

        let mut builder = Builder::new(&binary);
        builder
            .backend("finclaw")
            .spawn_wrapper_mode(SpawnWrapperMode::InteractiveSelfConfine)
            .current_dir(&self.config.serve_cwd);
        for arg in &args {
            builder.arg(arg);
        }

        let child = builder
            .spawn()
            .map_err(|e| format!("failed to spawn finclaw serve: {e}"))?;

        info!(
            profile = %self.config.profile,
            cwd = %self.config.serve_cwd.display(),
            "started finclaw serve"
        );

        let port = self.wait_for_port().await?;
        *self.child.lock().await = Some(child);
        *self.claw_port.lock().await = Some(port);
        Ok(port)
    }

    pub fn claw_port(&self) -> Option<u16> {
        self.claw_port
            .try_lock()
            .ok()
            .and_then(|guard| *guard)
    }

    async fn wait_for_port(&self) -> Result<u16, String> {
        for _ in 0..120 {
            if let Some(port) = read_claw_port(&self.config.profile) {
                return Ok(port);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        Err("timed out waiting for finclaw port.json".into())
    }

    pub async fn shutdown(&self) {
        if let Some(mut child) = self.child.lock().await.take() {
            if let Err(error) = child.start_kill() {
                warn!(?error, "finclaw serve kill failed");
            }
        }
        *self.claw_port.lock().await = None;
    }
}
