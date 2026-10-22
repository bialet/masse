//! Named sessions: one directory per conversation.
//!
//! Layout under the workspace:
//!
//! ```text
//! .masse/current                      # active session id, one line
//! .masse/sessions/<id>/session.csv    # one conversation per directory
//! .masse/archive/<id>/session.csv     # archived conversations, never deleted
//! ```
//!
//! There is no index and no metadata file: the title, timestamps and event
//! count are derived from the CSV, so the log stays the only source of truth.
//! Nothing here rewrites or truncates a log — `archive` only moves the
//! directory — because the full conversation is the debugging and abuse trail.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use chrono::{TimeZone, Utc};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::event::{Event, EventType};
use crate::store::SessionStore;

/// Directory (under `.masse/`) that holds one subdirectory per session.
pub const SESSIONS_DIR: &str = "sessions";
/// Pointer file with the active session id.
pub const CURRENT_FILE: &str = "current";
/// Where archived sessions are moved instead of being deleted.
pub const ARCHIVE_DIR: &str = "archive";
/// File name of the log inside a session directory.
pub const LOG_FILE: &str = "session.csv";

const TITLE_MAX_CHARS: usize = 60;
const ID_MAX_CHARS: usize = 64;

/// A summary of one session, derived by reading its log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionInfo {
    pub id: String,
    /// Explicit title, or the first user message truncated.
    pub title: String,
    /// Timestamp of the first event, empty for an unused session.
    pub created: String,
    /// Timestamp of the most recent event.
    pub updated: String,
    pub events: usize,
    pub path: PathBuf,
}

impl SessionInfo {
    /// A store for this session's log.
    pub fn store(&self) -> Arc<SessionStore> {
        Arc::new(SessionStore::new(self.path.clone()))
    }
}

struct Active {
    id: String,
    store: Arc<SessionStore>,
}

/// Resolves, creates and switches named sessions.
///
/// One manager is shared by the agent, the TUI and the server, so all three
/// agree on which log is active. Methods that change the active session also
/// update `current`, so a restart resumes the same conversation.
pub struct SessionManager {
    root: PathBuf,
    dir: PathBuf,
    current: PathBuf,
    archive: PathBuf,
    active: RwLock<Active>,
}

impl SessionManager {
    /// Open the session store for a workspace.
    ///
    /// `explicit` keeps the old `--session <path>` escape hatch: the given CSV
    /// becomes the active log and is not managed under `.masse/sessions/`.
    /// Otherwise the active session comes from `.masse/current`; when that is
    /// missing the legacy `.masse/session.csv` is migrated, then the newest
    /// session is resumed, and only then is a fresh id minted (without writing
    /// anything: the directory appears on the first event).
    pub async fn load(root: impl Into<PathBuf>, explicit: Option<PathBuf>) -> Result<Arc<Self>> {
        let root = root.into();
        let base = root.join(".masse");
        let dir = base.join(SESSIONS_DIR);
        tokio::fs::create_dir_all(&dir)
            .await
            .with_context(|| format!("cannot create {}", dir.display()))?;

        let manager = Arc::new(SessionManager {
            root,
            dir,
            current: base.join(CURRENT_FILE),
            archive: base.join(ARCHIVE_DIR),
            active: RwLock::new(Active {
                id: String::new(),
                store: Arc::new(SessionStore::new(PathBuf::new())),
            }),
        });

        if let Some(path) = explicit {
            manager.set_active("explicit", path).await?;
            return Ok(manager);
        }

        let id = match manager.read_current().await {
            Some(id) => id,
            None => match manager.migrate_legacy().await? {
                Some(id) => id,
                None => manager.latest_id().await.unwrap_or_else(new_id),
            },
        };
        manager.set_active(&id, manager.session_path(&id)).await?;
        manager.write_current(&id).await?;
        Ok(manager)
    }

