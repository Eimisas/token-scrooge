use thiserror::Error;

#[derive(Error, Debug)]
#[allow(dead_code)]
pub enum ScroogeError {
    #[error("Database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Could not find project root")]
    NoProjectRoot,
    #[error("Claude binary not found in PATH")]
    ClaudeNotFound,
    #[error("Fact not found: {0}")]
    FactNotFound(String),
    #[error("Hook input missing required field: {0}")]
    HookInputMissing(String),
}
