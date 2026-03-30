use crate::config::canonical_project_path;
use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub project_path: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub last_assistant_message: Option<String>,
    pub facts_extracted: i64,
    pub tokens_injected: i64,
}

pub fn start(conn: &Connection, session_id: &str, project_root: &Path) -> Result<()> {
    let project_path = canonical_project_path(project_root);
    let now = Utc::now().timestamp();
    conn.execute(
        "INSERT OR IGNORE INTO sessions (id, project_path, started_at)
         VALUES (?1, ?2, ?3)",
        params![session_id, project_path, now],
    )?;
    Ok(())
}

pub fn end(
    conn: &Connection,
    session_id: &str,
    last_message: Option<&str>,
    facts_extracted: i64,
    tokens_injected: i64,
) -> Result<()> {
    let now = Utc::now().timestamp();
    conn.execute(
        "UPDATE sessions
         SET ended_at = ?1, last_assistant_message = ?2,
             facts_extracted = ?3, tokens_injected = ?4
         WHERE id = ?5",
        params![now, last_message, facts_extracted, tokens_injected, session_id],
    )?;
    Ok(())
}

pub fn list(conn: &Connection, project_root: &Path, limit: usize) -> Result<Vec<Session>> {
    let project_path = canonical_project_path(project_root);
    let mut stmt = conn.prepare(
        "SELECT id, project_path, started_at, ended_at, last_assistant_message,
                facts_extracted, tokens_injected
         FROM   sessions
         WHERE  project_path = ?1
         ORDER  BY started_at DESC
         LIMIT  ?2",
    )?;
    let sessions = stmt
        .query_map(params![project_path, limit as i64], |row| {
            Ok(Session {
                id:                    row.get(0)?,
                project_path:          row.get(1)?,
                started_at:            ts_to_dt(row.get(2)?),
                ended_at:              row.get::<_, Option<i64>>(3)?.map(ts_to_dt),
                last_assistant_message: row.get(4)?,
                facts_extracted:       row.get(5)?,
                tokens_injected:       row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(sessions)
}

fn ts_to_dt(ts: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(ts, 0).unwrap_or_default()
}