    /// The directory that holds every managed session.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Path of a session's log.
    pub fn session_path(&self, id: &str) -> PathBuf {
        self.dir.join(id).join(LOG_FILE)
    }

    /// The active log.
    pub async fn active_store(&self) -> Arc<SessionStore> {
        self.active.read().await.store.clone()
    }

    /// Path of the active log.
    pub async fn active_path(&self) -> PathBuf {
        self.active_store().await.path().to_path_buf()
    }

    pub async fn active_id(&self) -> String {
        self.active.read().await.id.clone()
    }

    /// Everything known about the active session.
    pub async fn active_info(&self) -> Result<SessionInfo> {
        let active = self.active.read().await;
        let events = active.store.read_all().await?;
        Ok(info_from(
            &active.id,
            active.store.path().to_path_buf(),
            &events,
        ))
    }

    /// Create a fresh session and make it active.
    ///
    /// Without a title nothing is written yet: an unused session leaves no
    /// directory, so the picker does not fill up with empty entries.
    pub async fn create(&self, title: Option<&str>) -> Result<SessionInfo> {
        let mut id = new_id();
        while self.dir.join(&id).exists() {
            id = new_id();
        }
        let path = self.session_path(&id);
        let store = Arc::new(SessionStore::new(path.clone()));
        if let Some(title) = title.map(str::trim).filter(|t| !t.is_empty()) {
            store.append(&Event::session_start()).await?;
            store.append(&Event::session_title(title)).await?;
        }
        self.write_current(&id).await?;
        *self.active.write().await = Active {
            id: id.clone(),
            store,
        };
        self.info(&id).await
    }

    /// Make an existing session active. `"last"` selects the newest one.
    pub async fn open(&self, selector: &str) -> Result<SessionInfo> {
        let id = if selector == "last" {
            self.latest_id()
                .await
                .context("there are no sessions yet")?
        } else {
            validate_id(selector)?
        };
        let path = self.session_path(&id);
        if !path.is_file() {
            // A session created a moment ago has no log until its first event.
            // Accept it only when it is already the active one: a typo must
            // not silently materialize a new session.
            if self.active_id().await != id {
                bail!("no such session: {id}");
            }
        }
        self.set_active(&id, path).await?;
        self.write_current(&id).await?;
        self.info(&id).await
    }

    /// Info for one managed session. A session that has not received its first
    /// event yet yields an empty entry rather than an error.
    pub async fn info(&self, id: &str) -> Result<SessionInfo> {
        let id = validate_id(id)?;
        let path = self.session_path(&id);
        let store = SessionStore::new(path.clone());
        let events = store.read_all().await?;
        Ok(info_from(&id, path, &events))
    }

    /// Every known session, most recently active first. The active session is
    /// included even before its first event, so a freshly created one is
    /// visible in the picker (it simply has no title yet).
    pub async fn list(&self) -> Vec<SessionInfo> {
        let mut out = Vec::new();
        let Ok(mut entries) = tokio::fs::read_dir(&self.dir).await else {
            return out;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if validate_id(&name).is_err() {
                continue;
            }
            let path = entry.path().join(LOG_FILE);
            let store = SessionStore::new(path.clone());
            let events = match store.read_all().await {
                Ok(events) if !events.is_empty() => events,
                _ => continue,
            };
            out.push(info_from(&name, path, &events));
        }
        let active = self.active_id().await;
        if !out.iter().any(|info| info.id == active) {
            if let Ok(info) = self.active_info().await {
                out.push(info);
            }
        }
        out.sort_by(|a, b| {
            let a_fresh = (a.id == active && a.events == 0) as u8;
            let b_fresh = (b.id == active && b.events == 0) as u8;
            b_fresh
                .cmp(&a_fresh)
                .then_with(|| b.updated.cmp(&a.updated))
                .then_with(|| b.id.cmp(&a.id))
        });
        out
    }

