use crate::config::canonical_project_path;
use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
use uuid::Uuid;

// ─── Types ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fact {
    pub id: String,
    pub session_id: String,
    pub project_path: String,
    pub content: String,
    pub category: FactCategory,
    pub created_at: DateTime<Utc>,
    pub last_accessed: Option<DateTime<Utc>>,
    pub access_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FactCategory {
    Decision,
    Fix,
    File,
    Convention,
    User,
    Context,
}

impl FactCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            FactCategory::Decision   => "decision",
            FactCategory::Fix        => "fix",
            FactCategory::File       => "file",
            FactCategory::Convention => "convention",
            FactCategory::User       => "user",
            FactCategory::Context    => "context",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "decision"   => FactCategory::Decision,
            "fix"        => FactCategory::Fix,
            "file"       => FactCategory::File,
            "convention" => FactCategory::Convention,
            "user"       => FactCategory::User,
            _            => FactCategory::Context,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub fact: Fact,
    pub rank: f64,
}

// ─── Operations ───────────────────────────────────────────────────────────────

/// Insert a new fact. If an identical fact (same project + content hash) already
/// exists, returns the existing ID without inserting a duplicate.
pub fn insert(
    conn: &Connection,
    session_id: &str,
    project_root: &Path,
    content: &str,
    category: FactCategory,
) -> Result<String> {
    let project_path = canonical_project_path(project_root);

    let content_hash = {
        let mut h = Sha256::new();
        h.update(project_path.as_bytes());
        h.update(b":");
        h.update(content.as_bytes());
        format!("{:x}", h.finalize())
    };

    if let Ok(existing_id) = conn.query_row(
        "SELECT id FROM facts WHERE project_path = ?1 AND content_hash = ?2",
        params![project_path, content_hash],
        |row| row.get::<_, String>(0),
    ) {
        return Ok(existing_id);
    }

    let id  = Uuid::new_v4().to_string();
    let now = Utc::now().timestamp();

    // Ensure the session row exists (FK constraint). Uses OR IGNORE so it's safe
    // even when sessions::start() has already been called from the hook.
    conn.execute(
        "INSERT OR IGNORE INTO sessions (id, project_path, started_at) VALUES (?1, ?2, ?3)",
        params![session_id, project_path, now],
    )?;

    conn.execute(
        "INSERT INTO facts
             (id, session_id, project_path, content, category, content_hash, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![id, session_id, project_path, content, category.as_str(), content_hash, now],
    )?;

    Ok(id)
}

/// Search facts with FTS5 BM25 ranking. Falls back to recency order when query is empty.
pub fn search(
    conn: &Connection,
    project_root: &Path,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    let project_path = canonical_project_path(project_root);

    if query.trim().is_empty() {
        let mut stmt = conn.prepare(
            "SELECT id, session_id, project_path, content, category,
                    created_at, last_accessed, access_count
             FROM facts
             WHERE project_path = ?1
             ORDER BY created_at DESC
             LIMIT ?2",
        )?;
        let facts = stmt
            .query_map(params![project_path, limit as i64], row_to_fact)?
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(facts
            .into_iter()
            .map(|f| SearchResult { rank: 0.0, fact: f })
            .collect());
    }

    let safe_query = sanitize_fts_query(query);
    if safe_query.is_empty() {
        return Ok(vec![]);
    }

    let mut stmt = conn.prepare(
        "SELECT f.id, f.session_id, f.project_path, f.content, f.category,
                f.created_at, f.last_accessed, f.access_count,
                fts.rank
         FROM   facts_fts fts
         JOIN   facts f ON f.rowid = fts.rowid
         WHERE  facts_fts MATCH ?1
           AND  f.project_path = ?2
         ORDER  BY fts.rank
         LIMIT  ?3",
    )?;

    let results = stmt
        .query_map(params![safe_query, project_path, limit as i64], |row| {
            let fact = Fact {
                id:           row.get(0)?,
                session_id:   row.get(1)?,
                project_path: row.get(2)?,
                content:      row.get(3)?,
                category:     FactCategory::from_str(&row.get::<_, String>(4)?),
                created_at:   ts_to_dt(row.get(5)?),
                last_accessed: row.get::<_, Option<i64>>(6)?.map(ts_to_dt),
                access_count: row.get(7)?,
            };
            // FTS5 rank is negative — negate so higher = better
            let rank: f64 = -row.get::<_, f64>(8)?;
            Ok(SearchResult { fact, rank })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

/// Delete a fact by ID. Returns true if a row was actually deleted.
pub fn delete(conn: &Connection, id: &str) -> Result<bool> {
    let n = conn.execute("DELETE FROM facts WHERE id = ?1", params![id])?;
    Ok(n > 0)
}

/// Bump access stats for a slice of fact IDs (called after injection).
pub fn record_access_batch(conn: &Connection, ids: &[String]) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let now = Utc::now().timestamp();
    // Use an explicit transaction so N updates land in one fsync.
    let tx = conn.unchecked_transaction()?;
    for id in ids {
        tx.execute(
            "UPDATE facts
             SET last_accessed = ?1, access_count = access_count + 1
             WHERE id = ?2",
            params![now, id],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Count total facts stored for a project.
pub fn count(conn: &Connection, project_root: &Path) -> Result<i64> {
    let project_path = canonical_project_path(project_root);
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM facts WHERE project_path = ?1",
        params![project_path],
        |row| row.get(0),
    )?)
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn row_to_fact(row: &rusqlite::Row<'_>) -> rusqlite::Result<Fact> {
    Ok(Fact {
        id:           row.get(0)?,
        session_id:   row.get(1)?,
        project_path: row.get(2)?,
        content:      row.get(3)?,
        category:     FactCategory::from_str(&row.get::<_, String>(4)?),
        created_at:   ts_to_dt(row.get(5)?),
        last_accessed: row.get::<_, Option<i64>>(6)?.map(ts_to_dt),
        access_count: row.get(7)?,
    })
}

fn ts_to_dt(ts: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(ts, 0).unwrap_or_default()
}

/// Sanitise a user query for safe FTS5 MATCH expressions.
/// Each whitespace-separated token becomes a prefix search (token*).
fn sanitize_fts_query(query: &str) -> String {
    query
        .split_whitespace()
        .filter_map(|w| {
            let clean: String = w
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
                .collect();
            if clean.is_empty() {
                None
            } else {
                Some(format!("{}*", clean))
            }
        })
        .collect::<Vec<_>>()
        .join(" OR ")  // OR semantics: any matching term triggers inclusion, ranked by BM25
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open;
    use tempfile::TempDir;

    fn test_db() -> (TempDir, Connection) {
        let dir = TempDir::new().unwrap();
        let conn = open(dir.path()).unwrap();
        (dir, conn)
    }

    #[test]
    fn insert_and_search() {
        let (_dir, conn) = test_db();
        let project = Path::new("/test/project");
        insert(&conn, "s1", project, "Auth uses JWT tokens in httpOnly cookies", FactCategory::Decision).unwrap();
        insert(&conn, "s1", project, "Bug fixed: null pointer in LoginForm.tsx line 203", FactCategory::Fix).unwrap();

        let results = search(&conn, project, "login bug", 5).unwrap();
        assert!(!results.is_empty());
        assert!(results[0].fact.content.contains("LoginForm"));
    }

    #[test]
    fn deduplication() {
        let (_dir, conn) = test_db();
        let project = Path::new("/test/project");
        let id1 = insert(&conn, "s1", project, "Same content here", FactCategory::Context).unwrap();
        let id2 = insert(&conn, "s2", project, "Same content here", FactCategory::Context).unwrap();
        assert_eq!(id1, id2);
        assert_eq!(count(&conn, project).unwrap(), 1);
    }

    #[test]
    fn fts_query_sanitization() {
        assert_eq!(sanitize_fts_query("login bug"), "login* OR bug*");
        assert_eq!(sanitize_fts_query("AND OR"), "AND* OR OR*");
        assert_eq!(sanitize_fts_query("\"quoted\""), "quoted*");
        assert_eq!(sanitize_fts_query("  "), "");
    }

    #[test]
    fn delete_fact() {
        let (_dir, conn) = test_db();
        let project = Path::new("/test/project");
        let id = insert(&conn, "s1", project, "Fact to delete soon", FactCategory::Context).unwrap();
        assert!(delete(&conn, &id).unwrap());
        assert!(!delete(&conn, &id).unwrap()); // second delete is a no-op
    }
}
