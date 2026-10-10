//! Path resolution constrained to the project working directory.

use std::path::{Component, Path, PathBuf};

/// Resolve `user` relative to `root`, refusing to escape `root`.
///
/// `root` must be absolute (typically canonicalized once at startup).
/// `..` components are resolved lexically; the result must still live inside
/// `root`. Symlinks are *not* followed here for the containment check, so a
/// symlink pointing outside the project is still rejected by [`verify_exists_within`].
pub fn resolve_within(root: &Path, user: &str) -> Result<PathBuf, String> {
    // Normalize the root too: it can legitimately contain `..` (for example a
    // docs directory discovered as `../bialet/docs`). Comparing a normalized
    // candidate against a non-normalized root made every child path look like
    // an escape.
    let root = normalize(root);
    if user.trim().is_empty() {
        return Ok(root);
    }
    let raw = Path::new(user);
    let joined = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        root.join(raw)
    };
    let normalized = normalize(&joined);
    if normalized == root || normalized.starts_with(&root) {
        Ok(normalized)
    } else {
        Err(format!(
            "path '{}' escapes the project root '{}'",
            user,
            root.display()
        ))
    }
}

/// Lexically normalize a path, resolving `.` and `..` without touching the disk.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Resolve and then canonicalize an existing path, guaranteeing the real path
/// stays within `root`. Used before operations that read or delete.
pub fn resolve_existing_within(root: &Path, user: &str) -> Result<PathBuf, String> {
    let candidate = resolve_within(root, user)?;
    let real = std::fs::canonicalize(&candidate)
        .map_err(|e| format!("cannot access '{}': {e}", candidate.display()))?;
    let real_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    if real == real_root || real.starts_with(&real_root) {
        Ok(real)
    } else {
        Err(format!(
            "resolved path '{}' escapes the project root",
            real.display()
        ))
    }
}

/// The project root, refusing to delete it.
pub fn assert_not_root(root: &Path, target: &Path) -> Result<(), String> {
    if normalize(target) == normalize(root) {
        return Err("refusing to operate on the project root itself".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_escape() {
        let root = Path::new("/home/user/project");
        assert!(resolve_within(root, "../secrets").is_err());
        assert!(resolve_within(root, "src/main.rs").is_ok());
        assert_eq!(
            resolve_within(root, "src/../lib.rs").unwrap(),
            PathBuf::from("/home/user/project/lib.rs")
        );
    }

    #[test]
    fn a_root_that_contains_dotdot_still_contains_its_children() {
        // The docs directory is often detected as `../bialet/docs`, so the root
        // handed to the resolver carries a `..` of its own.
        let root = Path::new("/home/user/app/../shared/docs");
        assert_eq!(
            resolve_within(root, "tests.md").unwrap(),
            PathBuf::from("/home/user/shared/docs/tests.md")
        );
        // The empty path resolves to the normalized root, not the raw one.
        assert_eq!(
            resolve_within(root, "").unwrap(),
            PathBuf::from("/home/user/shared/docs")
        );
        // And a real escape from that root is still rejected.
        assert!(resolve_within(root, "../../secrets").is_err());
    }
}
