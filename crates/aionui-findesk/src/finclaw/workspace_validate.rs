use std::path::{Path, PathBuf};

/// Canonicalize a FinClaw workspace path for API and spawn use.
pub fn validate_finclaw_workspace_path(workspace: &str) -> Result<PathBuf, String> {
    let trimmed = workspace.trim();
    if trimmed.is_empty() {
        return Err("workspace is required".into());
    }
    if trimmed.contains("..") {
        return Err("workspace path must not contain '..'".into());
    }

    let path = PathBuf::from(trimmed);
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("invalid workspace path: {e}"))?;

    if !canonical.is_dir() {
        return Err("workspace must be an existing directory".into());
    }

    Ok(canonical)
}

/// Returns true when `candidate` is the same as or a subdirectory of `root`.
pub fn is_path_within_root(root: &Path, candidate: &Path) -> bool {
    let Ok(root_canon) = root.canonicalize() else {
        return false;
    };
    let Ok(candidate_canon) = candidate.canonicalize() else {
        return false;
    };
    candidate_canon.starts_with(&root_canon)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn rejects_parent_traversal_segments() {
        assert!(validate_finclaw_workspace_path("../etc").is_err());
    }

    #[test]
    fn canonicalizes_existing_directory() {
        let tmp = TempDir::new().unwrap();
        let resolved = validate_finclaw_workspace_path(tmp.path().to_str().unwrap()).unwrap();
        assert_eq!(resolved, tmp.path().canonicalize().unwrap());
    }

    #[test]
    fn detects_subdirectory_membership() {
        let tmp = TempDir::new().unwrap();
        let child = tmp.path().join("nested");
        std::fs::create_dir_all(&child).unwrap();
        assert!(is_path_within_root(tmp.path(), &child));
    }
}
