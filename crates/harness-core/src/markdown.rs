//! A small, dependency-free Markdown renderer.
//!
//! Assistant output is Markdown, and both front ends want to *render* it rather
//! than show the raw source — the TUI as styled terminal lines, the web page as
//! HTML. Parsing therefore lives here, in the shared core, and each front end
//! only formats the resulting AST.
//!
//! The supported subset is intentionally small but covers what a coding agent
//! actually writes: ATX headings, paragraphs, fenced code, blockquotes, ordered
//! and unordered lists, thematic breaks, pipe tables, and the usual inline
//! emphasis, inline code and links. Anything the parser does not understand
//! falls through as plain text, so no input is ever lost.

/// A parsed Markdown document: an ordered sequence of blocks.
pub type Document = Vec<Block>;

/// A block-level element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Heading {
        level: u8,
        content: Vec<Inline>,
    },
    Paragraph(Vec<Inline>),
    CodeBlock {
        language: Option<String>,
        code: String,
    },
    Quote(Vec<Block>),
    List {
        ordered: bool,
        start: u64,
        items: Vec<Vec<Inline>>,
    },
    Rule,
    Table {
        header: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
    },
}

/// An inline element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inline {
    Text(String),
    Code(String),
    Bold(Vec<Inline>),
    Italic(Vec<Inline>),
    Strike(Vec<Inline>),
    Link { text: Vec<Inline>, url: String },
}

/// Parse a Markdown string into blocks.
pub fn parse(input: &str) -> Document {
    let normalized = input.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
    let mut blocks = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();

        if trimmed.trim().is_empty() {
            i += 1;
            continue;
        }

        // Fenced code block.
        if let Some((fence, fence_len, language)) = parse_fence(trimmed) {
            let mut code = String::new();
            i += 1;
            while i < lines.len() {
                if is_closing_fence(lines[i].trim_start(), fence, fence_len) {
                    i += 1;
                    break;
                }
                if !code.is_empty() {
                    code.push('\n');
                }
                code.push_str(lines[i]);
                i += 1;
            }
            blocks.push(Block::CodeBlock { language, code });
            continue;
        }

        // ATX heading.
        if let Some((level, content)) = parse_heading(trimmed) {
            blocks.push(Block::Heading {
                level,
                content: parse_inlines(content),
            });
            i += 1;
            continue;
        }

        // Thematic break.
        if is_thematic_break(trimmed) {
            blocks.push(Block::Rule);
            i += 1;
            continue;
        }

        // Pipe table (header line plus a `---`-style delimiter).
        if trimmed.contains('|') && i + 1 < lines.len() && is_table_delimiter(lines[i + 1].trim()) {
            if let Some((table, next)) = parse_table(&lines, i) {
                blocks.push(table);
                i = next;
                continue;
            }
        }

        // Blockquote: consecutive `>`-prefixed lines, parsed recursively.
        if trimmed.starts_with('>') {
            let mut inner = String::new();
            while i < lines.len() {
                let lt = lines[i].trim_start();
                let Some(rest) = lt.strip_prefix('>') else {
                    break;
                };
                let rest = rest.strip_prefix(' ').unwrap_or(rest);
                if !inner.is_empty() {
                    inner.push('\n');
                }
                inner.push_str(rest);
                i += 1;
            }
            blocks.push(Block::Quote(parse(&inner)));
            continue;
        }

        // List.
        if let Some(marker) = list_marker(line) {
            let (block, next) = parse_list(&lines, i, marker);
            blocks.push(block);
            i = next;
            continue;
        }

        // Paragraph: consecutive lines until a blank line or a new block.
        let mut para: Vec<&str> = Vec::new();
        while i < lines.len() {
            let lt = lines[i].trim_start();
            if lt.trim().is_empty() {
                break;
            }
            if !para.is_empty()
                && (starts_block(lt)
                    || (lt.contains('|')
                        && i + 1 < lines.len()
                        && is_table_delimiter(lines[i + 1].trim())))
            {
                break;
            }
            para.push(lines[i].trim());
            i += 1;
        }
        blocks.push(Block::Paragraph(parse_inlines(&para.join(" "))));
    }

    blocks
}

