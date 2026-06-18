use futures_util::StreamExt;
use reqwest::Client;
use serde_json::{Value, json};
use tracing::info;

use crate::finclaw::approval::{FinclawApprovalRequired, parse_approval_required};

#[derive(Debug, Clone)]
pub struct FinclawToolProgressEvent {
    pub custom_name: String,
    pub tool_call_id: String,
    pub title: Option<String>,
    pub status: Option<String>,
    pub kind: Option<String>,
    pub raw_input: Option<Value>,
    pub raw_output: Option<Value>,
    pub content_text: Option<String>,
}

#[derive(Debug, Clone)]
pub enum FinclawInferEvent {
    TextChunk(String),
    ThinkingChunk(String),
    ToolProgress(FinclawToolProgressEvent),
    ApprovalRequired(FinclawApprovalRequired),
    Finished,
    Error(String),
}

pub async fn post_infer_stream(
    client: &Client,
    claw_port: u16,
    conversation_id: &str,
    user_id: &str,
    message: &str,
    capability: &str,
    max_tokens: Option<u32>,
) -> Result<impl futures_util::Stream<Item = Result<FinclawInferEvent, String>> + use<>, String> {
    let url = format!("http://127.0.0.1:{claw_port}/ai/infer/stream");
    let mut body = json!({
        "message": message,
        "user_id": user_id,
        "session_id": conversation_id,
        "capability": capability,
    });
    if let Some(max_tokens) = max_tokens {
        body["max_tokens"] = json!(max_tokens);
    }

    let response = client
        .post(url)
        .header("Accept", "text/event-stream")
        .header("Content-Type", "application/json")
        .header("X-User-ID", user_id)
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(if text.is_empty() {
            format!("FinClaw infer failed: {status}")
        } else {
            text
        });
    }

    let byte_stream = response.bytes_stream();
    let event_stream = async_stream::stream! {
        let mut buffer = String::new();
        futures_util::pin_mut!(byte_stream);
        while let Some(chunk) = byte_stream.next().await {
            let chunk = chunk.map_err(|e| e.to_string())?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(pos) = buffer.find("\n\n") {
                let frame = buffer[..pos].to_string();
                buffer = buffer[pos + 2..].to_string();
                if let Some(event) = parse_sse_frame(&frame) {
                    yield Ok(event);
                }
            }
        }
    };

    Ok(event_stream)
}

fn parse_sse_frame(frame: &str) -> Option<FinclawInferEvent> {
    let data_line = frame.lines().find(|line| line.starts_with("data:"))?;
    let raw = data_line.trim_start_matches("data:").trim();
    if raw.is_empty() {
        return None;
    }
    let parsed: Value = serde_json::from_str(raw).ok()?;
    let event_type = parsed
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();

    if let Some(approval) = parse_approval_event(&parsed, &event_type) {
        return Some(FinclawInferEvent::ApprovalRequired(approval));
    }

    if let Some(thinking) = parse_thinking_event(&parsed, &event_type) {
        return Some(FinclawInferEvent::ThinkingChunk(thinking));
    }

    if let Some(tool_progress) = parse_standard_tool_call_event(&parsed, &event_type) {
        return Some(FinclawInferEvent::ToolProgress(tool_progress));
    }

    if let Some(tool_progress) = parse_tool_progress_event(&parsed, &event_type) {
        return Some(FinclawInferEvent::ToolProgress(tool_progress));
    }

    match event_type.as_str() {
        "content" => {
            let delta = parsed
                .get("content")
                .or_else(|| parsed.get("delta"))
                .and_then(Value::as_str)?;
            if delta.is_empty() {
                None
            } else {
                Some(FinclawInferEvent::TextChunk(delta.to_string()))
            }
        }
        "done" | "run_finished" | "finish" => {
            info!(
                finclaw_event_type = %event_type,
                "FinClaw infer terminal SSE frame received"
            );
            Some(FinclawInferEvent::Finished)
        }
        "error" | "run_error" => {
            let message = parsed
                .get("message")
                .or_else(|| parsed.get("error"))
                .and_then(Value::as_str)
                .unwrap_or("Unknown error");
            Some(FinclawInferEvent::Error(message.to_string()))
        }
        _ => None,
    }
}

