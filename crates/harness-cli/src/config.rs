//! Persistent user configuration: `<user config dir>/masse/config`
//! (`~/Library/Application Support/masse/config` on macOS, `~/.config/masse/config`
//! on Linux, or whatever `MASSE_CONFIG_FILE` / `--config-file` points at).
//!
//! The file is a flat `KEY=VALUE` list parsed by hand: no dependency, easy to
//! read and edit, and deliberately **outside the workspace**. The harness's own
//! file tools are confined to the workspace root, so a key stored here is not
//! reachable by the model (nor copied into `session.csv` / `masse.log`).
//!
//! Two levels of keys:
//!
//! ```text
//! provider = deepseek        # top level: the profile to use by default
//! model = deepseek-chat      # top level: shared defaults
//! deepseek.api_key = sk-…    # per-profile values
//! openai.model = gpt-4o-mini
//! ```
//!
//! Precedence is resolved in `main.rs`; this module only reads, writes and
//! masks the file.

use std::collections::BTreeMap;
use std::env;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Subcommand;

/// Env override for the config file path.
pub const CONFIG_FILE_ENV: &str = "MASSE_CONFIG_FILE";
/// Env override for the config directory (mainly for tests and odd layouts).
pub const CONFIG_DIR_ENV: &str = "MASSE_CONFIG_DIR";

/// Keys understood inside a profile, plus the bare/top-level form.
const KEYS: [&str; 3] = ["base_url", "model", "api_key"];

const FILE_NAME: &str = "config";

/// Read or write the persistent configuration.
#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    /// Print the configuration file path.
    Path,
    /// Print the configuration file with secrets masked.
    Show,
    /// Set a key, for example `masse config set api_key sk-…` or
    /// `masse config set deepseek.model deepseek-chat`.
    Set {
        /// `provider`, `api_key`/`model`/`base_url`, or `<profile>.<key>`.
        key: String,
        /// The value. Quote it on the shell if it contains spaces.
        value: String,
    },
    /// Remove a key.
    Unset {
        /// Same key syntax as `set`.
        key: String,
    },
    /// Store the API key for the active (or `--profile`) provider. Omit the key
    /// to be prompted for it, which keeps it out of the shell history and off
    /// the screen.
    SetApiKey {
        /// The key itself. Omit to read it from stdin.
        key: Option<String>,
        /// Profile to write, for example `deepseek`. Defaults to the active
        /// profile, or the top-level `api_key` when none is set.
        #[arg(long)]
        profile: Option<String>,
    },
}

/// One profile's settings. Every field is optional so profiles can be partial
/// and layered over the top-level defaults.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Profile {
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<String>,
}

impl Profile {
    fn set(&mut self, key: &str, value: String) -> bool {
        match key {
            "base_url" => self.base_url = Some(value),
            "model" => self.model = Some(value),
            "api_key" => self.api_key = Some(value),
            _ => return false,
        }
        true
    }

    /// Layer `self` over `base`: values present in `self` win.
    fn over(self, base: Profile) -> Profile {
        Profile {
            base_url: self.base_url.or(base.base_url),
            model: self.model.or(base.model),
            api_key: self.api_key.or(base.api_key),
        }
    }
}
/// The parsed file. `lines` is kept verbatim so `set`/`unset` can rewrite the
/// file in place without dropping comments or keys this version doesn't know.
#[derive(Debug, Default, Clone)]
pub struct Config {
    /// Top-level `provider` key: the profile selected by default.
    pub provider: Option<String>,
    /// Top-level (undotted) `base_url`/`model`/`api_key`: shared defaults.
    pub global: Profile,
    /// `<name>.<key>` entries.
    pub profiles: BTreeMap<String, Profile>,
    lines: Vec<String>,
    /// False when the file does not exist yet.
    pub loaded: bool,
}

/// The config file path: `--config-file`/`MASSE_CONFIG_FILE` wins, then
/// `MASSE_CONFIG_DIR`, then the platform config directory. `None` when the home
/// directory cannot be determined.
pub fn path(override_path: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = override_path {
        return Some(p.to_path_buf());
    }
    if let Some(dir) = env::var_os(CONFIG_DIR_ENV).filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir).join(FILE_NAME));
    }
    let base = if cfg!(target_os = "macos") {
        env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        match env::var_os("XDG_CONFIG_HOME").filter(|d| !d.is_empty()) {
            Some(x) => Some(PathBuf::from(x)),
            None => env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")),
        }
    }?;
    Some(base.join("masse").join(FILE_NAME))
}