/// Parse standalone inline content (no block structure).
pub fn parse_inlines(text: &str) -> Vec<Inline> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' if i + 1 < chars.len() && is_escapable(chars[i + 1]) => {
                buf.push(chars[i + 1]);
                i += 2;
            }
            '`' => {
                if let Some(end) = find_run(&chars, i + 1, '`', 1) {
                    let code: String = chars[i + 1..end].iter().collect();
                    push_text(&mut out, &mut buf);
                    out.push(Inline::Code(code.trim().to_string()));
                    i = end + 1;
                } else {
                    buf.push(c);
                    i += 1;
                }
            }
            '*' | '_' => {
                // Underscores inside a word (`snake_case`) are literal.
                if c == '_' && i > 0 && chars[i - 1].is_alphanumeric() {
                    buf.push(c);
                    i += 1;
                    continue;
                }
                let run = run_length(&chars, i, c);
                let delim = if run >= 2 { 2 } else { 1 };
                if let Some(end) = find_exact_run(&chars, i + delim, c, delim) {
                    let opens = i + delim < chars.len() && !chars[i + delim].is_whitespace();
                    let closes = end > 0 && !chars[end - 1].is_whitespace();
                    if end > i + delim && opens && closes {
                        let inner: String = chars[i + delim..end].iter().collect();
                        push_text(&mut out, &mut buf);
                        let children = parse_inlines(&inner);
                        out.push(if delim == 2 {
                            Inline::Bold(children)
                        } else {
                            Inline::Italic(children)
                        });
                        i = end + delim;
                        continue;
                    }
                }
                buf.push(c);
                i += 1;
            }
            '~' if i + 1 < chars.len() && chars[i + 1] == '~' => {
                if let Some(end) = find_exact_run(&chars, i + 2, '~', 2) {
                    if end > i + 2 {
                        let inner: String = chars[i + 2..end].iter().collect();
                        push_text(&mut out, &mut buf);
                        out.push(Inline::Strike(parse_inlines(&inner)));
                        i = end + 2;
                        continue;
                    }
                }
                buf.push(c);
                i += 1;
            }
            '[' => {
                if let Some((text_end, url, after)) = parse_link_at(&chars, i) {
                    let label: String = chars[i + 1..text_end].iter().collect();
                    push_text(&mut out, &mut buf);
                    out.push(Inline::Link {
                        text: parse_inlines(&label),
                        url,
                    });
                    i = after;
                    continue;
                }
                buf.push(c);
                i += 1;
            }
            '!' if i + 1 < chars.len() && chars[i + 1] == '[' => {
                // Images have no terminal or web representation here; keep the
                // alt text so nothing is lost.
                if let Some((text_end, _url, after)) = parse_link_at(&chars, i + 1) {
                    let alt: String = chars[i + 2..text_end].iter().collect();
                    push_text(&mut out, &mut buf);
                    out.push(Inline::Text(alt));
                    i = after;
                    continue;
                }
                buf.push(c);
                i += 1;
            }
            _ => {
                buf.push(c);
                i += 1;
            }
        }
    }

    push_text(&mut out, &mut buf);
    out
}

/// Render a Markdown string to HTML.
pub fn to_html(input: &str) -> String {
    let mut out = String::new();
    write_html_blocks(&parse(input), &mut out);
    out
}

/// Render parsed blocks to HTML.
pub fn blocks_to_html(blocks: &[Block]) -> String {
    let mut out = String::new();
    write_html_blocks(blocks, &mut out);
    out
}

/// The plain text of a run of inlines, with all markup stripped. Handy for
/// measuring and for rendering a label without styles.
pub fn inline_text(inlines: &[Inline]) -> String {
    let mut out = String::new();
    for inline in inlines {
        match inline {
            Inline::Text(t) | Inline::Code(t) => out.push_str(t),
            Inline::Bold(x) | Inline::Italic(x) | Inline::Strike(x) => {
                out.push_str(&inline_text(x))
            }
            Inline::Link { text, .. } => out.push_str(&inline_text(text)),
        }
    }
    out
}

fn push_text(out: &mut Vec<Inline>, buf: &mut String) {
    if !buf.is_empty() {
        out.push(Inline::Text(std::mem::take(buf)));
    }
}

