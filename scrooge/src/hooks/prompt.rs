use crate::config::{db_path, load_config, resolve_scrooge_dir};
use crate::db::{self, facts, sessions, stats};
use crate::format;
use crate::hooks::{HookInput, HookOutput};
use anyhow::Result;
use chrono::Utc;
use std::collections::HashSet;
use std::path::Path;

/// Minimum BM25 rank (after negation) to include a candidate.
const MIN_RANK: f64 = 0.0;

pub fn handle(input: &HookInput) -> Result<HookOutput> {
    let prompt = match &input.prompt {
        Some(p) => p.clone(),
        None    => return Ok(HookOutput::allow()),
    };

    let cwd = input.cwd.as_deref().unwrap_or(".");
    let cwd_path = Path::new(cwd);
    let scrooge_dir = resolve_scrooge_dir(cwd_path);

    // Nothing to inject if no DB yet
    if !db_path(&scrooge_dir).exists() {
        return Ok(HookOutput::allow());
    }

    let conn = db::open(&scrooge_dir)?;
    let cfg = load_config(&scrooge_dir).unwrap_or_default();

    // Ensure session row exists
    sessions::start(&conn, &input.session_id, cwd_path)?;

    // Load already-seen fact IDs for this session to avoid repeating injections
    let seen = load_seen(&scrooge_dir, &input.session_id);

    // Fetch more candidates than needed so re-ranking has room to work
    let opts = facts::SearchOptions {
        category_weights:   cfg.category_weights.clone(),
        recency_decay_days: cfg.recency_decay_days,
    };
    let results = facts::search(&conn, cwd_path, &prompt, cfg.candidate_fetch, &opts)?;

    // Filter noise, exclude already-seen, apply category-weighted re-ranking
    let max_inject = cfg.max_injected_facts;
    let now = Utc::now();
    let mut ranked: Vec<_> = results
        .into_iter()
        .filter(|r| r.rank >= MIN_RANK && !seen.contains(r.fact.id.as_str()))
        .map(|r| {
            let score = r.rank
                * crate::scoring::category_weight(&r.fact.category, &cfg.category_weights)
                * crate::scoring::recency_factor(r.fact.created_at, now, cfg.recency_decay_days)
                * crate::scoring::access_boost(r.fact.access_count);
            (score, r)
        })
        .collect();
    ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let selected: Vec<_> = ranked.into_iter().take(max_inject).map(|(_, r)| r).collect();

    if selected.is_empty() {
        return Ok(HookOutput::allow());
    }

    // Persist injected IDs so subsequent messages in this session skip them
    let ids: Vec<String> = selected.iter().map(|r| r.fact.id.clone()).collect();
    save_seen(&scrooge_dir, &input.session_id, &ids);
    facts::record_access_batch(&conn, &ids)?;

    // Build context string and record stats
    let fact_refs: Vec<&crate::db::facts::Fact> = selected.iter().map(|r| &r.fact).collect();
    let context = format::memory_context(&fact_refs);
    let tokens_injected = stats::estimate_tokens(&context);
    let tokens_before   = stats::estimate_tokens(&prompt);

    stats::record_injection(
        &conn,
        &input.session_id,
        tokens_before,
        tokens_injected,
        selected.len() as i64,
    )?;

    Ok(HookOutput::allow_with_context("UserPromptSubmit", context))
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Path to the per-session seen-file that tracks already-injected fact IDs.
fn seen_file(scrooge_dir: &Path, session_id: &str) -> std::path::PathBuf {
    scrooge_dir.join(format!("session-{}.seen", session_id))
}

/// Load the set of fact IDs already injected in this session.
fn load_seen(scrooge_dir: &Path, session_id: &str) -> HashSet<String> {
    let path = seen_file(scrooge_dir, session_id);
    std::fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect()
}

/// Append newly-injected fact IDs to the session seen-file.
fn save_seen(scrooge_dir: &Path, session_id: &str, ids: &[String]) {
    let path = seen_file(scrooge_dir, session_id);
    // Best-effort: ignore write errors (seen-file is an optimisation, not critical)
    if let Ok(mut content) = std::fs::read_to_string(&path) {
        for id in ids {
            content.push_str(id);
            content.push('\n');
        }
        let _ = std::fs::write(&path, content);
    } else {
        let _ = std::fs::write(&path, ids.join("\n") + "\n");
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{facts::*, open};
    use tempfile::TempDir;

    fn make_input(cwd: &str, prompt: &str) -> HookInput {
        HookInput {
            session_id:   "test-sess".to_string(),
            cwd:          Some(cwd.to_string()),
            prompt:       Some(prompt.to_string()),
            ..Default::default()
        }
    }

    fn setup_project(dir: &TempDir) -> (std::path::PathBuf, rusqlite::Connection) {
        let project = dir.path().join("proj");
        std::fs::create_dir(&project).unwrap();
        std::fs::create_dir(project.join(".git")).unwrap();
        let scrooge = project.join(".scrooge");
        let conn = open(&scrooge).unwrap();
        (project, conn)
    }

    #[test]
    fn injects_relevant_fact() {
        let dir = TempDir::new().unwrap();
        let (project, conn) = setup_project(&dir);

        insert(&conn, "s0", &project, "Auth uses JWT tokens in httpOnly cookies", FactCategory::Decision).unwrap();

        let input  = make_input(&project.to_string_lossy(), "JWT auth cookies session");
        let output = handle(&input).unwrap();

        let ctx = output
            .hook_specific_output
            .as_ref()
            .and_then(|h| h.additional_context.as_ref());
        assert!(ctx.is_some(), "expected additionalContext");
        assert!(ctx.unwrap().contains("JWT"));
    }

    #[test]
    fn no_injection_on_empty_db() {
        let dir  = TempDir::new().unwrap();
        let input = make_input(&dir.path().to_string_lossy(), "anything");
        let output = handle(&input).unwrap();
        assert!(output.hook_specific_output.is_none());
    }

    // Note: category_weight, recency_factor, and access_boost are tested in scoring.rs

    #[test]
    fn same_fact_not_injected_twice_in_session() {
        let dir = TempDir::new().unwrap();
        let (project, conn) = setup_project(&dir);

        insert(&conn, "s0", &project, "JWT auth tokens convention always use httpOnly", FactCategory::Convention).unwrap();

        let input = make_input(&project.to_string_lossy(), "JWT auth tokens");

        // First call — should inject
        let out1 = handle(&input).unwrap();
        assert!(out1.hook_specific_output.is_some(), "first call should inject");

        // Second call in same session — fact already seen, should not inject again
        let out2 = handle(&input).unwrap();
        assert!(out2.hook_specific_output.is_none(), "second call should skip already-seen fact");
    }
}
