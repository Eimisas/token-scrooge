use crate::config::{load_config, resolve_scrooge_dir};
use crate::db::{self, facts, sessions};
use crate::extract::{heuristic, transcript};
use crate::hooks::{HookInput, HookOutput};
use anyhow::Result;
use std::path::Path;

pub fn handle(input: &HookInput) -> Result<HookOutput> {
    // Guard: stop_hook_active prevents infinite loops
    if input.stop_hook_active {
        return Ok(HookOutput::allow());
    }

    let cwd = input.cwd.as_deref().unwrap_or(".");
    let cwd_path = Path::new(cwd);
    let scrooge_dir = resolve_scrooge_dir(cwd_path);

    // Only run if the project has been explicitly set up with `scrooge setup` or `scrooge claude`.
    // This prevents creating .scrooge/ in projects the user never opted in.
    use crate::config::db_path;
    if !db_path(&scrooge_dir).exists() {
        return Ok(HookOutput::allow());
    }

    let conn = db::open(&scrooge_dir)?;
    let cfg = load_config(&scrooge_dir).unwrap_or_default();

    // Resolve the transcript path
    let transcript_path = input
        .transcript_path
        .clone()
        .or_else(|| find_latest_transcript(cwd_path))
        .unwrap_or_default();

    if transcript_path.is_empty() || !Path::new(&transcript_path).exists() {
        sessions::end(&conn, &input.session_id, input.last_assistant_message.as_deref(), 0, 0)?;
        return Ok(HookOutput::allow());
    }

    // Parse transcript and extract facts
    let messages  = transcript::parse_with_file_ops(Path::new(&transcript_path)).unwrap_or_default();
    let extracted = heuristic::extract(&messages);
    let facts_count = extracted.len() as i64;

    let model = crate::embeddings::EmbeddingModel::load().ok();

    for ef in extracted {
        let embedding = model.as_ref().and_then(|m| m.embed(&ef.content).ok());
        facts::insert(&conn, &input.session_id, cwd_path, &ef.content, ef.category, embedding.as_deref())?;
    }

    // Sum tokens injected across all injections in this session
    let tokens_injected: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(tokens_injected), 0) FROM token_stats WHERE session_id = ?1",
            rusqlite::params![input.session_id],
            |row| row.get(0),
        )
        .unwrap_or(0);

    sessions::end(
        &conn,
        &input.session_id,
        input.last_assistant_message.as_deref(),
        facts_count,
        tokens_injected,
    )?;

    // Auto-archive stale facts — non-fatal; a failure must never block Claude.
    match facts::archive_facts_older_than(&conn, cwd_path, cfg.archive_after_days) {
        Ok(ids) if !ids.is_empty() => {
            if std::env::var("SCROOGE_DEBUG").is_ok() {
                eprintln!("[scrooge] auto-archived {} stale fact(s)", ids.len());
            }
        }
        Ok(_) => {}
        Err(e) => eprintln!("[scrooge] warning: auto-archive failed: {}", e),
    }

    // Clean up the session seen-file now that the session has ended
    let seen_path = scrooge_dir.join(format!("session-{}.seen", input.session_id));
    let _ = std::fs::remove_file(&seen_path);

    Ok(HookOutput::allow())
}

/// Fallback: find the newest .jsonl in the Claude projects dir for this cwd.
fn find_latest_transcript(cwd: &Path) -> Option<String> {
    let home = dirs::home_dir()?;
    let hash = cwd
        .to_string_lossy()
        .replace('/', "-")
        .trim_start_matches('-')
        .to_string();
    let dir = home.join(".claude").join("projects").join(&hash);
    if !dir.exists() { return None; }

    std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|x| x == "jsonl").unwrap_or(false))
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
        .map(|e| e.path().to_string_lossy().to_string())
}