// --- block helpers ---------------------------------------------------------

/// `Option<(fence char, fence length, info string)>` for an opening fence.
fn parse_fence(line: &str) -> Option<(char, usize, Option<String>)> {
    let c = line.chars().next()?;
    if c != '`' && c != '~' {
        return None;
    }
    let len = line.chars().take_while(|x| *x == c).count();
    if len < 3 {
        return None;
    }
    let info = line[len..].trim();
    let language = info
        .split_whitespace()
        .next()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    Some((c, len, language))
}

fn is_closing_fence(line: &str, fence: char, min_len: usize) -> bool {
    let n = line.chars().take_while(|x| *x == fence).count();
    n >= min_len && line[n..].trim().is_empty()
}

fn parse_heading(line: &str) -> Option<(u8, &str)> {
    let level = line.chars().take_while(|c| *c == '#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let rest = &line[level..];
    if !rest.is_empty() && !rest.starts_with(' ') && !rest.starts_with('\t') {
        return None;
    }
    Some((level as u8, strip_closing_hashes(rest.trim())))
}

fn strip_closing_hashes(text: &str) -> &str {
    let trimmed = text.trim_end();
    if !trimmed.ends_with('#') {
        return trimmed;
    }
    let hashes = trimmed.chars().rev().take_while(|c| *c == '#').count();
    let head = &trimmed[..trimmed.len() - hashes];
    if head.ends_with(' ') || head.is_empty() {
        head.trim_end()
    } else {
        trimmed
    }
}

fn is_thematic_break(line: &str) -> bool {
    let t = line.trim();
    let mut marker = None;
    let mut count = 0usize;
    for c in t.chars() {
        if c == ' ' || c == '\t' {
            continue;
        }
        match marker {
            None => {
                if c == '-' || c == '*' || c == '_' {
                    marker = Some(c);
                    count = 1;
                } else {
                    return false;
                }
            }
            Some(m) if m == c => count += 1,
            _ => return false,
        }
    }
    marker.is_some() && count >= 3
}

/// A line that begins a new block and therefore terminates a paragraph.
fn starts_block(line: &str) -> bool {
    parse_fence(line).is_some()
        || parse_heading(line).is_some()
        || is_thematic_break(line)
        || line.starts_with('>')
        || list_marker(line).is_some()
}

#[derive(Clone, Copy)]
struct ListMarker {
    ordered: bool,
    start: u64,
    indent: usize,
    content_start: usize,
}

fn list_marker(line: &str) -> Option<ListMarker> {
    let indent = line.len() - line.trim_start().len();
    let t = &line[indent..];
    let first = t.chars().next()?;

    if first == '-' || first == '*' || first == '+' {
        let after = &t[first.len_utf8()..];
        if after.is_empty() || after.starts_with(' ') || after.starts_with('\t') {
            return Some(ListMarker {
                ordered: false,
                start: 0,
                indent,
                content_start: indent + first.len_utf8(),
            });
        }
        return None;
    }

    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 && digits <= 9 {
        let after = &t[digits..];
        let sep = after.chars().next()?;
        if sep == '.' || sep == ')' {
            let rest = &after[sep.len_utf8()..];
            if rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t') {
                let start = t[..digits].parse::<u64>().unwrap_or(1);
                return Some(ListMarker {
                    ordered: true,
                    start,
                    indent,
                    content_start: indent + digits + sep.len_utf8(),
                });
            }
        }
    }
    None
}

fn parse_list(lines: &[&str], start_idx: usize, marker: ListMarker) -> (Block, usize) {
    let mut items: Vec<Vec<Inline>> = Vec::new();
    let mut i = start_idx;

    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            break;
        }
        let indent = line.len() - line.trim_start().len();

        if let Some(m) = list_marker(line) {
            if m.indent == marker.indent && m.ordered == marker.ordered {
                items.push(parse_inlines(line[m.content_start..].trim()));
                i += 1;
                continue;
            }
            break;
        }

        // An indented line continues the previous item.
        if indent > marker.indent {
            if let Some(last) = items.last_mut() {
                last.push(Inline::Text(" ".into()));
                last.extend(parse_inlines(line.trim()));
                i += 1;
                continue;
            }
        }
        break;
    }

    (
        Block::List {
            ordered: marker.ordered,
            start: marker.start.max(1),
            items,
        },
        i,
    )
}

