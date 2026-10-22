//! The session event schema.
//!
//! Column order is fixed:
//!
//! ```text
//! timestamp,event_type,role,tool,args,status,output,exit_code
//! ```
//!
//! `args` is a `k=v` map joined with `|`, with percent-encoded values. Cells
//! that do not apply to an event are left empty.

use chrono::{SecondsFormat, Utc};

use crate::csv;

/// Column order of `session.csv`.
pub const COLUMNS: [&str; 8] = [
    "timestamp",
    "event_type",
    "role",
    "tool",
    "args",
    "status",
    "output",
    "exit_code",
];

/// The kind of a session event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventType {
    SessionStart,
    /// A user (or command) given title for the session. The last one wins.
    SessionTitle,
    UserMessage,
    AssistantMessage,
    ToolCall,
    ToolResult,
    /// A compacted summary of everything before it. Replay starts here, while
    /// the full raw log stays on disk for debugging and abuse review.
    ContextCompact,
    SessionEnd,
}

impl EventType {
    pub fn as_str(self) -> &'static str {
        match self {
            EventType::SessionStart => "session_start",
            EventType::SessionTitle => "session_title",
            EventType::UserMessage => "user_message",
            EventType::AssistantMessage => "assistant_message",
            EventType::ToolCall => "tool_call",
            EventType::ToolResult => "tool_result",
            EventType::ContextCompact => "context_compact",
            EventType::SessionEnd => "session_end",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "session_start" => EventType::SessionStart,
            "session_title" => EventType::SessionTitle,
            "user_message" => EventType::UserMessage,
            "assistant_message" => EventType::AssistantMessage,
            "tool_call" => EventType::ToolCall,
            "tool_result" => EventType::ToolResult,
            "context_compact" => EventType::ContextCompact,
            "session_end" => EventType::SessionEnd,
            _ => return None,
        })
    }
}

/// The outcome of a tool call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Ok,
    Error,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Error => "error",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "ok" => Status::Ok,
            "error" => Status::Error,
            _ => return None,
        })
    }
}

/// A single, immutable session event.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Event {
    pub timestamp: String,
    pub event_type: String,
    pub role: String,
    pub tool: String,
    pub args: String,
    pub status: String,
    pub output: String,
    pub exit_code: String,
}

impl Event {
    fn now() -> String {
        Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
    }

    fn blank(ty: EventType) -> Self {
        Event {
            timestamp: Self::now(),
            event_type: ty.as_str().to_string(),
            ..Default::default()
        }
    }

    pub fn session_start() -> Self {
        Event {
            role: "system".into(),
            ..Self::blank(EventType::SessionStart)
        }
    }

    pub fn user_message(text: &str) -> Self {
        Event {
            role: "user".into(),
            output: text.to_string(),
            ..Self::blank(EventType::UserMessage)
        }
    }

    /// An explicit session title. Appended, never rewriting the previous one,
    /// so the log remains a complete record.
    pub fn session_title(title: &str) -> Self {
        Event {
            role: "system".into(),
            output: title.to_string(),
            ..Self::blank(EventType::SessionTitle)
        }
    }

    /// A compacted summary of the conversation so far.
    ///
    /// `method` is `model` when the provider wrote the summary and
    /// `heuristic` when compaction fell back to a deterministic digest.
    /// `covered_events` is the length of the log at compaction time, which
    /// makes later audits able to tell exactly what the summary replaced.
    pub fn context_compact(summary: &str, method: &str, covered_events: usize) -> Self {
        Event {
            role: "system".into(),
            args: format!("method={method}|covered_events={covered_events}"),
            output: summary.to_string(),
            ..Self::blank(EventType::ContextCompact)
        }
    }

    pub fn assistant_message(text: &str) -> Self {
        Event {
            role: "assistant".into(),
            output: text.to_string(),
            ..Self::blank(EventType::AssistantMessage)
        }
    }

    pub fn tool_call(tool: &str, args: &str) -> Self {
        Event {
            role: "assistant".into(),
            tool: tool.to_string(),
            args: args.to_string(),
            ..Self::blank(EventType::ToolCall)
        }
    }

