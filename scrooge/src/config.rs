use crate::error::ScroogeError;
use anyhow::Result;
use serde::Deserialize;
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

// ─── Config ───────────────────────────────────────────────────────────────────

/// Per-project configuration loaded from `.scrooge/config.toml`.
/// All fields are optional in the TOML file; defaults are applied via serde.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScroogeConfig {
    /// Maximum facts injected per prompt. Env var `SCROOGE_MAX_FACTS` overrides.
    #[serde(default = "defaults::max_injected_facts")]
    pub max_injected_facts: usize,
    /// BM25 candidates fetched before re-ranking.
    #[serde(default = "defaults::candidate_fetch")]
    pub candidate_fetch: usize,
    /// Days over which recency score decays from 1.0 to 0.5.
    #[serde(default = "defaults::recency_decay_days")]
    pub recency_decay_days: f64,
    /// Facts inactive this many days are auto-archived.
    #[serde(default = "defaults::archive_after_days")]
    pub archive_after_days: i64,
    /// Per-category scoring weights (all must be positive).
    #[serde(default)]
    pub category_weights: CategoryWeights,
}

/// Scoring weights per fact category. All weights must be positive.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CategoryWeights {
    #[serde(default = "defaults::w_convention")] pub convention: f64,
    #[serde(default = "defaults::w_decision")]   pub decision:   f64,
    #[serde(default = "defaults::w_fix")]        pub fix:        f64,
    #[serde(default = "defaults::w_user")]       pub user:       f64,
    #[serde(default = "defaults::w_context")]    pub context:    f64,
    #[serde(default = "defaults::w_file")]       pub file:       f64,
}

impl Default for CategoryWeights {
    fn default() -> Self {
        Self {
            convention: defaults::w_convention(),
            decision:   defaults::w_decision(),
            fix:        defaults::w_fix(),
            user:       defaults::w_user(),
            context:    defaults::w_context(),
            file:       defaults::w_file(),
        }
    }
}

impl Default for ScroogeConfig {
    fn default() -> Self {
        Self {
            max_injected_facts: defaults::max_injected_facts(),
            candidate_fetch:    defaults::candidate_fetch(),
            recency_decay_days: defaults::recency_decay_days(),
            archive_after_days: defaults::archive_after_days(),
            category_weights:   CategoryWeights::default(),
        }
    }
}

mod defaults {
    pub fn max_injected_facts() -> usize { 4 }
    pub fn candidate_fetch()    -> usize { 15 }
    pub fn recency_decay_days() -> f64   { 90.0 }
    pub fn archive_after_days() -> i64   { 180 }
    pub fn w_convention()       -> f64   { 2.0 }
    pub fn w_decision()         -> f64   { 1.5 }
    pub fn w_fix()              -> f64   { 1.2 }
    pub fn w_user()             -> f64   { 1.0 }
    pub fn w_context()          -> f64   { 1.0 }
    pub fn w_file()             -> f64   { 0.5 }
}

impl ScroogeConfig {
    /// Validate all fields, returning an actionable error message if invalid.
    pub fn validate(&self) -> Result<()> {
        if self.max_injected_facts == 0 {
            anyhow::bail!("config: max_injected_facts must be > 0");
        }
        if self.candidate_fetch == 0 {
            anyhow::bail!("config: candidate_fetch must be > 0");
        }
        if self.recency_decay_days <= 0.0 {
            anyhow::bail!("config: recency_decay_days must be > 0");
        }
        if self.archive_after_days <= 0 {
            anyhow::bail!("config: archive_after_days must be > 0");
        }
        for (name, w) in [
            ("convention", self.category_weights.convention),
            ("decision",   self.category_weights.decision),
            ("fix",        self.category_weights.fix),
            ("user",       self.category_weights.user),
            ("context",    self.category_weights.context),
            ("file",       self.category_weights.file),
        ] {
            if w <= 0.0 {
                anyhow::bail!("config: category_weights.{} must be positive, got {}", name, w);
            }
        }
        Ok(())
    }
}