fn split_table_row(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    t.split('|').map(|c| c.trim().to_string()).collect()
}

fn is_table_delimiter(line: &str) -> bool {
    let t = line.trim();
    if !t.contains('-') {
        return false;
    }
    let cells = split_table_row(t);
    !cells.is_empty()
        && cells.iter().all(|c| {
            let c = c.trim_matches(':');
            !c.is_empty() && c.chars().all(|ch| ch == '-')
        })
}

fn parse_table(lines: &[&str], i: usize) -> Option<(Block, usize)> {
    let header_cells = split_table_row(lines[i]);
    let delim_cells = split_table_row(lines[i + 1]);
    if header_cells.is_empty() || delim_cells.len() != header_cells.len() {
        return None;
    }

    let header: Vec<Vec<Inline>> = header_cells.iter().map(|c| parse_inlines(c)).collect();
    let mut rows = Vec::new();
    let mut j = i + 2;
    while j < lines.len() {
        let l = lines[j];
        if l.trim().is_empty() || !l.contains('|') || parse_fence(l.trim_start()).is_some() {
            break;
        }
        let mut row: Vec<Vec<Inline>> = split_table_row(l)
            .iter()
            .map(|c| parse_inlines(c))
            .collect();
        row.resize(header.len(), Vec::new());
        row.truncate(header.len());
        rows.push(row);
        j += 1;
    }

    Some((Block::Table { header, rows }, j))
}

// --- inline helpers --------------------------------------------------------

fn is_escapable(c: char) -> bool {
    matches!(
        c,
        '\\' | '`'
            | '*'
            | '_'
            | '{'
            | '}'
            | '['
            | ']'
            | '('
            | ')'
            | '#'
            | '+'
            | '-'
            | '.'
            | '!'
            | '|'
            | '~'
            | '<'
            | '>'
            | '"'
            | '\''
            | '&'
    )
}

fn run_length(chars: &[char], start: usize, c: char) -> usize {
    let mut n = 0;
    while start + n < chars.len() && chars[start + n] == c {
        n += 1;
    }
    n
}

/// First index at or after `from` where exactly `len` copies of `c` appear.
fn find_exact_run(chars: &[char], from: usize, c: char, len: usize) -> Option<usize> {
    let mut i = from;
    while i + len <= chars.len() {
        if chars[i] == c {
            let run_start = i;
            let run = run_length(chars, i, c);
            if run == len {
                return Some(run_start);
            }
            i += run;
        } else {
            i += 1;
        }
    }
    None
}

/// First index at or after `from` where a run of at least `len` copies of `c`
/// starts.
fn find_run(chars: &[char], from: usize, c: char, len: usize) -> Option<usize> {
    let mut i = from;
    while i + len <= chars.len() {
        if chars[i] == c {
            let run = run_length(chars, i, c);
            if run >= len {
                return Some(i);
            }
            i += run;
        } else {
            i += 1;
        }
    }
    None
}

/// Parse `[label](url)` starting at the index of `[`. Returns
/// `(label_end, url, index_after_link)`.
fn parse_link_at(chars: &[char], open: usize) -> Option<(usize, String, usize)> {
    if chars.get(open) != Some(&'[') {
        return None;
    }
    let mut close = None;
    let mut i = open + 1;
    while i < chars.len() {
        match chars[i] {
            ']' => {
                close = Some(i);
                break;
            }
            '[' => return None,
            _ => i += 1,
        }
    }
    let close = close?;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let mut paren = None;
    let mut j = close + 2;
    while j < chars.len() {
        if chars[j] == ')' {
            paren = Some(j);
            break;
        }
        j += 1;
    }
    let paren = paren?;
    let raw: String = chars[close + 2..paren].iter().collect();
    let url = raw
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches(|c| c == '<' || c == '>')
        .to_string();
    if url.is_empty() {
        return None;
    }
    Some((close, url, paren + 1))
}

// --- HTML rendering --------------------------------------------------------