    pub fn tool_result(
        tool: &str,
        args: &str,
        status: Status,
        output: &str,
        exit_code: Option<i32>,
    ) -> Self {
        Event {
            role: "tool".into(),
            tool: tool.to_string(),
            args: args.to_string(),
            status: status.as_str().to_string(),
            output: output.to_string(),
            exit_code: exit_code.map(|c| c.to_string()).unwrap_or_default(),
            ..Self::blank(EventType::ToolResult)
        }
    }

    pub fn session_end(reason: &str) -> Self {
        Event {
            role: "system".into(),
            output: reason.to_string(),
            ..Self::blank(EventType::SessionEnd)
        }
    }

    /// The eight fields in canonical column order.
    pub fn fields(&self) -> Vec<String> {
        vec![
            self.timestamp.clone(),
            self.event_type.clone(),
            self.role.clone(),
            self.tool.clone(),
            self.args.clone(),
            self.status.clone(),
            self.output.clone(),
            self.exit_code.clone(),
        ]
    }

    /// Encode as one CSV record (no trailing newline).
    pub fn to_record(&self) -> String {
        csv::encode_record(&self.fields())
    }

    /// Rebuild an event from a parsed CSV record.
    pub fn from_record(rec: &[String]) -> Option<Self> {
        if rec.len() != COLUMNS.len() {
            return None;
        }
        // Tolerate a copied header line: it is documentation, not an event.
        if rec[1] == COLUMNS[1] {
            return None;
        }
        Some(Event {
            timestamp: rec[0].clone(),
            event_type: rec[1].clone(),
            role: rec[2].clone(),
            tool: rec[3].clone(),
            args: csv::unescape_field(&rec[4]),
            status: rec[5].clone(),
            output: csv::unescape_field(&rec[6]),
            exit_code: rec[7].clone(),
        })
    }

    pub fn kind(&self) -> Option<EventType> {
        EventType::parse(&self.event_type)
    }

    pub fn status_kind(&self) -> Option<Status> {
        Status::parse(&self.status)
    }

    pub fn exit_code_value(&self) -> Option<i32> {
        if self.exit_code.is_empty() {
            None
        } else {
            self.exit_code.parse().ok()
        }
    }
}

/// Percent-encode a value so it survives the `|` and `=` delimiters.
pub fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Decode a value produced by [`percent_encode`].
pub fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&value[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Encode an argument map for the `args` column: `k=v|k=v`, values encoded.
pub fn encode_args(pairs: &[(&str, String)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", k, percent_encode(v)))
        .collect::<Vec<_>>()
        .join("|")
}

/// Decode an `args` column back into `(key, value)` pairs.
pub fn decode_args(args: &str) -> Vec<(String, String)> {
    if args.is_empty() {
        return Vec::new();
    }
    args.split('|')
        .filter_map(|pair| {
            pair.split_once('=')
                .map(|(k, v)| (k.to_string(), percent_decode(v)))
        })
        .collect()
}

/// Serialize a `serde_json::Value` object as an `args` map string.
pub fn encode_json_args(value: &serde_json::Value) -> String {
    let mut pairs: Vec<(&str, String)> = Vec::new();
    if let serde_json::Value::Object(map) = value {
        for (k, v) in map {
            let text = match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            pairs.push((k.as_str(), text));
        }
    }
    encode_args(&pairs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_roundtrip_with_specials() {
        let encoded = encode_args(&[
            ("path", "src/a,b.wren".to_string()),
            ("pattern", "foo|bar=1".to_string()),
        ]);
        assert!(encoded.contains("%7C"));
        let decoded = decode_args(&encoded);
        assert_eq!(decoded[0], ("path".to_string(), "src/a,b.wren".to_string()));
        assert_eq!(decoded[1], ("pattern".to_string(), "foo|bar=1".to_string()));
    }

    #[test]
    fn record_roundtrip() {
        let ev = Event::tool_result(
            "curl",
            "url=https%3A%2F%2Fexample.com",
            Status::Ok,
            "a,b\nc",
            Some(0),
        );
        let rec = ev.to_record();
        let parsed = csv::parse_records(&rec);
        let back = Event::from_record(&parsed[0]).unwrap();
        assert_eq!(back.tool, "curl");
        assert_eq!(back.output, "a,b\nc");
        assert_eq!(back.exit_code_value(), Some(0));
    }

    #[test]
    fn a_copied_header_row_is_not_an_event() {
        let header: Vec<String> = COLUMNS.iter().map(|c| c.to_string()).collect();
        assert!(Event::from_record(&header).is_none());
    }
}
