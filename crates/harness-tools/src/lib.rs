//! Atomic tool implementations for the masse harness.
//!
//! Design rules, enforced throughout this crate:
//!
//! * Every tool does exactly one thing. There is no generic shell tool.
//! * Tools that need an external binary invoke it directly through
//!   [`tokio::process::Command`] with an argument **array**. `sh -c` is never
//!   used and arguments are never concatenated into a shell string.
//! * File and directory operations use `tokio::fs` natively.
//! * Every path is validated against the project working directory before use.
//! * External commands return a structured `{ stdout, stderr, exit_code }`.
//! * Output is filtered and truncated in Rust, never through shell pipes.

pub mod bialet_tools;
pub mod cli;
pub mod control_tools;
pub mod curl_tools;
pub mod fs_tools;
pub mod git_tools;
pub mod path;
pub mod process;
pub mod registry;
pub mod search_tools;
pub mod skill_tools;
pub mod sqlite_tools;
pub mod tool;

pub use registry::ToolRegistry;
pub use tool::{Tool, ToolContext, ToolResult, ToolStatus};

/// Build a registry with every built-in tool registered.
pub fn builtin_registry() -> ToolRegistry {
    let mut reg = ToolRegistry::new();

    // Files
    reg.register(std::sync::Arc::new(fs_tools::ReadFile));
    reg.register(std::sync::Arc::new(fs_tools::WriteFile));
    reg.register(std::sync::Arc::new(fs_tools::EditFile));
    reg.register(std::sync::Arc::new(fs_tools::DeleteFile));
    // Directories
    reg.register(std::sync::Arc::new(fs_tools::CreateDir));
    reg.register(std::sync::Arc::new(fs_tools::DeleteDir));
    reg.register(std::sync::Arc::new(fs_tools::ListDir));
    // Search
    reg.register(std::sync::Arc::new(search_tools::Grep));
    reg.register(std::sync::Arc::new(search_tools::FindGlob));
    // Git
    reg.register(std::sync::Arc::new(git_tools::GitStatus));
    reg.register(std::sync::Arc::new(git_tools::GitDiff));
    reg.register(std::sync::Arc::new(git_tools::GitLog));
    reg.register(std::sync::Arc::new(git_tools::GitCommit));
    reg.register(std::sync::Arc::new(git_tools::GitBranch));
    reg.register(std::sync::Arc::new(git_tools::GitStage));
    // SQLite
    reg.register(std::sync::Arc::new(sqlite_tools::SqliteQuery));
    reg.register(std::sync::Arc::new(sqlite_tools::SqliteExecute));
    // Bialet
    reg.register(std::sync::Arc::new(bialet_tools::BialetRun));
    reg.register(std::sync::Arc::new(bialet_tools::BialetValidate));
    reg.register(std::sync::Arc::new(bialet_tools::BialetTest));
    reg.register(std::sync::Arc::new(bialet_tools::BialetServerStart));
    reg.register(std::sync::Arc::new(bialet_tools::BialetServerStop));
    reg.register(std::sync::Arc::new(bialet_tools::BialetLogRead));
    reg.register(std::sync::Arc::new(bialet_tools::BialetDocs));
    // Web
    reg.register(std::sync::Arc::new(curl_tools::Curl));
    // Control
    reg.register(std::sync::Arc::new(control_tools::Finish));

    reg
}
