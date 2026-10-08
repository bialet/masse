//! Project context: `AGENTS.md`, `SKILL.md` skills and the Bialet docs root.
//!
//! Loading this once at startup lets the runtime inject repository instructions
//! into the system prompt and expose each skill as a callable tool.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use harness_tools::skill_tools::Skill;

/// Everything the harness learns from the project tree at startup.
#[derive(Clone, Debug, Default)]
pub struct ProjectContext {
    pub root: PathBuf,
    /// Contents of the repository `AGENTS.md`, when present.
    pub agents_md: Option<String>,
    /// Skills discovered under `skills/<name>/SKILL.md`.
    pub skills: Vec<Skill>,
    /// Where the local Bialet/Wren documentation lives.
    pub docs_dir: Option<PathBuf>,
}

impl ProjectContext {
    /// Load context from `root`, optionally overriding the docs directory.
    pub async fn load(root: &Path, docs_override: Option<PathBuf>) -> Self {
        let agents_md = tokio::fs::read_to_string(root.join("AGENTS.md")).await.ok();
        let docs_dir = docs_override
            .filter(|p| p.is_dir())
            .or_else(|| detect_docs(root));
        let skills = load_skills(root).await;
        ProjectContext {
            root: root.to_path_buf(),
            agents_md,
            skills,
            docs_dir,
        }
    }

    pub fn skill_names(&self) -> Vec<String> {
        self.skills.iter().map(|s| s.name.clone()).collect()
    }
}

/// Look for a Bialet docs directory in the usual sibling locations.
pub fn detect_docs(root: &Path) -> Option<PathBuf> {
    let candidates = [
        "bialet/docs",
        "../bialet/docs",
        "../../bialet/docs",
        "docs",
        "../docs",
        "vendor/bialet/docs",
    ];
    for candidate in candidates {
        let path = root.join(candidate);
        if looks_like_docs(&path) {
            return Some(path);
        }
    }
    None
}

fn looks_like_docs(path: &Path) -> bool {
    if !path.is_dir() {
        return false;
    }
    if path.join("index.rst").is_file() || path.join("usage.md").is_file() {
        return true;
    }
    // Fall back: any markdown/rst file means it is probably documentation.
    std::fs::read_dir(path)
        .ok()
        .map(|mut rd| {
            rd.any(|e| {
                e.ok().map(|e| {
                    matches!(
                        e.path().extension().and_then(|x| x.to_str()),
                        Some("md") | Some("rst")
                    )
                }) == Some(true)
            })
        })
        .unwrap_or(false)
}