fn write_html_blocks(blocks: &[Block], out: &mut String) {
    for block in blocks {
        match block {
            Block::Heading { level, content } => {
                let level = (*level).clamp(1, 6);
                out.push_str(&format!("<h{level}>"));
                write_html_inlines(content, out);
                out.push_str(&format!("</h{level}>"));
            }
            Block::Paragraph(inlines) => {
                out.push_str("<p>");
                write_html_inlines(inlines, out);
                out.push_str("</p>");
            }
            Block::CodeBlock { language, code } => {
                out.push_str("<pre><code");
                if let Some(language) = language {
                    out.push_str(&format!(" class=\"language-{}\"", escape_html(language)));
                }
                out.push('>');
                out.push_str(&escape_html(code));
                out.push_str("</code></pre>");
            }
            Block::Quote(inner) => {
                out.push_str("<blockquote>");
                write_html_blocks(inner, out);
                out.push_str("</blockquote>");
            }
            Block::List {
                ordered,
                start,
                items,
            } => {
                if *ordered {
                    if *start > 1 {
                        out.push_str(&format!("<ol start=\"{start}\">"));
                    } else {
                        out.push_str("<ol>");
                    }
                } else {
                    out.push_str("<ul>");
                }
                for item in items {
                    out.push_str("<li>");
                    write_html_inlines(item, out);
                    out.push_str("</li>");
                }
                out.push_str(if *ordered { "</ol>" } else { "</ul>" });
            }
            Block::Rule => out.push_str("<hr>"),
            Block::Table { header, rows } => {
                out.push_str("<table><thead><tr>");
                for cell in header {
                    out.push_str("<th>");
                    write_html_inlines(cell, out);
                    out.push_str("</th>");
                }
                out.push_str("</tr></thead><tbody>");
                for row in rows {
                    out.push_str("<tr>");
                    for cell in row {
                        out.push_str("<td>");
                        write_html_inlines(cell, out);
                        out.push_str("</td>");
                    }
                    out.push_str("</tr>");
                }
                out.push_str("</tbody></table>");
            }
        }
    }
}

fn write_html_inlines(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Text(t) => out.push_str(&escape_html(t)),
            Inline::Code(t) => {
                out.push_str("<code>");
                out.push_str(&escape_html(t));
                out.push_str("</code>");
            }
            Inline::Bold(x) => {
                out.push_str("<strong>");
                write_html_inlines(x, out);
                out.push_str("</strong>");
            }
            Inline::Italic(x) => {
                out.push_str("<em>");
                write_html_inlines(x, out);
                out.push_str("</em>");
            }
            Inline::Strike(x) => {
                out.push_str("<del>");
                write_html_inlines(x, out);
                out.push_str("</del>");
            }
            Inline::Link { text, url } => {
                // Drop unsafe schemes so a model-supplied link cannot execute
                // script in the renderer's browser.
                if let Some(href) = safe_url(url) {
                    out.push_str("<a href=\"");
                    out.push_str(&escape_html(&href));
                    out.push_str("\" target=\"_blank\" rel=\"noopener noreferrer\">");
                    write_html_inlines(text, out);
                    out.push_str("</a>");
                } else {
                    write_html_inlines(text, out);
                }
            }
        }
    }
}

/// Allow http(s), mailto, tel, anchors and relative links; reject anything that
/// could be a script URL.
fn safe_url(url: &str) -> Option<String> {
    let candidate = url.trim();
    if candidate.is_empty() {
        return None;
    }
    let normalized: String = candidate
        .to_ascii_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '\u{0}')
        .collect();
    const BLOCKED: [&str; 4] = ["javascript:", "data:", "vbscript:", "file:"];
    if BLOCKED.iter().any(|b| normalized.starts_with(b)) {
        return None;
    }
    Some(candidate.to_string())
}

fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headings_carry_their_level() {
        let doc = parse("# One\n\n### Three");
        assert_eq!(
            doc,
            vec![
                Block::Heading {
                    level: 1,
                    content: vec![Inline::Text("One".into())]
                },
                Block::Heading {
                    level: 3,
                    content: vec![Inline::Text("Three".into())]
                },
            ]
        );
    }

    #[test]
    fn hash_without_space_is_not_a_heading() {
        assert_eq!(parse("#hashtag").len(), 1);
        assert!(matches!(parse("#hashtag")[0], Block::Paragraph(_)));
    }

    #[test]
    fn fenced_code_keeps_lines_and_language() {
        let doc = parse("```rust\nfn main() {}\nlet x = 1;\n```");
        assert_eq!(
            doc,
            vec![Block::CodeBlock {
                language: Some("rust".into()),
                code: "fn main() {}\nlet x = 1;".into()
            }]
        );
    }

    #[test]
    fn inline_emphasis_code_and_strikethrough() {
        let inlines = parse_inlines("a **bold** and *it* and `code` and ~~no~~");
        assert_eq!(
            inlines,
            vec![
                Inline::Text("a ".into()),
                Inline::Bold(vec![Inline::Text("bold".into())]),
                Inline::Text(" and ".into()),
                Inline::Italic(vec![Inline::Text("it".into())]),
                Inline::Text(" and ".into()),
                Inline::Code("code".into()),
                Inline::Text(" and ".into()),
                Inline::Strike(vec![Inline::Text("no".into())]),
            ]
        );
    }

    #[test]
    fn underscores_inside_words_stay_literal() {
        assert_eq!(
            parse_inlines("snake_case_name"),
            vec![Inline::Text("snake_case_name".into())]
        );
    }

    #[test]
    fn links_are_parsed_with_their_target() {
        assert_eq!(
            parse_inlines("see [docs](https://example.com)"),
            vec![
                Inline::Text("see ".into()),
                Inline::Link {
                    text: vec![Inline::Text("docs".into())],
                    url: "https://example.com".into()
                },
            ]
        );
    }

    #[test]
    fn lists_parse_items_and_numbering() {
        assert_eq!(
            parse("- a\n- b"),
            vec![Block::List {
                ordered: false,
                start: 1,
                items: vec![
                    vec![Inline::Text("a".into())],
                    vec![Inline::Text("b".into())]
                ]
            }]
        );
        assert_eq!(
            parse("3. c"),
            vec![Block::List {
                ordered: true,
                start: 3,
                items: vec![vec![Inline::Text("c".into())]]
            }]
        );
    }

    #[test]
    fn blockquotes_are_nested_blocks() {
        assert_eq!(
            parse("> quoted **text**"),
            vec![Block::Quote(vec![Block::Paragraph(vec![
                Inline::Text("quoted ".into()),
                Inline::Bold(vec![Inline::Text("text".into())]),
            ])])]
        );
    }

    #[test]
    fn thematic_break_is_not_a_list() {
        assert_eq!(parse("---"), vec![Block::Rule]);
        assert_eq!(parse("* * *"), vec![Block::Rule]);
    }

    #[test]
    fn tables_parse_header_and_rows() {
        let doc = parse("| a | b |\n| --- | --- |\n| 1 | 2 |");
        assert_eq!(
            doc,
            vec![Block::Table {
                header: vec![
                    vec![Inline::Text("a".into())],
                    vec![Inline::Text("b".into())]
                ],
                rows: vec![vec![
                    vec![Inline::Text("1".into())],
                    vec![Inline::Text("2".into())]
                ]]
            }]
        );
    }

    #[test]
    fn html_escapes_text_and_code() {
        let html = to_html("a <b> & `x < y`");
        assert!(html.contains("a &lt;b&gt; &amp;"));
        assert!(html.contains("<code>x &lt; y</code>"));
    }

    #[test]
    fn html_renders_structure() {
        let html = to_html("# Title\n\n- one\n- two\n\n> note");
        assert!(html.contains("<h1>Title</h1>"));
        assert!(html.contains("<ul><li>one</li><li>two</li></ul>"));
        assert!(html.contains("<blockquote><p>note</p></blockquote>"));
    }

    #[test]
    fn unsafe_link_targets_are_dropped_but_text_kept() {
        let html = to_html("[click](javascript:alert(1))");
        assert!(!html.contains("javascript:"));
        assert!(html.contains("click"));
    }

    #[test]
    fn inline_text_strips_markup() {
        assert_eq!(
            inline_text(&parse_inlines("**bold** and `code`")),
            "bold and code"
        );
    }
}