fn parse_thinking_event(parsed: &Value, event_type: &str) -> Option<String> {
    if event_type == "thinking" {
        return parsed
            .get("content")
            .or_else(|| parsed.get("delta"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|s| !s.is_empty());
    }

    if event_type != "custom" {
        return None;
    }

    let name = parsed
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    if name != "thinking" {
        return None;
    }

    let value = parsed.get("value")?;
    let value_obj = match value {
        Value::Object(_) => value.clone(),
        Value::String(raw) => serde_json::from_str::<Value>(raw).ok()?,
        _ => return None,
    };

    let inner_type = value_obj
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    if inner_type != "thinking" {
        return None;
    }

    value_obj
        .get("content")
        .or_else(|| value_obj.get("delta"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}

fn parse_standard_tool_call_event(parsed: &Value, event_type: &str) -> Option<FinclawToolProgressEvent> {
    let kind = match event_type {
        "tool_call_start" | "tool_call_args" | "tool_call_end" | "tool_call_result" | "tool_call_chunk" => event_type,
        _ => return None,
    };

    let tool_call_id = parsed
        .get("tool_call_id")
        .or_else(|| parsed.get("toolCallId"))
        .or_else(|| parsed.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())?
        .to_string();

    let title = parsed
        .get("tool_call_name")
        .or_else(|| parsed.get("toolCallName"))
        .or_else(|| parsed.get("tool_name"))
        .or_else(|| parsed.get("toolName"))
        .or_else(|| parsed.get("name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let status = match kind {
        "tool_call_end" | "tool_call_result" => Some("completed".to_string()),
        _ => Some("in_progress".to_string()),
    };

    let raw_input = match kind {
        "tool_call_args" | "tool_call_chunk" => parsed
            .get("delta")
            .map(normalize_streamed_json_value)
            .or_else(|| parsed.get("args").cloned())
            .or_else(|| parsed.get("arguments").cloned()),
        "tool_call_start" => parsed
            .get("args")
            .cloned()
            .or_else(|| parsed.get("arguments").cloned())
            .or_else(|| parsed.get("raw_input").cloned())
            .or_else(|| parsed.get("rawInput").cloned()),
        _ => None,
    };

    let raw_output = match kind {
        "tool_call_result" => parsed
            .get("result")
            .cloned()
            .or_else(|| parsed.get("output").cloned())
            .or_else(|| parsed.get("content").cloned())
            .or_else(|| parsed.get("value").cloned()),
        _ => None,
    };

    Some(FinclawToolProgressEvent {
        custom_name: kind.to_string(),
        tool_call_id,
        title,
        status,
        kind: Some("execute".to_string()),
        raw_input,
        raw_output,
        content_text: None,
    })
}

fn normalize_streamed_json_value(value: &Value) -> Value {
    match value {
        Value::String(text) => {
            if let Ok(decoded) = serde_json::from_str::<Value>(text) {
                decoded
            } else {
                json!({ "delta": text })
            }
        }
        _ => value.clone(),
    }
}

fn parse_tool_progress_event(parsed: &Value, event_type: &str) -> Option<FinclawToolProgressEvent> {
    let custom_name = parsed
        .get("name")
        .and_then(Value::as_str)
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    let source = if event_type == "custom" {
        custom_source_payload(parsed)
    } else {
        parsed.clone()
    };

    let inner_type = source
        .get("type")
        .and_then(Value::as_str)
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    let is_tool_event = matches!(event_type, "tool.progress" | "tool_call_adjusted")
        || matches!(custom_name.as_str(), "tool.progress" | "tool_call_adjusted")
        || matches!(inner_type.as_str(), "tool.progress" | "tool_call_adjusted");
    if !is_tool_event {
        return None;
    }

    let nested_tool_call = source.get("tool_call").filter(|v| v.is_object());
    let tool_call_id = source
        .get("tool_call_id")
        .or_else(|| source.get("call_id"))
        .or_else(|| source.get("toolCallId"))
        .or_else(|| source.get("id"))
        .or_else(|| nested_tool_call.and_then(|tool| tool.get("tool_call_id")))
        .or_else(|| nested_tool_call.and_then(|tool| tool.get("call_id")))
        .or_else(|| nested_tool_call.and_then(|tool| tool.get("id")))
        .or_else(|| source.get("approval_request_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            // Some tool-progress frames omit call identifiers.
            // Keep them visible in UI by synthesizing a stable, merge-friendly id.
            let title_seed = source
                .get("tool_name")
                .or_else(|| source.get("tool"))
                .or_else(|| source.get("title"))
                .or_else(|| source.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("tool");
            let phase_seed = source
                .get("status")
                .or_else(|| source.get("state"))
                .and_then(Value::as_str)
                .unwrap_or("progress");
            format!("finclaw:{}:{}:{}", event_type, title_seed, phase_seed)
        });

    let title = source
        .get("tool_name")
        .or_else(|| source.get("tool"))
        .or_else(|| source.get("title"))
        .or_else(|| source.get("name"))
        .or_else(|| nested_tool_call.and_then(|tool| tool.get("title")))
        .or_else(|| nested_tool_call.and_then(|tool| tool.get("name")))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let status = source
        .get("status")
        .or_else(|| source.get("state"))
        .or_else(|| nested_tool_call.and_then(|tool| tool.get("status")))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let kind = source
        .get("kind")
        .or_else(|| source.get("tool_kind"))
        .or_else(|| source.get("toolKind"))
        .or_else(|| nested_tool_call.and_then(|tool| tool.get("kind")))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let raw_input = source
        .get("raw_input")
        .or_else(|| source.get("rawInput"))
        .or_else(|| source.get("arguments"))
        .or_else(|| source.get("input"))
        .filter(|v| !v.is_null())
        .cloned();
    let raw_output = source
        .get("raw_output")
        .or_else(|| source.get("rawOutput"))
        .or_else(|| source.get("output"))
        .or_else(|| source.get("result"))
        .filter(|v| !v.is_null())
        .cloned();
    let content_text = source
        .get("content")
        .or_else(|| source.get("message"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    Some(FinclawToolProgressEvent {
        custom_name,
        tool_call_id,
        title,
        status,
        kind,
        raw_input,
        raw_output,
        content_text,
    })
}

fn custom_source_payload(parsed: &Value) -> Value {
    let mut source = parsed.clone();
    if let Some(value) = parsed.get("value") {
        if value.is_object() {
            source = value.clone();
        } else if let Some(raw) = value.as_str()
            && let Ok(decoded) = serde_json::from_str::<Value>(raw)
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
    source
}

fn parse_approval_event(parsed: &Value, event_type: &str) -> Option<FinclawApprovalRequired> {
    if event_type == "approval_required" || event_type == "approval.required" {
        return parse_approval_required(parsed);
    }
    if event_type == "custom" {
        let name = parsed
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        if name == "approval.required" {
            return parse_approval_required(parsed);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_content_chunk() {
        let frame = "event: message\ndata: {\"type\":\"content\",\"delta\":\"hello\"}\n";
        let event = parse_sse_frame(frame).expect("chunk");
        assert!(matches!(event, FinclawInferEvent::TextChunk(ref s) if s == "hello"));
    }

    #[test]
    fn parse_custom_thinking_chunk() {
        let frame =
            "data: {\"type\":\"CUSTOM\",\"name\":\"thinking\",\"value\":\"{\\\"type\\\":\\\"thinking\\\",\\\"content\\\":\\\"analyzing\\\"}\"}\n";
        let event = parse_sse_frame(frame).expect("thinking");
        assert!(matches!(event, FinclawInferEvent::ThinkingChunk(ref s) if s == "analyzing"));
    }

    #[test]
    fn parse_finish_event() {
        let frame = "data: {\"type\":\"done\"}\n";
        assert!(matches!(parse_sse_frame(frame), Some(FinclawInferEvent::Finished)));
    }

    #[test]
    fn parse_error_event() {
        let frame = "data: {\"type\":\"error\",\"message\":\"boom\"}\n";
        assert!(matches!(
            parse_sse_frame(frame),
            Some(FinclawInferEvent::Error(ref m)) if m == "boom"
        ));
    }

    #[test]
    fn parse_approval_required_event() {
        let frame = "data: {\"type\":\"approval_required\",\"approval_request_id\":\"apr-1\",\"tool_name\":\"bash\",\"tool_call_id\":\"tc-1\"}\n";
        let event = parse_sse_frame(frame).expect("approval");
        assert!(matches!(
            event,
            FinclawInferEvent::ApprovalRequired(ref approval)
                if approval.approval_request_id == "apr-1" && approval.tool_name == "bash"
        ));
    }

    #[test]
    fn parse_custom_approval_required_event() {
        let frame = "data: {\"type\":\"CUSTOM\",\"name\":\"approval.required\",\"value\":{\"approval_request_id\":\"apr-2\",\"tool_name\":\"write\"}}\n";
        let event = parse_sse_frame(frame).expect("approval");
        assert!(matches!(
            event,
            FinclawInferEvent::ApprovalRequired(ref approval) if approval.approval_request_id == "apr-2"
        ));
    }

    #[test]
    fn parse_custom_tool_progress_event() {
        let frame = "data: {\"type\":\"CUSTOM\",\"name\":\"tool.progress\",\"value\":{\"type\":\"tool.progress\",\"tool_call_id\":\"tc-1\",\"tool_name\":\"bash\",\"status\":\"running\",\"kind\":\"execute\",\"arguments\":{\"command\":\"pwd\"}}}\n";
        let event = parse_sse_frame(frame).expect("tool.progress");
        assert!(matches!(
            event,
            FinclawInferEvent::ToolProgress(FinclawToolProgressEvent {
                tool_call_id,
                title: Some(title),
                status: Some(status),
                kind: Some(kind),
                ..
            }) if tool_call_id == "tc-1" && title == "bash" && status == "running" && kind == "execute"
        ));
    }

    #[test]
    fn parse_custom_tool_progress_merges_metadata() {
        let frame = "data: {\"type\":\"CUSTOM\",\"name\":\"tool_call_adjusted\",\"value\":{\"tool_call_id\":\"tc-2\",\"tool_name\":\"ReadFile\",\"metadata\":{\"status\":\"completed\"}}}\n";
        let event = parse_sse_frame(frame).expect("tool_call_adjusted");
        assert!(matches!(
            event,
            FinclawInferEvent::ToolProgress(FinclawToolProgressEvent {
                tool_call_id,
                status: Some(status),
                ..
            }) if tool_call_id == "tc-2" && status == "completed"
        ));
    }

    #[test]
    fn parse_tool_progress_without_call_id_uses_fallback_id() {
        let frame = "data: {\"type\":\"CUSTOM\",\"name\":\"tool.progress\",\"value\":{\"tool_name\":\"WriteFile\",\"status\":\"running\"}}\n";
        let event = parse_sse_frame(frame).expect("tool.progress");
        assert!(matches!(
            event,
            FinclawInferEvent::ToolProgress(FinclawToolProgressEvent {
                tool_call_id,
                title: Some(title),
                ..
            }) if tool_call_id.starts_with("finclaw:custom:WriteFile") && title == "WriteFile"
        ));
    }

    #[test]
    fn parse_direct_tool_progress_event() {
        let frame = "data: {\"type\":\"tool.progress\",\"tool_call\":{\"id\":\"tc-3\",\"title\":\"Shell\",\"status\":\"in_progress\",\"kind\":\"execute\"}}\n";
        let event = parse_sse_frame(frame).expect("tool.progress");
        assert!(matches!(
            event,
            FinclawInferEvent::ToolProgress(FinclawToolProgressEvent {
                tool_call_id,
                title: Some(title),
                status: Some(status),
                kind: Some(kind),
                ..
            }) if tool_call_id == "tc-3" && title == "Shell" && status == "in_progress" && kind == "execute"
        ));
    }

    #[test]
    fn parse_standard_tool_call_start_event() {
        let frame = "data: {\"type\":\"TOOL_CALL_START\",\"toolCallId\":\"tc-4\",\"toolCallName\":\"WriteFile\"}\n";
        let event = parse_sse_frame(frame).expect("tool_call_start");
        assert!(matches!(
            event,
            FinclawInferEvent::ToolProgress(FinclawToolProgressEvent {
                tool_call_id,
                title: Some(title),
                status: Some(status),
                ..
            }) if tool_call_id == "tc-4" && title == "WriteFile" && status == "in_progress"
        ));
    }

    #[test]
    fn parse_standard_tool_call_result_event() {
        let frame = "data: {\"type\":\"TOOL_CALL_RESULT\",\"tool_call_id\":\"tc-5\",\"result\":{\"ok\":true}}\n";
        let event = parse_sse_frame(frame).expect("tool_call_result");
        assert!(matches!(
            event,
            FinclawInferEvent::ToolProgress(FinclawToolProgressEvent {
                tool_call_id,
                status: Some(status),
                raw_output: Some(Value::Object(_)),
                ..
            }) if tool_call_id == "tc-5" && status == "completed"
        ));
    }
}
