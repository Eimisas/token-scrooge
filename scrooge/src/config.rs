use crate::error::ScroogeError;
use anyhow::Result;
use std::path::{Path, PathBuf};

const PROJECT_ROOT_MARKERS: &[&str] = &[
    ".git", "Cargo.toml", "package.json", "pyproject.toml", "go.mod", ".claude",
];

pub fn find_project_root(start: &Path) -> Result<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        for marker in PROJECT_ROOT_MARKERS {
            if current.join(marker).exists() {
                return Ok(current);
            }
        }
        match current.parent() {
            Some(parent) => current = parent.to_path_buf(),
            None => return Err(ScroogeError::NoProjectRoot.into()),
        }
    }
}

pub fn global_scrooge_dir() -> Result<PathBuf> {
    let home = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
    Ok(home.join(".scrooge"))
}

pub fn project_scrooge_dir(project_root: &Path) -> PathBuf {
    project_root.join(".scrooge")
}

pub fn resolve_scrooge_dir(cwd: &Path) -> PathBuf {
    match find_project_root(cwd) {
        Ok(root) => project_scrooge_dir(&root),
        Err(_) => global_scrooge_dir().unwrap_or_else(|_| cwd.join(".scrooge")),
    }
}

pub fn db_path(scrooge_dir: &Path) -> PathBuf {
    scrooge_dir.join("memory.db")
}

pub fn settings_json_path() -> Result<PathBuf> {
    let home = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
    Ok(home.join(".claude").join("settings.json"))
}

pub fn scrooge_binary_path() -> Result<PathBuf> {
    std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("Cannot determine scrooge path: {}", e))
}

pub fn canonical_project_path(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .trim_end_matches('/')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn finds_git_root() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        let sub = dir.path().join("a/b/c");
        fs::create_dir_all(&sub).unwrap();
        // Canonicalize both sides — on macOS /var is a symlink to /private/var
        let found = find_project_root(&sub).unwrap().canonicalize().unwrap();
        let expected = dir.path().canonicalize().unwrap();
        assert_eq!(found, expected);
    }

    #[test]
    fn canonical_path_no_trailing_slash() {
        // Path::new normalises — just check it doesn't panic
        let p = Path::new("/tmp");
        let c = canonical_project_path(p);
        assert!(!c.ends_with('/'));
    }
}
