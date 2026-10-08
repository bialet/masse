//! In-memory registry of background processes.
//!
//! Background processes (e.g. a Bialet dev server started by
//! `bialet_server_start`) live in the runtime memory of masse. They are never
//! recorded on disk: restarting masse forgets them, which is intentional.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::process::{Child, Command};
use tokio::sync::Mutex;

/// A running background process.
struct Managed {
    child: Child,
    pid: Option<u32>,
    display: String,
    log_file: Option<String>,
}

/// Metadata about a running process, safe to serialize.
#[derive(Clone, Debug, serde::Serialize)]
pub struct ProcessInfo {
    pub key: String,
    pub pid: Option<u32>,
    pub display: String,
    pub log_file: Option<String>,
}

#[derive(Default)]
struct Inner {
    procs: HashMap<String, Managed>,
}

/// Holds child handles for background processes, keyed by a logical name.
#[derive(Clone, Default)]
pub struct ProcessRegistry {
    inner: Arc<Mutex<Inner>>,
}

impl ProcessRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Spawn `program args...` in `cwd`, redirecting output to `log_file` when
    /// provided. Returns the PID.
    pub async fn start(
        &self,
        key: &str,
        program: &str,
        args: &[String],
        cwd: &Path,
        log_file: Option<&Path>,
    ) -> Result<u32, String> {
        let mut guard = self.inner.lock().await;
        if let Some(existing) = guard.procs.get_mut(key) {
            // Already running? Report the existing PID instead of leaking a new one.
            if matches!(existing.child.try_wait(), Ok(None)) {
                return Err(format!(
                    "process '{key}' is already running (pid {:?}); stop it first",
                    existing.pid
                ));
            }
            guard.procs.remove(key);
        }

        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .kill_on_drop(false);

        if let Some(path) = log_file {
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await.ok();
            }
            let stdout = std::fs::File::create(path)
                .map_err(|e| format!("cannot create log file {}: {e}", path.display()))?;
            let stderr = stdout
                .try_clone()
                .map_err(|e| format!("cannot clone log file handle: {e}"))?;
            cmd.stdout(Stdio::from(stdout));
            cmd.stderr(Stdio::from(stderr));
        } else {
            cmd.stdout(Stdio::null()).stderr(Stdio::null());
        }

        let child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn `{program}`: {e}"))?;
        let pid = child.id();
        let display = {
            let mut s = program.to_string();
            for a in args {
                s.push(' ');
                s.push_str(a);
            }
            s
        };
        guard.procs.insert(
            key.to_string(),
            Managed {
                child,
                pid,
                display,
                log_file: log_file.map(|p| p.display().to_string()),
            },
        );
        Ok(pid.unwrap_or(0))
    }

    /// Kill a background process by key. Returns true if it was running.
    pub async fn stop(&self, key: &str) -> bool {
        let mut guard = self.inner.lock().await;
        let Some(mut managed) = guard.procs.remove(key) else {
            return false;
        };
        let _ = managed.child.start_kill();
        // Give it a moment to die, then wait so the PID is reaped.
        let _ = tokio::time::timeout(Duration::from_secs(5), managed.child.wait()).await;
        true
    }

    /// Whether a process with `key` is still alive.
    pub async fn is_running(&self, key: &str) -> bool {
        let mut guard = self.inner.lock().await;
        match guard.procs.get_mut(key) {
            Some(m) => matches!(m.child.try_wait(), Ok(None)),
            None => false,
        }
    }

    pub async fn list(&self) -> Vec<ProcessInfo> {
        let mut guard = self.inner.lock().await;
        let mut out: Vec<ProcessInfo> = guard
            .procs
            .iter_mut()
            .map(|(k, m)| {
                let alive = matches!(m.child.try_wait(), Ok(None));
                ProcessInfo {
                    key: k.clone(),
                    pid: if alive { m.pid } else { None },
                    display: m.display.clone(),
                    log_file: m.log_file.clone(),
                }
            })
            .collect();
        out.sort_by(|a, b| a.key.cmp(&b.key));
        out
    }

    /// Kill every tracked process. Called on shutdown.
    pub async fn stop_all(&self) {
        let mut guard = self.inner.lock().await;
        for (_, mut m) in guard.procs.drain() {
            let _ = m.child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(3), m.child.wait()).await;
        }
    }
}
