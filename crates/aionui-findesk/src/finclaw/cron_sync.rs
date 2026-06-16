use std::path::Path;
use std::process::Output;

use aionui_runtime::Builder;
use serde::{Deserialize, Serialize};

use crate::config::FindeskConfig;
use crate::finclaw::binary::resolve_finclaw_binary;

/// One cron entry returned by `finclaw cron list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinclawCronEntry {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub schedule: Option<String>,
    #[serde(default)]
    pub expression: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// Map a native FinClaw cron entry to an AionCore cron job name (sync stub).
pub fn map_cron_entry_to_job_name(entry: &FinclawCronEntry, profile: &str) -> String {
    format!("finclaw:{profile}:{}", entry.id)
}

/// Map cron schedule expression for AionCore jobs table (sync stub).
pub fn map_cron_expression(entry: &FinclawCronEntry) -> Option<String> {
    entry
        .expression
        .clone()
        .or_else(|| entry.schedule.clone())
        .filter(|value| !value.trim().is_empty())
}

/// List FinClaw cron jobs via CLI (`finclaw cron list --profile <p> --json`).
pub async fn list_finclaw_cron(
    findesk: &FindeskConfig,
    cli_path: Option<&str>,
    profile: &str,
) -> Result<Vec<FinclawCronEntry>, String> {
    let binary = resolve_finclaw_binary(findesk, cli_path)
        .ok_or_else(|| "finclaw binary not found (set AIONCORE_FINCLAW_BIN)".to_string())?;

    let mut builder = Builder::clean_cli(&binary);
    builder.backend("finclaw");
    builder.arg("cron");
    builder.arg("list");
    builder.arg("--profile");
    builder.arg(profile);
    builder.arg("--json");
    let output = builder
        .output()
        .await
        .map_err(|e| format!("failed to run finclaw cron list: {e}"))?;

    parse_cron_list_output(&output)
}

/// Mirror FinClaw native cron into AionCore jobs (body stubbed for follow-up wiring).
pub async fn sync_finclaw_runtime_cron(
    findesk: &FindeskConfig,
    cli_path: Option<&str>,
    profile: &str,
    _workspace: &Path,
) -> Result<Vec<String>, String> {
    let entries = list_finclaw_cron(findesk, cli_path, profile).await?;
    Ok(entries
        .iter()
        .map(|entry| map_cron_entry_to_job_name(entry, profile))
        .collect())
}

fn parse_cron_list_output(output: &Output) -> Result<Vec<FinclawCronEntry>, String> {
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(if stderr.trim().is_empty() {
            format!("finclaw cron list failed with status {}", output.status)
        } else {
            stderr.into_owned()
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    serde_json::from_str(trimmed).map_err(|e| format!("failed to parse finclaw cron list JSON: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_cron_entry_to_job_name_includes_profile() {
        let entry = FinclawCronEntry {
            id: "job-1".into(),
            name: Some("daily".into()),
            schedule: None,
            expression: Some("0 9 * * *".into()),
            prompt: None,
            enabled: Some(true),
        };
        assert_eq!(map_cron_entry_to_job_name(&entry, "default"), "finclaw:default:job-1");
    }

    #[test]
    fn map_cron_expression_prefers_expression_field() {
        let entry = FinclawCronEntry {
            id: "job-1".into(),
            name: None,
            schedule: Some("every 1h".into()),
            expression: Some("0 * * * *".into()),
            prompt: None,
            enabled: None,
        };
        assert_eq!(map_cron_expression(&entry).as_deref(), Some("0 * * * *"));
    }

    #[test]
    fn parse_cron_list_output_reads_json_array() {
        let output = Output {
            status: std::process::ExitStatus::default(),
            stdout: br#"[{"id":"a","expression":"0 0 * * *"}]"#.to_vec(),
            stderr: Vec::new(),
        };
        let entries = parse_cron_list_output(&output).expect("entries");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "a");
    }
}
