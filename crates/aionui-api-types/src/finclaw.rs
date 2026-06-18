use serde::{Deserialize, Serialize};

/// One local Claw profile exposed to the FinSkills hub client WebView.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FinclawHostClaw {
    pub id: String,
    pub name: String,
    pub skills_dir: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_dir: Option<String>,
}

/// Authenticated Findesk user passed to the hub client for gateway_trust SSO.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FinclawHostUser {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FinclawHostContext {
    pub source: String,
    pub claws: Vec<FinclawHostClaw>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<FinclawHostUser>,
}

/// Snapshot for FinClaw tool-policy UI (active gateway pool usage).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FinclawToolPolicySnapshot {
    pub pool_ref_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FinclawGetToolPolicyQuery {
    pub workspace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// When set, the UI is switching policy from an active conversation; count at least this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FinclawToolPolicyResponse {
    pub tool_policy: String,
    pub pool_ref_count: usize,
    pub profile: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FinclawApplyToolPolicyRequest {
    pub workspace: String,
    pub tool_policy: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FinclawApplyToolPolicyResponse {
    pub tool_policy: String,
    pub pool_ref_count: usize,
    pub profile: String,
    pub restarted: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_context_serializes_with_source() {
        let ctx = FinclawHostContext {
            source: "finclaw".into(),
            claws: vec![FinclawHostClaw {
                id: "default".into(),
                name: "FinClaw".into(),
                skills_dir: "/home/user/.finclaw/profiles/default/skills".into(),
                home_dir: Some("/home/user/.finclaw/profiles/default".into()),
            }],
            user: None,
        };
        let json = serde_json::to_value(&ctx).unwrap();
        assert_eq!(json["source"], "finclaw");
        assert_eq!(json["claws"][0]["id"], "default");
    }
}
