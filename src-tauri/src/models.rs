use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Default)]
pub struct RawCacheCreation {
    #[serde(default)]
    pub ephemeral_1h_input_tokens: i64,
    #[serde(default)]
    pub ephemeral_5m_input_tokens: i64,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct RawUsage {
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
    #[serde(default)]
    pub cache_creation_input_tokens: i64,
    #[serde(default)]
    pub cache_read_input_tokens: i64,
    #[serde(default)]
    pub cache_creation: RawCacheCreation,
    #[serde(default)]
    pub service_tier: Option<String>,
    #[serde(default)]
    pub speed: Option<String>,
    #[serde(default)]
    pub server_tool_use: RawServerToolUse,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct RawServerToolUse {
    #[serde(default)]
    pub web_search_requests: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MessageRecord {
    pub session_id: String,
    pub project: String,
    pub ts: String,
    pub role: Option<String>,
    pub model: Option<String>,
    pub version: Option<String>,
    pub git_branch: Option<String>,
    pub service_tier: Option<String>,
    pub stop_reason: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation: i64,
    pub cache_read: i64,
    pub ephemeral_1h: i64,
    pub ephemeral_5m: i64,
    /// The API call's id; a call's lines share it and its usage is counted once.
    pub message_id: Option<String>,
    /// Set on a sub-agent's records (`agentId`).
    pub agent_id: Option<String>,
    pub web_searches: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolCallRecord {
    pub session_id: String,
    pub project: String,
    pub ts: String,
    pub tool_use_id: Option<String>,
    pub name: String,
    pub is_mcp: bool,
    pub user_modified: Option<bool>,
    /// A category decided from the tool input in memory ("test", "read"); the input
    /// itself is never stored.
    pub tag: Option<String>,
    /// For a test run only: how many tests its result reported failing (read in
    /// memory from the output, which is not stored). -1 = failed, count unknown.
    pub fail_count: Option<i64>,
    /// For a test run only: which test command it was, as a hash of the
    /// normalised command (parser::test_command_key). The text is not kept.
    pub test_key: Option<String>,
}

/// An inbound prompt: where one work segment starts. Only its kind is kept.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PromptRecord {
    pub session_id: String,
    pub agent_id: Option<String>,
    pub ts: String,
    /// human | compact | resume | task-notification
    pub kind: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedFile {
    pub messages: Vec<MessageRecord>,
    pub tool_calls: Vec<ToolCallRecord>,
    pub titles: Vec<(String, String)>,
    pub prompts: Vec<PromptRecord>,
    /// Claude Code's own running cost per session (its last `cost-state` record).
    pub cost_states: Vec<CostStateRecord>,
}

/// A `cost-state` snapshot covers the calls of the process that wrote it: from the
/// process start (`startTime`) to the last record written before the snapshot.
/// A resumed session's file also holds earlier processes' calls, outside it.
#[derive(Debug, Clone, PartialEq)]
pub struct CostStateRecord {
    pub session_id: String,
    pub total_usd: f64,
    /// ISO time of `startTime`; empty when the record has none (= from the start).
    pub from_ts: String,
    pub to_ts: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_deserializes_with_missing_fields() {
        let u: RawUsage = serde_json::from_str(r#"{"input_tokens": 10}"#).unwrap();
        assert_eq!(u.input_tokens, 10);
        assert_eq!(u.output_tokens, 0);
        assert_eq!(u.cache_creation.ephemeral_1h_input_tokens, 0);
    }

    #[test]
    fn usage_reads_nested_cache_creation() {
        let json = r#"{"input_tokens":5,"cache_creation":{"ephemeral_1h_input_tokens":4247,"ephemeral_5m_input_tokens":0}}"#;
        let u: RawUsage = serde_json::from_str(json).unwrap();
        assert_eq!(u.cache_creation.ephemeral_1h_input_tokens, 4247);
    }
}