/// Load the file, or return an empty config when it is absent. Malformed lines
/// are skipped with a warning rather than aborting a run.
pub fn load(path: &Path) -> Result<Config> {
    let mut config = Config::default();
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(config),
        Err(e) => return Err(e).with_context(|| format!("cannot read config {}", path.display())),
    };
    config.loaded = true;
    warn_if_permissive(path);

    for (n, line) in text.lines().enumerate() {
        let raw = line.to_string();
        match parse_line(line) {
            Some((key, value)) => {
                if !config.apply(&key, value.clone()) {
                    eprintln!(
                        "[masse] warning: {}:{}: ignoring unknown key `{key}`",
                        path.display(),
                        n + 1
                    );
                }
            }
            None if is_blank_or_comment(line) => {}
            None => eprintln!(
                "[masse] warning: {}:{}: ignoring malformed line",
                path.display(),
                n + 1
            ),
        }
        config.lines.push(raw);
    }
    Ok(config)
}

impl Config {
    /// Apply one parsed entry. Returns false for keys we do not understand.
    fn apply(&mut self, key: &str, value: String) -> bool {
        if key == "provider" {
            self.provider = Some(value);
            return true;
        }
        if KEYS.contains(&key) {
            return self.global.set(key, value);
        }
        if let Some((profile, field)) = split_profile_key(key) {
            return self
                .profiles
                .entry(profile.to_string())
                .or_default()
                .set(field, value);
        }
        false
    }

    /// The settings for `profile`, layered over the top-level defaults.
    pub fn resolve(&self, profile: Option<&str>) -> Profile {
        match profile.and_then(|p| self.profiles.get(p)) {
            Some(p) => p.clone().over(self.global.clone()),
            None => self.global.clone(),
        }
    }

    /// Whether the named profile exists in the file.
    #[cfg(test)]
    pub fn has_profile(&self, profile: &str) -> bool {
        self.profiles.contains_key(profile)
    }