/// Load config from `<scrooge_dir>/config.toml`.
///
/// - Missing file → `ScroogeConfig::default()`.
/// - Parse error  → `Err` with an actionable message.
/// - `SCROOGE_MAX_FACTS` env var overrides `max_injected_facts` after parsing.
pub fn load_config(scrooge_dir: &Path) -> Result<ScroogeConfig> {
    let path = scrooge_dir.join("config.toml");
    let mut cfg: ScroogeConfig = if path.exists() {
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("config: cannot read {}: {}", path.display(), e))?;
        toml::from_str(&raw)
            .map_err(|e| anyhow::anyhow!("config: parse error in {}: {}", path.display(), e))?
    } else {
        ScroogeConfig::default()
    };

    // Env var takes precedence over the file value.
    if let Ok(v) = std::env::var("SCROOGE_MAX_FACTS") {
        cfg.max_injected_facts = v.parse::<usize>().map_err(|_| {
            anyhow::anyhow!("SCROOGE_MAX_FACTS must be a positive integer, got {:?}", v)
        })?;
    }

    cfg.validate()?;
    Ok(cfg)
}

/// Serialise a `ScroogeConfig` to a commented TOML string.
pub fn config_to_toml(cfg: &ScroogeConfig) -> String {
    format!(
        r#"# Token Scrooge configuration
# https://github.com/Eimisas/scrooge

## Maximum facts injected per prompt (env: SCROOGE_MAX_FACTS)
max_injected_facts = {max_injected_facts}

## BM25 candidates fetched before re-ranking
candidate_fetch = {candidate_fetch}

## Days over which recency score decays from 1.0 to 0.5
recency_decay_days = {recency_decay_days}

## Facts inactive this many days are automatically archived
archive_after_days = {archive_after_days}

[category_weights]
convention = {convention}
decision   = {decision}
fix        = {fix}
user       = {user}
context    = {context}
file       = {file}
"#,
        max_injected_facts = cfg.max_injected_facts,
        candidate_fetch    = cfg.candidate_fetch,
        recency_decay_days = cfg.recency_decay_days,
        archive_after_days = cfg.archive_after_days,
        convention = cfg.category_weights.convention,
        decision   = cfg.category_weights.decision,
        fix        = cfg.category_weights.fix,
        user       = cfg.category_weights.user,
        context    = cfg.category_weights.context,
        file       = cfg.category_weights.file,
    )
}

/// Canonical path to the project config file.
pub fn config_file_path(scrooge_dir: &Path) -> PathBuf {
    scrooge_dir.join("config.toml")
}

// ─── Tests ────────────────────────────────────────────────────────────────────

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

    // Serialize all tests that read/write SCROOGE_MAX_FACTS — env vars are process-global
    // and tests run in parallel threads sharing the same env.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn default_config_validates() {
        ScroogeConfig::default().validate().unwrap();
    }

    #[test]
    fn missing_config_file_returns_defaults() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("SCROOGE_MAX_FACTS");
        let dir = TempDir::new().unwrap();
        let cfg = load_config(dir.path()).unwrap();
        assert_eq!(cfg.max_injected_facts, defaults::max_injected_facts());
        assert_eq!(cfg.candidate_fetch, defaults::candidate_fetch());
    }

    #[test]
    fn partial_toml_fills_defaults() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("SCROOGE_MAX_FACTS");
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("config.toml"), "max_injected_facts = 8\n").unwrap();
        let cfg = load_config(dir.path()).unwrap();
        assert_eq!(cfg.max_injected_facts, 8);
        assert_eq!(cfg.candidate_fetch, defaults::candidate_fetch());
    }

    #[test]
    fn invalid_toml_errors_with_parse_error_in_message() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("config.toml"), "not valid toml :::").unwrap();
        let err = load_config(dir.path()).unwrap_err();
        assert!(err.to_string().contains("parse error"), "got: {}", err);
    }

    #[test]
    fn zero_max_facts_fails_validation() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("SCROOGE_MAX_FACTS");
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("config.toml"), "max_injected_facts = 0\n").unwrap();
        let err = load_config(dir.path()).unwrap_err();
        assert!(err.to_string().contains("max_injected_facts"), "got: {}", err);
    }

    #[test]
    fn negative_category_weight_fails_validation() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("SCROOGE_MAX_FACTS");
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("config.toml"),
            "[category_weights]\nconvention = -1.0\n",
        ).unwrap();
        let err = load_config(dir.path()).unwrap_err();
        assert!(err.to_string().contains("convention"), "got: {}", err);
    }

    #[test]
    fn env_var_overrides_config_file() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("config.toml"), "max_injected_facts = 2\n").unwrap();
        std::env::set_var("SCROOGE_MAX_FACTS", "7");
        let cfg = load_config(dir.path()).unwrap();
        std::env::remove_var("SCROOGE_MAX_FACTS");
        assert_eq!(cfg.max_injected_facts, 7);
    }
}
