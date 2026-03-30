use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection};

pub fn record_injection(
    conn: &Connection,
    session_id: &str,
    tokens_before: i64,
    tokens_injected: i64,
    facts_count: i64,
) -> Result<()> {
    let now = Utc::now().timestamp();
    conn.execute(
        "INSERT INTO token_stats
             (session_id, event, tokens_before, tokens_injected, facts_count, recorded_at)
         VALUES (?1, 'injection', ?2, ?3, ?4, ?5)",
        params![session_id, tokens_before, tokens_injected, facts_count, now],
    )?;
    Ok(())
}

#[derive(Debug)]
pub struct GainSummary {
    pub total_injections:   i64,
    pub total_tokens_saved: i64,
    pub total_facts_stored: i64,
    pub total_sessions:     i64,
}

/// Approximate token count: chars / 4 (no tokenizer dependency, ~10% accurate).
pub fn estimate_tokens(text: &str) -> i64 {
    (text.len() as i64 + 3) / 4
}

pub fn gain_summary(conn: &Connection) -> Result<GainSummary> {
    let total_injections: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM token_stats WHERE event = 'injection'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    // "saved" = full-context load (tokens_before) minus what we actually injected
    let total_tokens_saved: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(tokens_before - tokens_injected), 0) FROM token_stats",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let total_facts_stored: i64 = conn
        .query_row("SELECT COUNT(*) FROM facts", [], |r| r.get(0))
        .unwrap_or(0);

    let total_sessions: i64 = conn
        .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
        .unwrap_or(0);

    Ok(GainSummary {
        total_injections,
        total_tokens_saved,
        total_facts_stored,
        total_sessions,
    })
}
