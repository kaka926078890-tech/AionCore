use futures_util::StreamExt;
use reqwest::Client;
use serde_json::{Value, json};

#[derive(Debug, Clone)]
pub enum FinclawInferEvent {
    TextChunk(String),
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
        "done" | "run_finished" | "finish" => Some(FinclawInferEvent::Finished),
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
}
