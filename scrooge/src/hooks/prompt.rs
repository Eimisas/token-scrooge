use crate::config::{db_path, load_config, resolve_scrooge_dir};
use crate::db::{self, facts, sessions, stats};
use crate::format;
use crate::hooks::{HookInput, HookOutput};
use anyhow::Result;
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

    // Load embedding model and embed query
    let model = crate::embeddings::EmbeddingModel::load().ok();
    let query_embedding = model.and_then(|m| m.embed(&prompt).ok());

    // Ensure session row exists
    sessions::start(&conn, &input.session_id, cwd_path)?;

    // Load already-seen fact IDs for this session to avoid repeating injections
    let seen = load_seen(&scrooge_dir, &input.session_id);

    // Fetch more candidates than needed so re-ranking and semantic deduplication have room to work
    let opts = facts::SearchOptions {
        category_weights:   cfg.category_weights.clone(),
        recency_decay_days: cfg.recency_decay_days,
        query_embedding:    query_embedding.clone(),
    };
    // Fetch 2x the limit to allow room for deduplication
    let results = facts::search(&conn, cwd_path, &prompt, cfg.max_injected_facts * 2, &opts)?;

    // Filter noise, exclude already-seen, and perform SEMANTIC DEDUPLICATION
    let mut selected: Vec<facts::SearchResult> = Vec::new();
    for r in results {
        if r.rank < MIN_RANK || seen.contains(r.fact.id.as_str()) {
            continue;
        }

        // Check if this candidate is too similar to something already selected
        let mut is_duplicate = false;
        if let Some(cand_emb) = &r.fact.embedding {
            for existing in &selected {
                if let Some(existing_emb) = &existing.fact.embedding {
                    let sim = crate::embeddings::cosine_similarity(cand_emb, existing_emb);
                    if sim > 0.70 { // Lowered from 0.80 to be more aggressive against duplicates
                        is_duplicate = true;
                        break;
                    }
                }
            }
        }

        if !is_duplicate {
            selected.push(r);
        }

        if selected.len() >= cfg.max_injected_facts {
            break;
        }
    }

    if selected.is_empty() {
        return Ok(HookOutput::allow());
    }

    // Persist injected IDs so subsequent messages in this session skip them
    let ids: Vec<String> = selected.iter().map(|r| r.fact.id.clone()).collect();
    save_seen(&scrooge_dir, &input.session_id, &ids);
    facts::record_access_batch(&conn, &ids)?;

    let fact_refs: Vec<&crate::db::facts::Fact> = selected.iter().map(|r| &r.fact).collect();
    
    // --- Librarian (SLM Reconciliation) ---
    let home = dirs::home_dir().expect("no home dir");
    let socket_path = home.join(".scrooge").join("daemon.sock").to_string_lossy().to_string();
    let client = crate::daemon::Client::new(&socket_path);

    let mut context = format::memory_context(&fact_refs);

    if client.is_running() && selected.len() > 1 {
        // Simple conflict trigger: high similarity or same category
        let mut needs_reconciliation = false;
        for i in 0..selected.len() {
            for j in i+1..selected.len() {
                if selected[i].fact.category == selected[j].fact.category {
                    needs_reconciliation = true;
                    break;
                }
                if let (Some(e1), Some(e2)) = (&selected[i].fact.embedding, &selected[j].fact.embedding) {
                    if crate::embeddings::cosine_similarity(e1, e2) > 0.65 {
                        needs_reconciliation = true;
                        break;
                    }
                }
            }
            if needs_reconciliation { break; }
        }

        if needs_reconciliation {
            let req = crate::protocol::Request::Librarian {
                prompt: format!("Query: {}\n\nFacts:\n{}", prompt, context),
                max_tokens: 150,
            };
            if let Ok(crate::protocol::Response::Librarian { summary }) = client.send(req) {
                context = format!("[scrooge] Reconciled Memory:\n{}", summary);
            }
        }
    }

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

        insert(&conn, "s0", &project, "Auth uses JWT tokens in httpOnly cookies", FactCategory::Decision, None).unwrap();

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

        insert(&conn, "s0", &project, "JWT auth tokens convention always use httpOnly", FactCategory::Convention, None).unwrap();

        let input = make_input(&project.to_string_lossy(), "JWT auth tokens");

        // First call — should inject
        let out1 = handle(&input).unwrap();
        assert!(out1.hook_specific_output.is_some(), "first call should inject");

        // Second call in same session — fact already seen, should not inject again
        let out2 = handle(&input).unwrap();
        assert!(out2.hook_specific_output.is_none(), "second call should skip already-seen fact");
    }
}
