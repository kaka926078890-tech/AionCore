use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use aionui_runtime::{Builder, SpawnWrapperMode};
use tokio::process::Child;
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::config::FindeskConfig;
use crate::finclaw::binary::resolve_finclaw_binary;
use crate::finclaw::port_json::{probe_claw_health, probe_claw_real_llm_ready, read_claw_port, stop_profile_daemon};
use crate::finclaw::serve_config::{
    prepare_finclaw_workspace_skills_config, workspace_has_linked_skills,
};
use crate::finsafe::finsafe_enabled;

#[derive(Debug, Clone)]
pub struct FinclawGatewayConfig {
    pub cli_path: Option<String>,
    pub profile: String,
    pub security_mode: Option<String>,
    pub serve_cwd: PathBuf,
    /// Env overrides for `finclaw serve` (FinDesk model → FINCLAW_LLM_*).
    pub llm_serve_env: HashMap<String, String>,
    /// When set, pooled gateways restart if the fingerprint changes.
    pub model_fingerprint: Option<String>,
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
        let needs_skills_overlay = workspace_has_linked_skills(&self.config.serve_cwd);

        // Sync profile config + overlay before any reuse decision.
        let _ = prepare_finclaw_workspace_skills_config(&self.config.serve_cwd, &self.config.profile);

        // Workspace-linked auto-inject skills live under `{workspace}/.finclaw/skills`
        // and are wired into the profile via `skills.external_dirs`. Recycle any
        // existing serve process so embedded supervisor rescans with the updated
        // profile config (overlay alone is not always honored on reuse paths).
        if needs_skills_overlay {
            if let Some(mut child) = self.child.lock().await.take() {
                let _ = child.start_kill();
            }
            *self.claw_port.lock().await = None;
            stop_profile_daemon(&self.config.profile).await;
        } else if let Some(port) = *self.claw_port.lock().await {
            if probe_claw_health(port).await && probe_claw_real_llm_ready(port).await {
                return Ok(port);
            }
            warn!(port, "cached finclaw claw_port is unhealthy or mock-only; respawning");
            *self.claw_port.lock().await = None;
            if let Some(mut child) = self.child.lock().await.take() {
                let _ = child.start_kill();
            }
        }

        if !needs_skills_overlay
            && let Some(port) = read_claw_port(&self.config.profile)
        {
            if probe_claw_health(port).await && probe_claw_real_llm_ready(port).await {
                info!(
                    profile = %self.config.profile,
                    port,
                    "reusing healthy finclaw serve daemon"
                );
                *self.claw_port.lock().await = Some(port);
                return Ok(port);
            }
            if probe_claw_health(port).await {
                warn!(
                    profile = %self.config.profile,
                    port,
                    "finclaw daemon is mock-only; restarting with current profile config"
                );
                stop_profile_daemon(&self.config.profile).await;
            }
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
        if let Some(config_path) = prepare_finclaw_workspace_skills_config(&self.config.serve_cwd, &self.config.profile) {
            builder.arg("--config");
            builder.arg(config_path);
        }
        for (key, value) in &self.config.llm_serve_env {
            builder.env(key, value);
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
        self.claw_port.try_lock().ok().and_then(|guard| *guard)
    }

    async fn wait_for_port(&self) -> Result<u16, String> {
        for _ in 0..120 {
            if let Some(port) = read_claw_port(&self.config.profile)
                && probe_claw_health(port).await
                && probe_claw_real_llm_ready(port).await
            {
                return Ok(port);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        Err("timed out waiting for finclaw serve with a non-mock LLM provider; configure finclaw models".into())
    }

    pub async fn shutdown(&self) {
        if let Some(mut child) = self.child.lock().await.take()
            && let Err(error) = child.start_kill()
        {
            warn!(?error, "finclaw serve kill failed");
        }
        // Pooled gateways often reuse an existing profile daemon via port.json; killing only
        // our tracked child is not enough for tool-policy switches to take effect.
        stop_profile_daemon(&self.config.profile).await;
        *self.claw_port.lock().await = None;
    }
}
