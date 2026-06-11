use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct FinclawPortJson {
    pub claw_port: u16,
    pub shim_port: Option<u16>,
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

pub fn read_claw_port(profile: &str) -> Option<u16> {
    let path = resolve_port_json_path(profile);
    let raw = fs::read_to_string(path).ok()?;
    parse_port_json(&raw).map(|p| p.claw_port)
}