/// Scan `skills/<name>/SKILL.md` and load every skill.
pub async fn load_skills(root: &Path) -> Vec<Skill> {
    let skills_dir = root.join("skills");
    let mut out = Vec::new();
    let Ok(mut rd) = tokio::fs::read_dir(&skills_dir).await else {
        return out;
    };
    while let Ok(Some(entry)) = rd.next_entry().await {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let skill_file = path.join("SKILL.md");
        let Ok(content) = tokio::fs::read_to_string(&skill_file).await else {
            continue;
        };
        let dir_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        out.push(parse_skill(&dir_name, &skill_file, &content));
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Parse a `SKILL.md` file, falling back to sensible defaults when the YAML
/// frontmatter is missing or incomplete.
pub fn parse_skill(dir_name: &str, path: &Path, content: &str) -> Skill {
    let (front, body) = split_frontmatter(content);
    let mut name = String::new();
    let mut description = String::new();
    if let Some(front) = front {
        let map = parse_frontmatter_keys(&front);
        if let Some(v) = map.get("name") {
            name = v.clone();
        }
        if let Some(v) = map.get("description") {
            description = v.clone();
        }
    }
    if name.trim().is_empty() {
        name = dir_name.replace(' ', "-").to_lowercase();
    }
    let body_trimmed = body.trim();
    if description.trim().is_empty() {
        description = first_paragraph(body_trimmed).unwrap_or_else(|| name.clone());
    }
    Skill {
        name,
        description,
        body: body_trimmed.to_string(),
        path: path.display().to_string(),
    }
}

/// Split optional YAML frontmatter from the body.
pub fn split_frontmatter(content: &str) -> (Option<String>, String) {
    let text = content.strip_prefix('\u{feff}').unwrap_or(content);
    let first_nl = match text.find('\n') {
        Some(i) => i,
        None => return (None, text.to_string()),
    };
    if text[..first_nl].trim() != "---" {
        return (None, text.to_string());
    }
    let rest = &text[first_nl + 1..];
    let mut pos = 0usize;
    for line in rest.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\r', '\n']).trim();
        if trimmed == "---" || trimmed == "..." {
            let front = rest[..pos].to_string();
            let body = rest[pos + line.len()..].to_string();
            return (Some(front), body);
        }
        pos += line.len();
    }
    (None, text.to_string())
}

fn parse_frontmatter_keys(front: &str) -> HashMap<String, String> {
    let lines: Vec<&str> = front.lines().collect();
    let mut map = HashMap::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let Some((key, raw_val)) = line.split_once(':') else {
            i += 1;
            continue;
        };
        let key = key.trim();
        if key.is_empty() || key.contains(' ') {
            i += 1;
            continue;
        }
        let val = raw_val.trim();
        let is_block = matches!(val, "|" | "|-" | "|+" | ">" | ">-" | ">+");
        if is_block {
            i += 1;
            let mut block: Vec<String> = Vec::new();
            let separator = if val.starts_with('>') { " " } else { "\n" };
            while i < lines.len() {
                let l = lines[i];
                if l.trim().is_empty() {
                    block.push(String::new());
                    i += 1;
                    continue;
                }
                let indent = l.len() - l.trim_start().len();
                if indent == 0 {
                    break;
                }
                block.push(l.trim().to_string());
                i += 1;
            }
            let joined = block
                .iter()
                .map(|s| s.trim())
                .collect::<Vec<_>>()
                .join(separator)
                .trim()
                .to_string();
            map.insert(key.to_string(), joined);
            continue;
        }
        if val.is_empty() {
            // Nested mapping or empty value: skip the indented block.
            i += 1;
            while i < lines.len() {
                let l = lines[i];
                if l.trim().is_empty() {
                    i += 1;
                    continue;
                }
                let indent = l.len() - l.trim_start().len();
                if indent == 0 {
                    break;
                }
                i += 1;
            }
            continue;
        }
        let cleaned = val.trim_matches('"').trim_matches('\'').trim().to_string();
        map.insert(key.to_string(), cleaned);
        i += 1;
    }
    map
}

/// First non-empty paragraph of a markdown document, collapsed to one line.
pub fn first_paragraph(body: &str) -> Option<String> {
    let mut para = String::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !para.is_empty() {
                break;
            }
            continue;
        }
        if trimmed.starts_with('#') {
            continue;
        }
        if !para.is_empty() {
            para.push(' ');
        }
        para.push_str(trimmed);
    }
    if para.is_empty() {
        None
    } else {
        Some(para)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_block_description() {
        let content = "---\nname: demo\ndescription: |-\n  First line.\n  Second line.\n---\n\n# Body\nHello\n";
        let (front, body) = split_frontmatter(content);
        assert!(front.is_some());
        assert!(body.contains("# Body"));
        let map = parse_frontmatter_keys(&front.unwrap());
        assert_eq!(map.get("name").unwrap(), "demo");
        assert_eq!(map.get("description").unwrap(), "First line.\nSecond line.");
    }

    #[test]
    fn falls_back_without_frontmatter() {
        let content = "# Title\n\nA short description here.\n";
        let (front, body) = split_frontmatter(content);
        assert!(front.is_none());
        assert_eq!(first_paragraph(&body).unwrap(), "A short description here.");
    }
}
