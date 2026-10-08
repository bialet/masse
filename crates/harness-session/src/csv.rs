//! Minimal RFC 4180 CSV codec.
//!
//! Fields are quoted when they contain a comma, a double quote, a CR or a LF.
//! Internal double quotes are escaped by doubling them. The parser is a proper
//! state machine over the whole document, so a quoted field may legitimately
//! contain newlines (tool output frequently does).

/// Encode a single field following RFC 4180.
pub fn encode_field(field: &str) -> String {
    if field
        .bytes()
        .any(|b| b == b',' || b == b'"' || b == b'\n' || b == b'\r')
    {
        let mut out = String::with_capacity(field.len() + 2);
        out.push('"');
        for ch in field.chars() {
            if ch == '"' {
                out.push('"');
            }
            out.push(ch);
        }
        out.push('"');
        out
    } else {
        field.to_string()
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
        assert_eq!(
            line,
            "\"hello, world\",\"say \"\"hi\"\"\",\"line1\nline2\","
        );
        assert_eq!(parse_records(&line), vec![fields]);
    }

    #[test]
    fn multiple_records_and_trailing_newline() {
        let doc = "a,b\nc,d\n";
        let recs = parse_records(doc);
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[1], vec!["c".to_string(), "d".to_string()]);
    }
}
