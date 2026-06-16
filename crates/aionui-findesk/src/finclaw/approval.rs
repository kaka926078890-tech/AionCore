use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Parsed approval payload from FinClaw infer SSE events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinclawApprovalRequired {
    pub approval_request_id: String,
    pub tool_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinclawApprovalDecision {
    Allow,
    Deny,
}

impl FinclawApprovalDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
        }
    }
}

/// Parse approval fields from a FinClaw SSE JSON payload.
pub fn parse_approval_required(parsed: &Value) -> Option<FinclawApprovalRequired> {
    let mut source = parsed.clone();
    if let Some(value) = parsed.get("value") {
        if value.is_object() {
            source = value.clone();
        } else if let Some(text) = value.as_str()
            && let Ok(decoded) = serde_json::from_str::<Value>(text)
            && decoded.is_object()
        {
            source = decoded;
        }
    }

    if let Some(metadata) = source.get("metadata").and_then(Value::as_object).cloned() {
        for (key, value) in metadata {
            source[key] = value;
        }
    }

    let approval_request_id = source
        .get("approval_request_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())?;
    let tool_name = source
        .get("tool_name")
        .or_else(|| source.get("tool"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())?;

    let tool_call_id = source
        .get("tool_call_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string);

    Some(FinclawApprovalRequired {
        approval_request_id: approval_request_id.to_string(),
        tool_name: tool_name.to_string(),
        tool_call_id,
        arguments: source.get("arguments").cloned(),
        reason: source.get("reason").and_then(Value::as_str).map(str::to_string),
        run_id: source
            .get("run_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string),
    })
}

/// Submit an approval decision to the loopback FinClaw serve daemon.
pub async fn submit_approval_resolve(
    client: &Client,
    claw_port: u16,
    user_id: &str,
    session_id: &str,
    approval: &FinclawApprovalRequired,
    decision: FinclawApprovalDecision,
    reason: Option<&str>,
) -> Result<(), String> {
    let url = format!("http://127.0.0.1:{claw_port}/ai/approval/resolve");
    let mut body = json!({
        "approval_request_id": approval.approval_request_id,
        "decision": decision.as_str(),
        "user_id": user_id,
        "session_id": session_id,
        "tool_name": approval.tool_name,
    });
    if let Some(tool_call_id) = approval.tool_call_id.as_deref() {
        body["tool_call_id"] = json!(tool_call_id);
    }
    if let Some(args) = &approval.arguments {
        body["arguments"] = args.clone();
    }
    if let Some(run_id) = approval.run_id.as_deref() {
        body["run_id"] = json!(run_id);
    }
    if let Some(reason) = reason.filter(|r| !r.is_empty()) {
        body["reason"] = json!(reason);
    }

    let response = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("X-User-ID", user_id)
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.status().is_success() {
        Ok(())
    } else {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        Err(if text.is_empty() {
            format!("FinClaw approval resolve failed: {status}")
        } else {
            text
        })
    }
}

/// Map a UI confirmation payload to an approval decision.
pub fn decision_from_confirm_data(data: &Value) -> FinclawApprovalDecision {
    let raw = data
        .get("value")
        .or_else(|| data.get("option_id"))
        .and_then(Value::as_str)
        .unwrap_or("deny")
        .to_ascii_lowercase();

    if matches!(
        raw.as_str(),
        "allow" | "allow_once" | "allow_always" | "approve" | "yes"
    ) {
        FinclawApprovalDecision::Allow
    } else {
        FinclawApprovalDecision::Deny
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_approval_required_from_custom_event() {
        let parsed = json!({
            "type": "CUSTOM",
            "name": "approval.required",
            "value": {
                "approval_request_id": "apr-1",
                "tool_name": "bash",
                "tool_call_id": "tc-1",
                "run_id": "run-1"
            }
        });
        let approval = parse_approval_required(&parsed).expect("approval");
        assert_eq!(approval.approval_request_id, "apr-1");
        assert_eq!(approval.tool_name, "bash");
        assert_eq!(approval.tool_call_id.as_deref(), Some("tc-1"));
    }

    #[test]
    fn parse_approval_required_merges_metadata() {
        let parsed = json!({
            "type": "approval_required",
            "metadata": {
                "approval_request_id": "apr-2",
                "tool_name": "write_file"
            }
        });
        let approval = parse_approval_required(&parsed).expect("approval");
        assert_eq!(approval.approval_request_id, "apr-2");
        assert_eq!(approval.tool_name, "write_file");
    }

    #[test]
    fn decision_from_confirm_data_maps_allow_variants() {
        assert_eq!(
            decision_from_confirm_data(&json!({"value": "allow_once"})),
            FinclawApprovalDecision::Allow
        );
        assert_eq!(
            decision_from_confirm_data(&json!({"value": "reject_once"})),
            FinclawApprovalDecision::Deny
        );
    }
}