    /// Append a title event. The log is never rewritten, so the old title is
    /// still in the record.
    pub async fn rename(&self, id: &str, title: &str) -> Result<SessionInfo> {
        let title = title.trim();
        if title.is_empty() {
            bail!("the title cannot be empty");
        }
        let id = validate_id(id)?;
        let path = self.session_path(&id);
        if !path.is_file() {
            bail!("no such session: {id}");
        }
        let store = SessionStore::new(path);
        store.append(&Event::session_title(title)).await?;
        self.info(&id).await
    }

    /// Move a session out of the picker without deleting its log.
    ///
    /// The active session cannot be archived: switching first keeps the agent
    /// from writing into a directory that just moved.
    pub async fn archive(&self, id: &str) -> Result<PathBuf> {
        let id = validate_id(id)?;
        if self.active_id().await == id {
            bail!("cannot archive the active session; switch to another one first");
        }
        let src = self.dir.join(&id);
        if !src.is_dir() {
            bail!("no such session: {id}");
        }
        tokio::fs::create_dir_all(&self.archive)
            .await
            .with_context(|| format!("cannot create {}", self.archive.display()))?;
        let mut dest = self.archive.join(&id);
        if dest.exists() {
            dest = self
                .archive
                .join(format!("{}-{}", id, Utc::now().format("%Y%m%d-%H%M%S")));
        }
        tokio::fs::rename(&src, &dest)
            .await
            .with_context(|| format!("cannot archive session {id}"))?;
        Ok(dest)
    }

    async fn set_active(&self, id: &str, path: impl Into<PathBuf>) -> Result<()> {
        let store = Arc::new(SessionStore::new(path));
        *self.active.write().await = Active {
            id: id.to_string(),
            store,
        };
        Ok(())
    }

    async fn read_current(&self) -> Option<String> {
        let text = tokio::fs::read_to_string(&self.current).await.ok()?;
        let id = text.trim();
        validate_id(id).ok()
    }

    /// Write the pointer atomically so a crash never leaves half an id.
    async fn write_current(&self, id: &str) -> Result<()> {
        let tmp = self.current.with_extension("tmp");
        tokio::fs::write(&tmp, format!("{id}\n"))
            .await
            .with_context(|| format!("cannot write {}", tmp.display()))?;
        tokio::fs::rename(&tmp, &self.current)
            .await
            .with_context(|| format!("cannot replace {}", self.current.display()))?;
        Ok(())
    }

    /// Newest managed session by id (ids start with a UTC timestamp).
    /// Newest managed session by id (ids start with a UTC timestamp).
    async fn latest_id(&self) -> Option<String> {
        let mut entries = tokio::fs::read_dir(&self.dir).await.ok()?;
        let mut ids: Vec<String> = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if validate_id(&name).is_ok() && entry.path().join(LOG_FILE).is_file() {
                ids.push(name);
            }
        }
        ids.sort();
        ids.pop()
    }

    /// Move the old single-file `.masse/session.csv` into a session directory
    /// so an upgrade keeps the conversation.
    async fn migrate_legacy(&self) -> Result<Option<String>> {
        let legacy = self.root.join(".masse").join(LOG_FILE);
        let meta = match tokio::fs::metadata(&legacy).await {
            Ok(meta) => meta,
            Err(_) => return Ok(None),
        };
        if meta.len() == 0 {
            let _ = tokio::fs::remove_file(&legacy).await;
            return Ok(None);
        }
        let stamp = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .and_then(|d| Utc.timestamp_opt(d.as_secs() as i64, 0).single())
            .unwrap_or_else(Utc::now);
        let id = format!("{}-legacy", stamp.format("%Y%m%d-%H%M%S"));
        let dir = self.dir.join(&id);
        tokio::fs::create_dir_all(&dir)
            .await
            .with_context(|| format!("cannot create {}", dir.display()))?;
        let dest = dir.join(LOG_FILE);
        if let Err(rename) = tokio::fs::rename(&legacy, &dest).await {
            // Different filesystem, or the file is on a mount: copy instead.
            tokio::fs::copy(&legacy, &dest).await.with_context(|| {
                format!(
                    "cannot migrate {} to {} ({rename})",
                    legacy.display(),
                    dest.display()
                )
            })?;
            let _ = tokio::fs::remove_file(&legacy).await;
        }
        self.write_current(&id).await?;
        Ok(Some(id))
    }
}

