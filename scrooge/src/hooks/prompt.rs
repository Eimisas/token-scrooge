use crate::config::{db_path, resolve_scrooge_dir};
use crate::db::{self, facts, sessions, stats};
use crate::format;
use crate::hooks::{HookInput, HookOutput};
use anyhow::Result;
use std::path::Path;

const MAX_INJECTED_FACTS: usize = 4;
/// Minimum BM25 rank (after negation) to include a fact. Filters noise.
const MIN_RANK: f64 = 0.0; // BM25 negated — any positive score counts

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

    // Ensure session row exists
    sessions::start(&conn, &input.session_id, cwd_path)?;

    // BM25 search
    let results = facts::search(&conn, cwd_path, &prompt, MAX_INJECTED_FACTS)?;
    let relevant: Vec<_> = results.into_iter().filter(|r| r.rank >= MIN_RANK).collect();

    if relevant.is_empty() {
        return Ok(HookOutput::allow());
    }

    // Bump access stats
    let ids: Vec<String> = relevant.iter().map(|r| r.fact.id.clone()).collect();
    facts::record_access_batch(&conn, &ids)?;

    // Build context string and record stats
    let fact_refs: Vec<&crate::db::facts::Fact> = relevant.iter().map(|r| &r.fact).collect();
    let context = format::memory_context(&fact_refs);
    let tokens_injected = stats::estimate_tokens(&context);
    let tokens_before   = stats::estimate_tokens(&prompt);

    stats::record_injection(
        &conn,
        &input.session_id,
        tokens_before,
        tokens_injected,
        relevant.len() as i64,
    )?;

    Ok(HookOutput::allow_with_context("UserPromptSubmit", context))
}

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

    #[test]
    fn injects_relevant_fact() {
        let dir     = TempDir::new().unwrap();
        let project = dir.path().join("proj");
        std::fs::create_dir(&project).unwrap();
        // Create .git so find_project_root anchors here instead of falling back to ~/.scrooge
        std::fs::create_dir(project.join(".git")).unwrap();
        let scrooge = project.join(".scrooge");
        let conn    = open(&scrooge).unwrap();

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
}
