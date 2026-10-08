//! Append-only store over `session.csv`.

use std::path::{Path, PathBuf};

use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use crate::csv;
use crate::event::Event;

/// A single-file, append-only session log.
///
/// Writes are serialized by an internal mutex so concurrent appends (e.g. a
/// TUI and a server sharing the same file) never interleave a record.
pub struct SessionStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl SessionStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        SessionStore {
            path: path.into(),
            lock: Mutex::new(()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one event as a CSV record terminated by `\n`.
    pub async fn append(&self, event: &Event) -> std::io::Result<()> {
        let _guard = self.lock.lock().await;
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent).await.ok();
            }
        }
        let mut line = event.to_record();
        line.push('\n');
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await?;
        file.write_all(line.as_bytes()).await?;
        file.flush().await?;
        Ok(())
    }

    /// Read and parse every event in the file. Missing file yields an empty log.
    pub async fn read_all(&self) -> std::io::Result<Vec<Event>> {
        match tokio::fs::read_to_string(&self.path).await {
            Ok(content) => Ok(parse(&content)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    /// Whether the log already contains at least one event.
    pub async fn is_empty(&self) -> bool {
        self.read_all().await.map(|e| e.is_empty()).unwrap_or(true)
    }
}

/// Parse raw CSV content into events, skipping malformed rows.
pub fn parse(content: &str) -> Vec<Event> {
    csv::parse_records(content)
        .iter()
        .filter_map(|rec| Event::from_record(rec))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Status;

    #[tokio::test]
    async fn append_and_read() {
        let dir = std::env::temp_dir().join(format!("masse-test-{}", std::process::id()));
        let _ = tokio::fs::remove_file(&dir).await;
        let store = SessionStore::new(&dir);
        assert!(store.is_empty().await);
        store.append(&Event::session_start()).await.unwrap();
        store
            .append(&Event::user_message("hi, there"))
            .await
            .unwrap();
        store
            .append(&Event::tool_result("curl", "", Status::Ok, "ok\n", Some(0)))
            .await
            .unwrap();
        let events = store.read_all().await.unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[1].output, "hi, there");
        assert_eq!(events[2].exit_code_value(), Some(0));
        let _ = tokio::fs::remove_file(&dir).await;
    }
}
