use anyhow::Result;
use serde_json::Value;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub enum TranscriptMessage {
    User      { content: String },
    Assistant { content: String, thinking: Option<String> },
    ToolResult { tool_name: Option<String>, content: String, is_error: bool },
    Summary   { summary: String },
    FileWrite { path: String },
    FileEdit  { path: String },
}

/// Parse a JSONL transcript file. Skips unparseable lines gracefully.
pub fn parse(path: &Path) -> Result<Vec<TranscriptMessage>> {
    let raw = fs::read_to_string(path)?;
    let mut messages = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }
        let Ok(record) = serde_json::from_str::<Value>(line) else { continue };
        if let Some(msg) = parse_record(&record) {
            messages.push(msg);
        }
    }
    Ok(messages)
}

/// Parse transcript and also extract file-op messages from assistant tool_use blocks.
pub fn parse_with_file_ops(path: &Path) -> Result<Vec<TranscriptMessage>> {
    let raw = fs::read_to_string(path)?;
    let mut messages = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }
        let Ok(record) = serde_json::from_str::<Value>(line) else { continue };
        if record.get("type").and_then(|t| t.as_str()) == Some("assistant") {
            messages.extend(extract_file_ops(&record));
        }
        if let Some(msg) = parse_record(&record) {
            messages.push(msg);
        }
    }
    Ok(messages)
}

fn parse_record(record: &Value) -> Option<TranscriptMessage> {
    match record.get("type")?.as_str()? {
        "user" => {
            let msg_content = record.get("message")?.get("content")?;
            let content = msg_content
                .as_str()
                .map(|s| s.to_string())
                .or_else(|| extract_text_blocks(msg_content))?;
            Some(TranscriptMessage::User { content })
        }

        "assistant" => {
            let content_arr = record.get("message")?.get("content")?.as_array()?;
            let mut text_parts = Vec::new();
            let mut thinking = None;
            for part in content_arr {
                match part.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        if let Some(t) = part.get("text").and_then(|v| v.as_str()) {
                            text_parts.push(t.to_string());
                        }
                    }
                    Some("thinking") => {
                        thinking = part
                            .get("thinking")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                    }
                    _ => {}
                }
            }
            if text_parts.is_empty() { return None; }
            Some(TranscriptMessage::Assistant {
                content: text_parts.join("\n"),
                thinking,
            })
        }

        "tool_result" => {
            let result  = record.get("toolUseResult")?;
            let content = result
                .get("content")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string();
            let is_error = result
                .get("is_error")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            Some(TranscriptMessage::ToolResult {
                tool_name: None,
                content,
                is_error,
            })
        }

        "summary" => {
            let summary = record
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if summary.is_empty() { None } else { Some(TranscriptMessage::Summary { summary }) }
        }

        _ => None,
    }
}

/// Extract FileWrite / FileEdit messages from an assistant record's tool_use blocks.
pub fn extract_file_ops(record: &Value) -> Vec<TranscriptMessage> {
    let mut ops = Vec::new();
    let content_arr = match record
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
    {
        Some(a) => a,
        None => return ops,
    };
    for part in content_arr {
        if part.get("type").and_then(|t| t.as_str()) != Some("tool_use") { continue; }
        let name  = match part.get("name").and_then(|n| n.as_str()) { Some(n) => n, None => continue };
        let input = match part.get("input") { Some(i) => i, None => continue };
        match name {
            "Write" => {
                if let Some(p) = input.get("file_path").and_then(|v| v.as_str()) {
                    ops.push(TranscriptMessage::FileWrite { path: p.to_string() });
                }
            }
            "Edit" => {
                if let Some(p) = input.get("file_path").and_then(|v| v.as_str()) {
                    ops.push(TranscriptMessage::FileEdit { path: p.to_string() });
                }
            }
            _ => {}
        }
    }
    ops
}

fn extract_text_blocks(content: &Value) -> Option<String> {
    let parts: Vec<&str> = content
        .as_array()?
        .iter()
        .filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("text"))
        .filter_map(|p| p.get("text")?.as_str())
        .collect();
    if parts.is_empty() { None } else { Some(parts.join("\n")) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn tmp_transcript(lines: &[&str]) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        for line in lines { writeln!(f, "{}", line).unwrap(); }
        f
    }

    #[test]
    fn parses_user_message() {
        let f = tmp_transcript(&[
            r#"{"type":"user","message":{"role":"user","content":"fix the login bug"}}"#,
        ]);
        let msgs = parse(f.path()).unwrap();
        assert_eq!(msgs.len(), 1);
        assert!(matches!(msgs[0], TranscriptMessage::User { .. }));
    }

    #[test]
    fn skips_malformed_lines() {
        let f = tmp_transcript(&[
            "not json",
            r#"{"type":"user","message":{"role":"user","content":"hello"}}"#,
        ]);
        let msgs = parse(f.path()).unwrap();
        assert_eq!(msgs.len(), 1);
    }

    #[test]
    fn parses_assistant_text() {
        let f = tmp_transcript(&[
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"I fixed the bug"}]}}"#,
        ]);
        let msgs = parse(f.path()).unwrap();
        assert_eq!(msgs.len(), 1);
        if let TranscriptMessage::Assistant { content, .. } = &msgs[0] {
            assert!(content.contains("fixed"));
        } else {
            panic!("expected Assistant message");
        }
    }
}
