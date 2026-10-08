//! Direct, shell-free subprocess execution.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::Command;

/// The structured result of running an external binary.
#[derive(Clone, Debug, Default)]
pub struct CmdOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    /// Populated when the process could not be spawned at all.
    pub error: Option<String>,
}

impl CmdOutput {
    pub fn success(&self) -> bool {
        self.error.is_none() && !self.timed_out && self.exit_code == Some(0)
    }

    /// Render the structured result as the text handed back to the agent.
    pub fn to_agent_text(&self) -> String {
        let mut out = String::new();
        if let Some(err) = &self.error {
            out.push_str(&format!("error: {err}\n"));
            return out;
        }
        match self.exit_code {
            Some(code) => out.push_str(&format!("exit_code: {code}\n")),
            None => out.push_str("exit_code: (none)\n"),
        }
        if self.timed_out {
            out.push_str("timed_out: true\n");
        }
        if !self.stdout.is_empty() {
            out.push_str("stdout:\n");
            out.push_str(&self.stdout);
            if !self.stdout.ends_with('\n') {
                out.push('\n');
            }
        }
        if !self.stderr.is_empty() {
            out.push_str("stderr:\n");
            out.push_str(&self.stderr);
            if !self.stderr.ends_with('\n') {
                out.push('\n');
            }
        }
        out
    }
}

/// The program to run plus its argument array (never a shell string).
#[derive(Clone, Debug)]
pub struct CmdSpec {
    pub program: String,
    pub args: Vec<String>,
}

impl CmdSpec {
    pub fn new(program: impl Into<String>) -> Self {
        CmdSpec {
            program: program.into(),
            args: Vec::new(),
        }
    }

    pub fn arg(mut self, value: impl Into<String>) -> Self {
        self.args.push(value.into());
        self
    }

    pub fn args<I, S>(mut self, values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(values.into_iter().map(Into::into));
        self
    }

    pub fn display(&self) -> String {
        let mut s = self.program.clone();
        for a in &self.args {
            s.push(' ');
            if a.contains(' ') || a.contains('"') {
                s.push_str(&format!("{a:?}"));
            } else {
                s.push_str(a);
            }
        }
        s
    }
}

/// Run a command to completion, killing it if it exceeds `timeout`.
///
/// `stdin` is closed, stdout/stderr are captured. Output is read concurrently
/// so a chatty process cannot deadlock on a full pipe.
pub async fn run(spec: &CmdSpec, cwd: &Path, timeout: Duration) -> CmdOutput {
    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return CmdOutput {
                error: Some(format!("failed to spawn `{}`: {e}", spec.program)),
                ..Default::default()
            }
        }
    };

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let out_task = tokio::spawn(async move { drain(stdout).await });
    let err_task = tokio::spawn(async move { drain(stderr).await });

    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => {
            let stdout = out_task.await.unwrap_or_default();
            let stderr = err_task.await.unwrap_or_default();
            CmdOutput {
                stdout,
                stderr,
                exit_code: status.code(),
                timed_out: false,
                error: None,
            }
        }
        Ok(Err(e)) => {
            let _ = child.kill().await;
            CmdOutput {
                stdout: out_task.await.unwrap_or_default(),
                stderr: err_task.await.unwrap_or_default(),
                exit_code: None,
                timed_out: false,
                error: Some(format!("failed to wait for `{}`: {e}", spec.program)),
            }
        }
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            let mut stdout = out_task.await.unwrap_or_default();
            let mut stderr = err_task.await.unwrap_or_default();
            if !stderr.is_empty() && !stderr.ends_with('\n') {
                stderr.push('\n');
            }
            stderr.push_str(&format!(
                "[masse] process killed after {}s timeout",
                timeout.as_secs()
            ));
            if !stdout.is_empty() {
                stdout.push('\n');
            }
            CmdOutput {
                stdout,
                stderr,
                exit_code: None,
                timed_out: true,
                error: None,
            }
        }
    }
}

async fn drain<R>(reader: Option<R>) -> String
where
    R: AsyncReadExt + Unpin,
{
    let Some(mut r) = reader else {
        return String::new();
    };
    let mut buf = Vec::new();
    let _ = r.read_to_end(&mut buf).await;
    String::from_utf8_lossy(&buf).into_owned()
}

/// Truncate captured text to a maximum number of lines, appending a marker.
pub fn truncate_lines(text: &str, max_lines: usize) -> (String, bool) {
    if max_lines == 0 || text.is_empty() {
        return (text.to_string(), false);
    }
    let mut line_count = 0usize;
    for (idx, ch) in text.char_indices() {
        if ch == '\n' {
            line_count += 1;
            if line_count == max_lines {
                let mut out = text[..idx].to_string();
                out.push_str(&format!("\n[masse] output truncated at {max_lines} lines"));
                return (out, true);
            }
        }
    }
    (text.to_string(), false)
}