fn info_from(id: &str, path: PathBuf, events: &[Event]) -> SessionInfo {
    SessionInfo {
        id: id.to_string(),
        title: derive_title(events),
        created: events
            .first()
            .map(|e| e.timestamp.clone())
            .unwrap_or_default(),
        updated: events
            .last()
            .map(|e| e.timestamp.clone())
            .unwrap_or_default(),
        events: events.len(),
        path,
    }
}

/// Last explicit title wins; otherwise the first user message stands in for
/// one, collapsed to a single line and truncated.
fn derive_title(events: &[Event]) -> String {
    if let Some(title) = events
        .iter()
        .rev()
        .find(|e| e.kind() == Some(EventType::SessionTitle))
    {
        return title.output.trim().to_string();
    }
    events
        .iter()
        .find(|e| e.kind() == Some(EventType::UserMessage))
        .map(|e| truncate(&collapse(&e.output), TITLE_MAX_CHARS))
        .unwrap_or_default()
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(text: &str, max_chars: usize) -> String {
    let mut out: String = text.chars().take(max_chars).collect();
    if text.chars().count() > max_chars {
        out.push('…');
    }
    out
}

/// A session id is a directory name and nothing else: reject separators,
/// dots and traversal before it ever reaches the filesystem.
fn validate_id(id: &str) -> Result<String> {
    let valid = !id.is_empty()
        && id.len() <= ID_MAX_CHARS
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if !valid {
        bail!("invalid session id `{id}`");
    }
    Ok(id.to_string())
}

fn new_id() -> String {
    let suffix = Uuid::new_v4().simple().to_string();
    // Milliseconds keep ids sortable even for two sessions created in the same
    // second; the random suffix keeps them unique across processes.
    format!("{}-{}", Utc::now().format("%Y%m%d-%H%M%S%3f"), &suffix[..4])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Status;

    fn temp_root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "masse-sessions-{tag}-{}-{}",
            std::process::id(),
            Uuid::new_v4().simple()
        ))
    }

    #[tokio::test]
    async fn unused_session_leaves_no_directory_until_the_first_event() {
        let root = temp_root("lazy");
        let manager = SessionManager::load(&root, None).await.unwrap();
        let id = manager.active_id().await;
        assert!(!manager.session_path(&id).exists());

        // The active session is listed (so the picker can show it) but has no
        // log on disk yet.
        let list = manager.list().await;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, id);
        assert_eq!(list[0].events, 0);

        manager
            .active_store()
            .await
            .append(&Event::user_message("build a guestbook"))
            .await
            .unwrap();
        let list = manager.list().await;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, id);
        assert_eq!(list[0].title, "build a guestbook");
        assert_eq!(list[0].events, 1);
        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    #[tokio::test]
    async fn create_switch_rename_and_archive_keep_full_logs() {
        let root = temp_root("lifecycle");
        let manager = SessionManager::load(&root, None).await.unwrap();

        let first = manager.create(Some("guestbook")).await.unwrap();
        assert_eq!(first.title, "guestbook");
        assert_eq!(manager.active_id().await, first.id);

        manager
            .active_store()
            .await
            .append(&Event::user_message("add a form"))
            .await
            .unwrap();
        manager
            .active_store()
            .await
            .append(&Event::assistant_message("done"))
            .await
            .unwrap();

        let second = manager.create(None).await.unwrap();
        assert_ne!(first.id, second.id);
        manager
            .active_store()
            .await
            .append(&Event::user_message("other work"))
            .await
            .unwrap();

        // Most recently active first.
        let list = manager.list().await;
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, second.id);

        // `last` picks the most recent.
        assert_eq!(manager.open("last").await.unwrap().id, second.id);
        assert_eq!(manager.open(&first.id).await.unwrap().title, "guestbook");

        let renamed = manager
            .rename(&first.id, "the guestbook app")
            .await
            .unwrap();
        assert_eq!(renamed.title, "the guestbook app");

        // The old title and every message are still in the log.
        let events = manager.active_store().await.read_all().await.unwrap();
        let all = events.iter().map(|e| e.output.as_str()).collect::<Vec<_>>();
        assert!(all.contains(&"build a guestbook") || all.contains(&"add a form"));
        assert!(all.contains(&"guestbook"));

        // Switch away first: the active session cannot be archived.
        manager.open(&second.id).await.unwrap();
        let dest = manager.archive(&first.id).await.unwrap();
        assert!(dest.join(LOG_FILE).is_file());
        assert_eq!(manager.list().await.len(), 1);
        // Archive never deletes.
        assert!(dest.exists());
        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    #[tokio::test]
    async fn the_active_session_cannot_be_archived() {
        let root = temp_root("active");
        let manager = SessionManager::load(&root, None).await.unwrap();
        let id = manager.create(Some("keep me")).await.unwrap().id;
        let err = manager.archive(&id).await.unwrap_err().to_string();
        assert!(err.contains("active"), "{err}");
        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    #[tokio::test]
    async fn a_freshly_created_session_can_be_opened_before_its_first_event() {
        let root = temp_root("fresh-open");
        let manager = SessionManager::load(&root, None).await.unwrap();
        let created = manager.create(None).await.unwrap();
        assert!(!created.path.exists(), "an unused session has no log yet");

        let opened = manager.open(&created.id).await.unwrap();
        assert_eq!(opened.id, created.id);
        assert_eq!(opened.events, 0);

        // An unknown id still fails instead of materializing a session.
        assert!(manager.open("20260101-000000-ffff").await.is_err());
        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    #[tokio::test]
    async fn ids_cannot_escape_the_sessions_directory() {
        let root = temp_root("ids");
        let manager = SessionManager::load(&root, None).await.unwrap();
        for bad in ["../etc", "a/b", ".", "", "x y"] {
            assert!(manager.open(bad).await.is_err(), "{bad:?} was accepted");
        }
        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    #[tokio::test]
    async fn the_old_single_file_is_migrated_once() {
        let root = temp_root("migrate");
        let base = root.join(".masse");
        tokio::fs::create_dir_all(&base).await.unwrap();
        let store = SessionStore::new(base.join(LOG_FILE));
        store.append(&Event::session_start()).await.unwrap();
        store
            .append(&Event::user_message("from the old layout"))
            .await
            .unwrap();

        let manager = SessionManager::load(&root, None).await.unwrap();
        assert!(!base.join(LOG_FILE).exists(), "legacy file was not moved");
        let list = manager.list().await;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].title, "from the old layout");
        assert!(list[0].id.ends_with("-legacy"));

        // Reopening resumes the migrated session instead of creating another.
        let again = SessionManager::load(&root, None).await.unwrap();
        assert_eq!(again.active_id().await, list[0].id);
        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    #[tokio::test]
    async fn an_explicit_file_is_used_as_the_active_log() {
        let root = temp_root("explicit");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let path = root.join("elsewhere.csv");
        let store = SessionStore::new(path.clone());
        store
            .append(&Event::tool_result("x", "", Status::Ok, "ok", Some(0)))
            .await
            .unwrap();

        let manager = SessionManager::load(&root, Some(path.clone()))
            .await
            .unwrap();
        assert_eq!(manager.active_path().await, path);
        assert_eq!(manager.active_id().await, "explicit");
        assert!(manager.active_info().await.unwrap().events == 1);
        let _ = tokio::fs::remove_dir_all(&root).await;
    }
}