    /// The file, verbatim, with every `api_key` value masked.
    pub fn render_masked(&self) -> String {
        let mut out = String::new();
        for line in &self.lines {
            match parse_line(line) {
                Some((key, value)) if key.ends_with("api_key") => {
                    out.push_str(&format!("{key} = {}\n", mask(&value)));
                }
                _ => {
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
        out
    }
}

/// Write `key = value`, replacing an existing entry for `key` and preserving
/// every other line. Creates the file (mode 0600) and parent directory.
pub fn set(path: &Path, key: &str, value: &str) -> Result<bool> {
    validate_key(key)?;
    let mut config = load(path)?;
    let mut replaced = false;
    for line in config.lines.iter_mut() {
        if let Some((existing, _)) = parse_line(line) {
            if existing == key {
                *line = format!("{key} = {}", format_value(value));
                replaced = true;
                break;
            }
        }
    }
    if !replaced {
        config
            .lines
            .push(format!("{key} = {}", format_value(value)));
    }
    write_lines(path, &config.lines)?;
    Ok(replaced)
}

/// Remove `key`. Returns false when it was not present.
pub fn unset(path: &Path, key: &str) -> Result<bool> {
    let config = load(path)?;
    let before = config.lines.len();
    let lines: Vec<String> = config
        .lines
        .into_iter()
        .filter(|line| parse_line(line).map(|(k, _)| k) != Some(key.to_string()))
        .collect();
    let removed = lines.len() != before;
    if removed {
        write_lines(path, &lines)?;
    }
    Ok(removed)
}

/// Mask a secret for display: keep enough to recognise it, not enough to use it.
pub fn mask(secret: &str) -> String {
    let n = secret.chars().count();
    if n == 0 {
        return "(empty)".into();
    }
    if n <= 8 {
        return "********".into();
    }
    let head: String = secret.chars().take(4).collect();
    let tail: String = secret.chars().skip(n - 4).collect();
    format!("{head}…{tail}")
}

/// Entry point for `masse config …`.
pub async fn run(action: &ConfigAction, override_path: Option<PathBuf>) -> Result<()> {
    let path = path(override_path.as_deref()).context(
        "cannot determine the config directory (set HOME, MASSE_CONFIG_FILE or --config-file)",
    )?;

    match action {
        ConfigAction::Path => {
            let state = if path.is_file() {
                ""
            } else {
                " (not created yet)"
            };
            println!("{}{state}", path.display());
        }
        ConfigAction::Show => {
            let config = load(&path)?;
            if !config.loaded {
                println!("# {} does not exist yet", path.display());
                return Ok(());
            }
            print!("{}", config.render_masked());
        }
        ConfigAction::Set { key, value } => {
            let replaced = set(&path, key, value)?;
            let shown = if key.ends_with("api_key") {
                mask(value)
            } else {
                value.clone()
            };
            println!(
                "{} {key} = {shown} in {}",
                if replaced { "updated" } else { "added" },
                path.display()
            );
        }
        ConfigAction::Unset { key } => {
            if unset(&path, key)? {
                println!("removed {key} from {}", path.display());
            } else {
                println!("{key} is not set in {}", path.display());
            }
        }
        ConfigAction::SetApiKey { key, profile } => {
            let secret = match key {
                Some(k) => k.clone(),
                None => read_stdin_secret()?,
            };
            if secret.trim().is_empty() {
                bail!("refusing to store an empty API key");
            }
            let target = match profile {
                Some(p) => format!("{p}.api_key"),
                None => match load(&path)?.provider {
                    Some(p) => format!("{p}.api_key"),
                    None => "api_key".to_string(),
                },
            };
            set(&path, &target, secret.trim())?;
            println!(
                "stored {target} = {} in {}",
                mask(secret.trim()),
                path.display()
            );
        }
    }
    Ok(())
}

fn read_stdin_secret() -> Result<String> {
    use std::io::IsTerminal;

    // At a terminal, let rpassword drive: it writes the prompt, hides the key as
    // it is typed, ends the input at Enter (no EOF, so no Ctrl-D) and restores
    // the terminal on the way out.
    //
    // Caveat (rpassword, not our code): it turns the tty raw, so Ctrl-C is read
    // as a character and re-raised as SIGINT, which kills the process before the
    // restore runs. Ctrl-C at the prompt can therefore leave the terminal raw;
    // `reset` or `stty sane` fixes it. Typing the key normally is unaffected.
    if std::io::stdin().is_terminal() {
        return rpassword::prompt_password("API key (press Enter to finish): ")
            .context("cannot read the API key from the terminal");
    }
    read_stdin_line()
}

/// Piped input: read exactly one line, not to EOF, so a producer that keeps the
/// pipe open does not look like a hang.
fn read_stdin_line() -> Result<String> {
    use std::io::BufRead;

    let mut buf = String::new();
    let read = std::io::stdin()
        .lock()
        .read_line(&mut buf)
        .context("cannot read the API key from stdin")?;
    if read == 0 {
        bail!("no API key given on stdin");
    }
    Ok(buf)
}

fn validate_key(key: &str) -> Result<()> {
    if key == "provider" || KEYS.contains(&key) {
        return Ok(());
    }
    match split_profile_key(key) {
        Some((profile, _)) if !profile.is_empty() => Ok(()),
        _ => bail!(
            "unknown config key `{key}` (expected `provider`, one of {}, or `<profile>.<key>`)",
            KEYS.join(", ")
        ),
    }
}

fn split_profile_key(key: &str) -> Option<(&str, &str)> {
    let (profile, field) = key.split_once('.')?;
    if KEYS.contains(&field) {
        Some((profile, field))
    } else {
        None
    }
}

fn is_blank_or_comment(line: &str) -> bool {
    let t = line.trim();
    t.is_empty() || t.starts_with('#')
}

/// Parse `key = value`, stripping trailing comments from unquoted values.
fn parse_line(line: &str) -> Option<(String, String)> {
    if is_blank_or_comment(line) {
        return None;
    }
    let (key, raw) = line.split_once('=')?;
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    Some((key.to_string(), parse_value(raw)))
}

fn parse_value(raw: &str) -> String {
    let v = raw.trim();
    let quoted = v.len() >= 2
        && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\'')));
    if quoted {
        return unescape(&v[1..v.len() - 1]);
    }
    // Strip an inline comment: ` #` (space before the hash) only, so URLs with
    // fragments and keys containing `#` survive.
    match v.find(" #") {
        Some(i) => v[..i].trim_end().to_string(),
        None => v.to_string(),
    }
}

fn format_value(value: &str) -> String {
    let needs_quotes = value.is_empty()
        || value.trim() != value
        || value.starts_with(['"', '\''])
        || value.contains(" #")
        || value.contains('\\')
        || value.contains('"');
    if !needs_quotes {
        return value.to_string();
    }
    let escaped: String = value
        .chars()
        .flat_map(|c| match c {
            '\\' => vec!['\\', '\\'],
            '"' => vec!['\\', '"'],
            '\n' => vec!['\\', 'n'],
            '\t' => vec!['\\', 't'],
            other => vec![other],
        })
        .collect();
    format!("\"{escaped}\"")
}

fn unescape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

