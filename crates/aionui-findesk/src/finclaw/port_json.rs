use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use reqwest::Client;
use serde::Deserialize;
use tracing::warn;

#[derive(Debug, Clone, Deserialize)]
pub struct FinclawPortJson {
    pub claw_port: u16,
    pub shim_port: Option<u16>,
    #[serde(default)]
    pub pid: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct ClawHealthResponse {
    capabilities: Option<ClawHealthCapabilities>,
    providers: Option<HashMap<String, bool>>,
}

#[derive(Debug, Deserialize)]
struct ClawHealthCapabilities {
    llm: Option<String>,
}

pub fn resolve_port_json_path(profile: &str) -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".finclaw")
        .join("profiles")
        .join(profile)
        .join("run")
        .join("port.json")
}

pub fn parse_port_json(raw: &str) -> Option<FinclawPortJson> {
    serde_json::from_str::<FinclawPortJson>(raw).ok().filter(|p| p.claw_port > 0)
}

pub fn read_port_json(profile: &str) -> Option<FinclawPortJson> {
    let path = resolve_port_json_path(profile);
    let raw = fs::read_to_string(path).ok()?;
    parse_port_json(&raw)
}

pub fn read_claw_port(profile: &str) -> Option<u16> {
    read_port_json(profile).map(|p| p.claw_port)
}

/// Returns true when Claw answers `/health` on the loopback port (matches finclaw CLI `find_daemon`).
pub async fn probe_claw_health(port: u16) -> bool {
    fetch_claw_health(port).await.is_some()
}

/// True when the daemon is healthy and has at least one non-mock LLM provider enabled.
pub async fn probe_claw_real_llm_ready(port: u16) -> bool {
    let Some(health) = fetch_claw_health(port).await else {
        return false;
    };
    if health
        .capabilities
        .and_then(|capabilities| capabilities.llm)
        .is_some_and(|llm| llm == "mock")
    {
        return false;
    }
    match health.providers {
        Some(providers) => providers
            .iter()
            .any(|(name, enabled)| *enabled && name != "mock"),
        None => true,
    }
}

async fn fetch_claw_health(port: u16) -> Option<ClawHealthResponse> {
    let url = format!("http://127.0.0.1:{port}/health");
    let response = Client::new()
        .get(url)
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.json::<ClawHealthResponse>().await.ok()
}

/// Stop a profile-scoped `finclaw serve` daemon recorded in `port.json`.
pub async fn stop_profile_daemon(profile: &str) {
    let Some(port_json) = read_port_json(profile) else {
        return;
    };
    if let Some(pid) = port_json.pid.filter(|pid| *pid > 0) {
        terminate_pid(pid);
    }
    for _ in 0..20 {
        if !probe_claw_health(port_json.claw_port).await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    warn!(
        profile,
        port = port_json.claw_port,
        "finclaw daemon still healthy after stop request"
    );
}

#[cfg(unix)]
fn terminate_pid(pid: u32) {
    use std::process::Command;
    let _ = Command::new("kill").arg("-TERM").arg(pid.to_string()).status();
}

#[cfg(not(unix))]
fn terminate_pid(_pid: u32) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_port_json_with_pid() {
        let raw = r#"{"pid":42,"claw_port":8080,"shim_port":8081}"#;
        let parsed = parse_port_json(raw).expect("port json");
        assert_eq!(parsed.pid, Some(42));
        assert_eq!(parsed.claw_port, 8080);
    }

    #[test]
    fn real_llm_ready_when_non_mock_provider_enabled() {
        let raw = r#"{"capabilities":{"llm":"real"},"providers":{"deepseek":true,"mock":true}}"#;
        let health: ClawHealthResponse = serde_json::from_str(raw).unwrap();
        assert!(health_has_real_llm_provider(&health));
    }

    #[test]
    fn mock_only_provider_is_not_ready() {
        let raw = r#"{"capabilities":{"llm":"real"},"providers":{"mock":true}}"#;
        let health: ClawHealthResponse = serde_json::from_str(raw).unwrap();
        assert!(!health_has_real_llm_provider(&health));
    }

    fn health_has_real_llm_provider(health: &ClawHealthResponse) -> bool {
        if health
            .capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.llm.as_deref())
            == Some("mock")
        {
            return false;
        }
        match &health.providers {
            Some(providers) => providers
                .iter()
                .any(|(name, enabled)| *enabled && name != "mock"),
            None => true,
        }
    }
}
