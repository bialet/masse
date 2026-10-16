//! Documentation bundled into the binary and served by the `bialet_docs` tool.
//!
//! The harness must stay useful with no network and no `docs/` directory on
//! disk, so a compact reference for the web stack (SQLite, HTML, CSS, JS) is
//! embedded at compile time. `bialet_docs` searches these alongside any local
//! docs, and local files of the same name win.

/// One bundled page: the doc's file name and its full contents.
#[derive(Clone, Copy, Debug)]
pub struct EmbeddedDoc {
    pub name: &'static str,
    pub content: &'static str,
}

/// Every page compiled into the binary.
pub const EMBEDDED_DOCS: &[EmbeddedDoc] = &[
    EmbeddedDoc {
        name: "sqlite.md",
        content: include_str!("../../../assets/docs/sqlite.md"),
    },
    EmbeddedDoc {
        name: "database.md",
        content: include_str!("../../../assets/docs/database.md"),
    },
    EmbeddedDoc {
        name: "html.md",
        content: include_str!("../../../assets/docs/html.md"),
    },
    EmbeddedDoc {
        name: "template.md",
        content: include_str!("../../../assets/docs/template.md"),
    },
    EmbeddedDoc {
        name: "html-first.md",
        content: include_str!("../../../assets/docs/html-first.md"),
    },
    EmbeddedDoc {
        name: "css.md",
        content: include_str!("../../../assets/docs/css.md"),
    },
    EmbeddedDoc {
        name: "javascript.md",
        content: include_str!("../../../assets/docs/javascript.md"),
    },
    EmbeddedDoc {
        name: "tests.md",
        content: include_str!("../../../assets/docs/tests.md"),
    },
];

/// Look up a bundled page by its file name.
pub fn get(name: &str) -> Option<&'static EmbeddedDoc> {
    EMBEDDED_DOCS.iter().find(|d| d.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_doc_is_present_and_non_empty() {
        assert_eq!(EMBEDDED_DOCS.len(), 8);
        for doc in EMBEDDED_DOCS {
            assert!(doc.content.contains('#'), "{} has no heading", doc.name);
        }
    }

    #[test]
    fn lookup_by_name() {
        assert!(get("sqlite.md").is_some());
        assert!(get("database.md").is_some());
        assert!(get("tests.md").is_some());
        assert!(get("nope.md").is_none());
    }

    /// Every `*.md` a bundled page points at must itself be bundled, so
    /// `bialet_docs file=<name>` never dead-ends the model.
    #[test]
    fn bundled_docs_have_no_dangling_references() {
        for doc in EMBEDDED_DOCS {
            for name in referenced_pages(doc.content) {
                assert!(
                    get(&name).is_some(),
                    "{} references {} but it is not bundled",
                    doc.name,
                    name
                );
            }
        }
    }

    /// Pull out `foo.md`-shaped names from backtick-quoted spans and prose.
    fn referenced_pages(content: &str) -> Vec<String> {
        let mut found = Vec::new();
        for word in content
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'))
        {
            if let Some(name) = word.strip_suffix(".md") {
                if !name.is_empty() && !name.contains('/') {
                    found.push(format!("{name}.md"));
                }
            }
        }
        found
    }
}
