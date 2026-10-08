//! Minimal RFC 4180 CSV codec.
//!
//! Fields are quoted when they contain a comma, a double quote, a CR or a LF.
//! Internal double quotes are escaped by doubling them. The parser is a proper
//! state machine over the whole document, so a quoted field may legitimately
//! contain newlines (tool output frequently does).

/// Escape the characters that would otherwise break the one-record-per-line
/// invariant: backslash, LF and CR. The escaped form contains no newline, so a
/// record is always exactly one physical line.
pub fn escape_field(field: &str) -> String {
    let mut out = String::with_capacity(field.len());
    for ch in field.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(ch),
        }
    }
    out
}

/// Reverse [`escape_field`]. Unknown escapes are preserved verbatim.
pub fn unescape_field(field: &str) -> String {
    let mut out = String::with_capacity(field.len());
    let mut chars = field.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Encode a single field: escape control characters, then apply RFC 4180
/// quoting only when a comma or a double quote remains.
pub fn encode_field(field: &str) -> String {
    let escaped = escape_field(field);
    if escaped.bytes().any(|b| b == b',' || b == b'"') {
        let mut out = String::with_capacity(escaped.len() + 2);
        out.push('"');
        for ch in escaped.chars() {
            if ch == '"' {
                out.push('"');
            }
            out.push(ch);
        }
        out.push('"');
        out
    } else {
        escaped
    }
}

/// Encode one record (a row) as a CSV line without the trailing newline.
pub fn encode_record(fields: &[String]) -> String {
    let mut out = String::new();
    for (i, f) in fields.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&encode_field(f));
    }
    out
}

/// Parse a whole CSV document into records, honouring quoted fields that span
/// multiple physical lines. A trailing newline does not produce an empty row.
pub fn parse_records(input: &str) -> Vec<Vec<String>> {
    let mut records: Vec<Vec<String>> = Vec::new();
    let mut record: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut started = false;

    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            match c {
                '"' => {
                    if chars.peek() == Some(&'"') {
                        chars.next();
                        field.push('"');
                    } else {
                        in_quotes = false;
                    }
                }
                _ => field.push(c),
            }
        } else {
            match c {
                '"' => {
                    in_quotes = true;
                    started = true;
                }
                ',' => {
                    record.push(std::mem::take(&mut field));
                    started = true;
                }
                '\n' => {
                    record.push(std::mem::take(&mut field));
                    records.push(std::mem::take(&mut record));
                    started = false;
                }
                // Drop CR: CRLF is the canonical terminator and a lone CR inside
                // an unquoted field is not meaningful.
                '\r' => {}
                _ => {
                    field.push(c);
                    started = true;
                }
            }
        }
    }

    if started || !field.is_empty() || !record.is_empty() {
        record.push(field);
        records.push(record);
    }

    records
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_plain() {
        let fields = vec!["a".to_string(), "b".to_string()];
        let line = encode_record(&fields);
        assert_eq!(line, "a,b");
        assert_eq!(parse_records(&line), vec![fields]);
    }

    #[test]
    fn quotes_commas_and_newlines() {
        let fields = vec![
            "hello, world".to_string(),
            "say \"hi\"".to_string(),
            "line1\nline2".to_string(),
            String::new(),
        ];
        let line = encode_record(&fields);
        assert!(!line.contains('\n'), "record must be a single line");
        assert!(line.contains("line1\\nline2"));
        let recs = parse_records(&line);
        assert_eq!(recs.len(), 1);
        assert_eq!(unescape_field(&recs[0][0]), "hello, world");
        assert_eq!(unescape_field(&recs[0][1]), "say \"hi\"");
        assert_eq!(unescape_field(&recs[0][2]), "line1\nline2");
        assert_eq!(recs[0][3], "");
    }

    #[test]
    fn control_escape_roundtrip() {
        let original = "a\\b\nc\rd";
        let encoded = escape_field(original);
        assert!(!encoded.contains('\n') && !encoded.contains('\r'));
        assert_eq!(unescape_field(&encoded), original);
    }

    #[test]
    fn multiple_records_and_trailing_newline() {
        let doc = "a,b\nc,d\n";
        let recs = parse_records(doc);
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[1], vec!["c".to_string(), "d".to_string()]);
    }
}