fn write_lines(path: &Path, lines: &[String]) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    let tmp = path.with_extension("tmp");
    {
        // Create with mode 0600 from the start: no window where the key is
        // readable by other users.
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .with_context(|| format!("cannot write {}", tmp.display()))?;
        for line in lines {
            file.write_all(line.as_bytes())?;
            file.write_all(b"\n")?;
        }
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path).with_context(|| format!("cannot replace {}", path.display()))?;
    Ok(())
}

/// Warn once when the file is readable by group or others.
fn warn_if_permissive(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    if let Ok(meta) = std::fs::metadata(path) {
        if meta.permissions().mode() & 0o077 != 0 {
            eprintln!(
                "[masse] warning: {} is readable by other users; run `chmod 600 {}`",
                path.display(),
                path.display()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Config {
        let mut config = Config::default();
        for line in text.lines() {
            if let Some((key, value)) = parse_line(line) {
                assert!(config.apply(&key, value), "unexpected key in test data");
            }
        }
        config
    }

    #[test]
    fn parses_profiles_and_defaults() {
        let config = parse(
            "# a comment\n\
             provider = deepseek\n\
             model = shared-model\n\
             deepseek.base_url = https://api.deepseek.com/v1\n\
             deepseek.api_key = sk-abc\n",
        );
        assert_eq!(config.provider.as_deref(), Some("deepseek"));
        let p = config.resolve(Some("deepseek"));
        assert_eq!(p.model.as_deref(), Some("shared-model"));
        assert_eq!(p.api_key.as_deref(), Some("sk-abc"));
        assert_eq!(p.base_url.as_deref(), Some("https://api.deepseek.com/v1"));
    }

    #[test]
    fn profile_wins_over_default() {
        let config = parse("model = shared\ndeepseek.model = deepseek-chat\n");
        assert_eq!(
            config.resolve(Some("deepseek")).model.as_deref(),
            Some("deepseek-chat")
        );
        // An unknown profile falls back to the top-level defaults.
        assert_eq!(
            config.resolve(Some("nope")).model.as_deref(),
            Some("shared")
        );
    }

    #[test]
    fn handles_quotes_and_inline_comments() {
        assert_eq!(parse_value(" sk-1 # trailing"), "sk-1");
        assert_eq!(parse_value(" \" sk-1 # kept \" "), " sk-1 # kept ");
        assert_eq!(parse_value("'single'"), "single");
        // No space before the hash: part of the value.
        assert_eq!(parse_value("sk-1#2"), "sk-1#2");
        assert!(parse_line("nonsense").is_none());
        assert!(parse_line("  # comment").is_none());
    }

    #[test]
    fn round_trips_awkward_values() {
        for value in [
            "sk-plain",
            "has space",
            "trailing ",
            "# leading hash",
            "a\"b\\c",
        ] {
            let formatted = format_value(value);
            assert_eq!(parse_value(&formatted), value, "value was {value:?}");
        }
    }

    #[test]
    fn masks_secrets() {
        assert_eq!(mask(""), "(empty)");
        assert_eq!(mask("short"), "********");
        assert_eq!(mask("sk-1234567890"), "sk-1…7890");
    }

    #[test]
    fn set_unset_and_show_preserve_other_lines() {
        let dir = std::env::temp_dir().join(format!("masse-config-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("config");

        assert!(!set(&file, "provider", "deepseek").unwrap());
        set(&file, "deepseek.api_key", "sk-secret").unwrap();
        // Rewriting an existing key replaces it in place.
        assert!(set(&file, "deepseek.api_key", "sk-rotated").unwrap());

        let raw = std::fs::read_to_string(&file).unwrap();
        assert_eq!(raw, "provider = deepseek\ndeepseek.api_key = sk-rotated\n");
        assert!(!raw.contains("sk-secret"));

        let config = load(&file).unwrap();
        assert!(config.loaded);
        assert!(config.has_profile("deepseek"));
        assert_eq!(
            config.resolve(Some("deepseek")).api_key.as_deref(),
            Some("sk-rotated")
        );
        assert!(config.render_masked().contains("sk-r…ated"));
        assert!(!config.render_masked().contains("sk-rotated"));

        assert!(unset(&file, "deepseek.api_key").unwrap());
        assert!(!unset(&file, "deepseek.api_key").unwrap());
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "provider = deepseek\n"
        );

        // The file is not group/other readable.
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "mode was {mode:o}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(validate_key("api_key").is_ok());
        assert!(validate_key("provider").is_ok());
        assert!(validate_key("deepseek.model").is_ok());
        assert!(validate_key("deepseek.nonsense").is_err());
        assert!(validate_key("nonsense").is_err());
    }
}
