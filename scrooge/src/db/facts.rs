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
    pub id:            String,
    pub session_id:    String,
    pub project_path:  String,
    pub content:       String,
    pub category:      FactCategory,
    pub created_at:    DateTime<Utc>,
    pub last_accessed: Option<DateTime<Utc>>,
    pub access_count:  i64,
    pub archived_at:   Option<DateTime<Utc>>,
    pub embedding:     Option<Vec<f32>>,
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

/// Scoring parameters passed into `search` so the DB layer stays decoupled from hook config.
#[derive(Debug, Clone)]
pub struct SearchOptions {
    pub category_weights:   crate::config::CategoryWeights,
    pub recency_decay_days: f64,
    pub query_embedding:    Option<Vec<f32>>,
}

impl Default for SearchOptions {
    fn default() -> Self {
        let cfg = crate::config::ScroogeConfig::default();
        Self {
            category_weights: cfg.category_weights,
            recency_decay_days: cfg.recency_decay_days,
            query_embedding: None,
        }
    }
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
    embedding: Option<&[f32]>,
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

    // --- Fact Compaction (Semantic Deduplication) ---
    if let Some(new_vec) = embedding {
        // Collect into an owned Vec before doing any writes. rusqlite does not allow
        // a prepared statement (and its borrow on `conn`) to remain open while another
        // operation mutates the same connection, so we fully consume the query first.
        let mut sim_stmt = conn.prepare(
            "SELECT id, embedding, category FROM facts
             WHERE project_path = ?1 AND archived_at IS NULL AND embedding IS NOT NULL",
        )?;
        let existing: Vec<(String, Vec<f32>, String)> = sim_stmt
            .query_map(params![project_path], |row| {
                let id: String = row.get(0)?;
                let blob: Vec<u8> = row.get(1)?;
                let cat: String = row.get(2)?;
                let vec = blob
                    .chunks_exact(4)
                    .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
                    .collect::<Vec<f32>>();
                Ok((id, vec, cat))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(sim_stmt); // release the borrow on `conn` before any write below

        for (id, old_vec, old_cat_str) in existing {
            let sim = crate::embeddings::cosine_similarity(new_vec, &old_vec);
            if sim > 0.85 {
                let old_cat = FactCategory::from_str(&old_cat_str);
                let both_mutable =
                    matches!(category, FactCategory::Decision | FactCategory::Convention)
                    && matches!(old_cat, FactCategory::Decision | FactCategory::Convention);

                if both_mutable {
                    // The new fact supersedes the old one: archive the old entry so the
                    // fresher decision/convention wins. Fall through to insert below.
                    let now_ts = Utc::now().timestamp();
                    conn.execute(
                        "UPDATE facts SET archived_at = ?1 WHERE id = ?2",
                        params![now_ts, id],
                    )?;
                    break;
                } else {
                    // Non-mutable categories (fix, file, context, user): treat as
                    // duplicate and bump access count instead of inserting again.
                    record_access_batch(conn, &[id.clone()])?;
                    return Ok(id);
                }
            }
        }
    }

    let id  = Uuid::new_v4().to_string();
    let now = Utc::now().timestamp();

    let embedding_blob = embedding.map(|v| {
        let mut bytes = Vec::with_capacity(v.len() * 4);
        for &f in v {
            bytes.extend_from_slice(&f.to_le_bytes());
        }
        bytes
    });

    // Ensure the session row exists (FK constraint). Uses OR IGNORE so it's safe
    // even when sessions::start() has already been called from the hook.
    conn.execute(
        "INSERT OR IGNORE INTO sessions (id, project_path, started_at) VALUES (?1, ?2, ?3)",
        params![session_id, project_path, now],
    )?;

    conn.execute(
        "INSERT INTO facts
             (id, session_id, project_path, content, category, content_hash, created_at, embedding)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![id, session_id, project_path, content, category.as_str(), content_hash, now, embedding_blob],
    )?;

    Ok(id)
}

/// Search facts with FTS5 BM25 ranking. When query is empty, returns scored fallback
/// ranked by category weight × recency × access count using `opts`.
pub fn search(
    conn: &Connection,
    project_root: &Path,
    query: &str,
    limit: usize,
    opts: &SearchOptions,
) -> Result<Vec<SearchResult>> {
    let project_path = canonical_project_path(project_root);
    let now = Utc::now();

    if query.trim().is_empty() {
        let mut stmt = conn.prepare(
            "SELECT id, session_id, project_path, content, category,
                    created_at, last_accessed, access_count, archived_at,
                    NULL, embedding
             FROM facts
             WHERE project_path = ?1
               AND archived_at  IS NULL
             LIMIT ?2",
        )?;
        let fetch_limit = (limit * 4) as i64;
        let mut results: Vec<SearchResult> = stmt
            .query_map(params![project_path, fetch_limit], row_to_fact)?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|f| {
                let score = crate::scoring::category_weight(&f.category, &opts.category_weights)
                    * crate::scoring::recency_factor(f.created_at, now, opts.recency_decay_days)
                    * crate::scoring::access_boost(f.access_count);
                SearchResult { rank: score, fact: f }
            })
            .collect();
        results.sort_by(|a, b| b.rank.partial_cmp(&a.rank).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(limit);
        return Ok(results);
    }

    let tokens = fts_tokens(query);
    if tokens.is_empty() {
        return Ok(vec![]);
    }

    // Try AND semantics first (precise); fall back to OR if no results (broader)
    let and_query = tokens.join(" AND ");
    let mut results = run_fts_query(conn, &and_query, &project_path, limit * 2)?;
    if results.is_empty() {
        let or_query = tokens.join(" OR ");
        results = run_fts_query(conn, &or_query, &project_path, limit * 2)?;
    }

    // --- Semantic Recovery (Fallback for keyword misses) ---
    // If we have few results or keyword misses, scan the 50 most recent facts semantically.
    if let Some(q_emb) = &opts.query_embedding {
        let mut stmt = conn.prepare(
            "SELECT id, session_id, project_path, content, category,
                    created_at, last_accessed, access_count, archived_at,
                    NULL, embedding
             FROM facts
             WHERE project_path = ?1 AND archived_at IS NULL
             ORDER BY created_at DESC
             LIMIT 50",
        )?;
        let seen_ids: std::collections::HashSet<_> = results.iter().map(|r| r.fact.id.clone()).collect();
        let fallback_facts = stmt.query_map(params![project_path], row_to_fact)?
            .collect::<Result<Vec<_>, _>>()?;

        for f in fallback_facts {
            if seen_ids.contains(&f.id) { continue; }
            if let Some(f_emb) = &f.embedding {
                let similarity = crate::embeddings::cosine_similarity(q_emb, f_emb);
                if similarity > 0.6 { // Minimum semantic threshold for recovery
                    results.push(SearchResult { rank: 0.0, fact: f });
                }
            }
        }
    }

    // Apply full re-ranking: (BM25 + SemanticBoost) * weights * recency * access
    for r in &mut results {
        let mut semantic_score = 0.0;
        if let (Some(q_emb), Some(f_emb)) = (&opts.query_embedding, &r.fact.embedding) {
            let similarity = crate::embeddings::cosine_similarity(q_emb, f_emb);
            // We ADD semantic similarity to the rank instead of multiplying by it.
            // This ensures a 0 BM25 score (keyword miss) can still rank high.
            semantic_score = (similarity.max(0.0) as f64) * 15.0; // Scaled to compete with BM25
        }

        r.rank = (r.rank + semantic_score)
            * crate::scoring::category_weight(&r.fact.category, &opts.category_weights)
            * crate::scoring::recency_factor(r.fact.created_at, now, opts.recency_decay_days)
            * crate::scoring::access_boost(r.fact.access_count);
    }

    results.sort_by(|a, b| b.rank.partial_cmp(&a.rank).unwrap_or(std::cmp::Ordering::Equal));
    results.truncate(limit);
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
#[allow(dead_code)]
pub fn count(conn: &Connection, project_root: &Path) -> Result<i64> {
    let project_path = canonical_project_path(project_root);
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM facts WHERE project_path = ?1",
        params![project_path],
        |row| row.get(0),
    )?)
}

/// Archive facts inactive for more than `threshold_days`.
/// Uses `last_accessed` when set, otherwise `created_at`.
/// Returns the IDs of newly-archived facts.
pub fn archive_facts_older_than(
    conn: &Connection,
    project_root: &Path,
    threshold_days: i64,
) -> Result<Vec<String>> {
    let project_path = canonical_project_path(project_root);
    let cutoff = Utc::now().timestamp() - threshold_days * 86_400;
    let now    = Utc::now().timestamp();

    let mut stmt = conn.prepare(
        "SELECT id FROM facts
         WHERE  project_path = ?1
           AND  archived_at  IS NULL
           AND  COALESCE(last_accessed, created_at) < ?2",
    )?;
    let ids: Vec<String> = stmt
        .query_map(params![project_path, cutoff], |row| row.get(0))?
        .collect::<Result<_, _>>()?;

    if ids.is_empty() {
        return Ok(ids);
    }

    let tx = conn.unchecked_transaction()?;
    for id in &ids {
        tx.execute(
            "UPDATE facts SET archived_at = ?1 WHERE id = ?2",
            params![now, id],
        )?;
    }
    tx.commit()?;
    Ok(ids)
}

/// Restore an archived fact by clearing its `archived_at` timestamp.
/// Returns `true` if the fact was found and unarchived.
#[allow(dead_code)]
pub fn unarchive_fact(conn: &Connection, id: &str) -> Result<bool> {
    let n = conn.execute(
        "UPDATE facts SET archived_at = NULL WHERE id = ?1 AND archived_at IS NOT NULL",
        params![id],
    )?;
    Ok(n > 0)
}

/// Like `search` but also returns archived facts.
/// Archived facts have `fact.archived_at.is_some()`.
pub fn search_including_archived(
    conn: &Connection,
    project_root: &Path,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    let project_path = canonical_project_path(project_root);

    if query.trim().is_empty() {
        let mut stmt = conn.prepare(
            "SELECT id, session_id, project_path, content, category,
                    created_at, last_accessed, access_count, archived_at
             FROM facts
             WHERE project_path = ?1
             ORDER BY created_at DESC
             LIMIT ?2",
        )?;
        let facts = stmt
            .query_map(params![project_path, limit as i64], row_to_fact)?
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(facts.into_iter().map(|f| SearchResult { rank: 0.0, fact: f }).collect());
    }

    let tokens = fts_tokens(query);
    if tokens.is_empty() { return Ok(vec![]); }

    let and_query = tokens.join(" AND ");
    let results = run_fts_query_all(conn, &and_query, &project_path, limit)?;
    if !results.is_empty() { return Ok(results); }

    let or_query = tokens.join(" OR ");
    run_fts_query_all(conn, &or_query, &project_path, limit)
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn row_to_fact(row: &rusqlite::Row<'_>) -> rusqlite::Result<Fact> {
    let embedding_blob: Option<Vec<u8>> = row.get(9).ok();
    let embedding = embedding_blob.map(|bytes| {
        bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
            .collect()
    });

    Ok(Fact {
        id:            row.get(0)?,
        session_id:    row.get(1)?,
        project_path:  row.get(2)?,
        content:       row.get(3)?,
        category:      FactCategory::from_str(&row.get::<_, String>(4)?),
        created_at:    ts_to_dt(row.get(5)?),
        last_accessed: row.get::<_, Option<i64>>(6)?.map(ts_to_dt),
        access_count:  row.get(7)?,
        archived_at:   row.get::<_, Option<i64>>(8)?.map(ts_to_dt),
        embedding,
    })
}

fn ts_to_dt(ts: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(ts, 0).unwrap_or_default()
}

/// Tokenise a query into safe FTS5 prefix-search terms (`token*`).
/// Returns an empty Vec if no valid tokens exist.
///
/// FTS5 reserved keywords (AND, OR, NOT, NEAR) are dropped: appending `*`
/// to them causes a parse error because FTS5 sees e.g. `NEAR*` as the
/// proximity-search keyword followed by an unexpected `*`.
fn fts_tokens(query: &str) -> Vec<String> {
    const FTS5_KEYWORDS: &[&str] = &["and", "or", "not", "near"];
    query
        .split_whitespace()
        .filter_map(|w| {
            let clean: String = w
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
                .collect();
            // Strip leading dashes (e.g. CLI flags `--stat`, `-v`).
            // A token starting with `-` followed by `*` is invalid FTS5 and
            // triggers "syntax error near *".
            let clean = clean.trim_start_matches('-');
            if clean.is_empty() { return None; }
            if FTS5_KEYWORDS.contains(&clean.to_ascii_lowercase().as_str()) { return None; }
            Some(format!("{}*", clean))
        })
        .collect()
}

/// Execute a single FTS5 MATCH query, excluding archived facts.
fn run_fts_query(
    conn: &Connection,
    fts_expr: &str,
    project_path: &str,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    run_fts_query_inner(conn, fts_expr, project_path, limit, false)
}

/// Execute a single FTS5 MATCH query, including archived facts.
fn run_fts_query_all(
    conn: &Connection,
    fts_expr: &str,
    project_path: &str,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    run_fts_query_inner(conn, fts_expr, project_path, limit, true)
}

fn run_fts_query_inner(
    conn: &Connection,
    fts_expr: &str,
    project_path: &str,
    limit: usize,
    include_archived: bool,
) -> Result<Vec<SearchResult>> {
    let archived_clause = if include_archived { "" } else { "AND  f.archived_at  IS NULL" };
    let sql = format!(
        "SELECT f.id, f.session_id, f.project_path, f.content, f.category,
                f.created_at, f.last_accessed, f.access_count, f.archived_at,
                fts.rank, f.embedding
         FROM   facts_fts fts
         JOIN   facts f ON f.rowid = fts.rowid
         WHERE  facts_fts MATCH ?1
           AND  f.project_path = ?2
           {archived_clause}
         ORDER  BY fts.rank
         LIMIT  ?3"
    );
    let mut stmt = conn.prepare(&sql)?;
    let result = stmt.query_map(params![fts_expr, project_path, limit as i64], |row| {
        let rank: f64 = -row.get::<_, f64>(9)?;
        Ok(SearchResult { fact: row_to_fact(row).unwrap(), rank })
    })?
    .collect::<Result<Vec<_>, _>>();
    result.map_err(Into::into)
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
        insert(&conn, "s1", project, "Auth uses JWT tokens in httpOnly cookies", FactCategory::Decision, None).unwrap();
        insert(&conn, "s1", project, "Bug fixed: null pointer in LoginForm.tsx line 203", FactCategory::Fix, None).unwrap();

        let results = search(&conn, project, "login bug", 5, &SearchOptions::default()).unwrap();
        assert!(!results.is_empty());
        assert!(results[0].fact.content.contains("LoginForm"));
    }

    #[test]
    fn deduplication() {
        let (_dir, conn) = test_db();
        let project = Path::new("/test/project");
        let id1 = insert(&conn, "s1", project, "Same content here", FactCategory::Context, None).unwrap();
        let id2 = insert(&conn, "s2", project, "Same content here", FactCategory::Context, None).unwrap();
        assert_eq!(id1, id2);
        assert_eq!(count(&conn, project).unwrap(), 1);
    }

    #[test]
    fn fts_tokenisation() {
        assert_eq!(fts_tokens("login bug"), vec!["login*", "bug*"]);
        assert_eq!(fts_tokens("AND OR"), Vec::<String>::new()); // FTS5 keywords are dropped
        assert_eq!(fts_tokens("NOT NEAR"), Vec::<String>::new());
        // keywords mixed with real terms — keywords are stripped, terms kept
        assert_eq!(fts_tokens("syntax error near \"*\""), vec!["syntax*", "error*"]);
        assert_eq!(fts_tokens("\"quoted\""), vec!["quoted*"]);
        assert!(fts_tokens("  ").is_empty());
        // CLI flags: leading dashes are stripped to avoid invalid FTS5 like `-*` / `--stat*`
        assert_eq!(fts_tokens("git --stat -v"), vec!["git*", "stat*", "v*"]);
        assert!(fts_tokens("---").is_empty()); // bare dashes → empty → dropped
    }

    #[test]
    fn search_uses_and_when_results_exist() {
        let (_dir, conn) = test_db();
        let project = Path::new("/test/project");
        // Only this fact matches BOTH "login" AND "bug"
        insert(&conn, "s1", project, "login bug fix in auth flow", FactCategory::Fix, None).unwrap();
        // This fact only matches "login" — should NOT appear under AND semantics
        insert(&conn, "s1", project, "login session handling", FactCategory::Convention, None).unwrap();

        let results = search(&conn, project, "login bug", 5, &SearchOptions::default()).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].fact.content.contains("bug"));
    }

    #[test]
    fn search_falls_back_to_or_when_and_yields_nothing() {
        let (_dir, conn) = test_db();
        let project = Path::new("/test/project");
        // No fact matches both "login" AND "database" — OR fallback should return both
        insert(&conn, "s1", project, "login session handling convention", FactCategory::Convention, None).unwrap();
        insert(&conn, "s1", project, "database migration strategy", FactCategory::Decision, None).unwrap();

        let results = search(&conn, project, "login database", 5, &SearchOptions::default()).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn delete_fact() {
        let (_dir, conn) = test_db();
        let project = Path::new("/test/project");
        let id = insert(&conn, "s1", project, "Fact to delete soon", FactCategory::Context, None).unwrap();
        assert!(delete(&conn, &id).unwrap());
        assert!(!delete(&conn, &id).unwrap()); // second delete is a no-op
    }

    #[test]
    fn archive_fact_excludes_from_search() {
        let (_dir, conn) = test_db();
        let project = Path::new("/test/project");
        let id = insert(&conn, "s1", project, "Old gRPC convention we follow", FactCategory::Convention, None).unwrap();
        // Force last_accessed to 200 days ago
        let old_ts = Utc::now().timestamp() - 200 * 86_400;
        conn.execute("UPDATE facts SET last_accessed = ?1 WHERE id = ?2", params![old_ts, id]).unwrap();

        let archived = archive_facts_older_than(&conn, project, 180).unwrap();
        assert_eq!(archived, vec![id.clone()]);

        // Regular search must hide the archived fact
        let results = search(&conn, project, "gRPC", 10, &SearchOptions::default()).unwrap();
        assert!(results.is_empty(), "archived facts must be hidden from search");

        // search_including_archived must still find it
        let all = search_including_archived(&conn, project, "gRPC", 10).unwrap();
        assert_eq!(all.len(), 1);
        assert!(all[0].fact.archived_at.is_some());
    }

    #[test]
    fn archive_respects_threshold() {
        let (_dir, conn) = test_db();
        let project = Path::new("/test/project");
        // Recent fact — should NOT be archived
        insert(&conn, "s1", project, "Recent convention not to be archived", FactCategory::Convention, None).unwrap();
        let archived = archive_facts_older_than(&conn, project, 180).unwrap();
        assert!(archived.is_empty(), "recently accessed fact must not be archived");
    }

    #[test]
    fn unarchive_restores_to_search() {
        let (_dir, conn) = test_db();
        let project = Path::new("/test/project");
        let id = insert(&conn, "s1", project, "Convention to restore later", FactCategory::Convention, None).unwrap();
        let old_ts = Utc::now().timestamp() - 200 * 86_400;
        conn.execute("UPDATE facts SET last_accessed = ?1 WHERE id = ?2", params![old_ts, id]).unwrap();
        archive_facts_older_than(&conn, project, 180).unwrap();

        assert!(unarchive_fact(&conn, &id).unwrap());
        let results = search(&conn, project, "restore", 10, &SearchOptions::default()).unwrap();
        assert_eq!(results.len(), 1, "unarchived fact must reappear in regular search");
    }

    #[test]
    fn empty_query_excludes_archived() {
        let (_dir, conn) = test_db();
        let project = Path::new("/test/project");
        let id = insert(&conn, "s1", project, "Old convention to archive", FactCategory::Convention, None).unwrap();
        let old_ts = Utc::now().timestamp() - 200 * 86_400;
        conn.execute("UPDATE facts SET last_accessed = ?1 WHERE id = ?2", params![old_ts, id]).unwrap();
        archive_facts_older_than(&conn, project, 180).unwrap();

        let results = search(&conn, project, "", 10, &SearchOptions::default()).unwrap();
        assert!(results.is_empty(), "empty-query must not return archived facts");
    }

    #[test]
    fn decision_supersedes_similar_old_decision() {
        let (_dir, conn) = test_db();
        let project = Path::new("/test/project");

        // Simulate embeddings: two vectors with >0.75 cosine similarity.
        // We use identical vectors (similarity = 1.0) to guarantee the threshold fires.
        let old_emb: Vec<f32> = vec![1.0, 0.0, 0.0, 0.0];
        let new_emb: Vec<f32> = vec![1.0, 0.0, 0.0, 0.0];

        let old_id = insert(&conn, "s1", project, "we use Redux for state management",
            FactCategory::Decision, Some(&old_emb)).unwrap();

        // New decision supersedes the old one.
        let new_id = insert(&conn, "s2", project, "we use Zustand for state management",
            FactCategory::Decision, Some(&new_emb)).unwrap();

        assert_ne!(old_id, new_id, "supersede must insert a fresh fact, not return the old id");

        // Old fact must be archived.
        let archived_at: Option<i64> = conn.query_row(
            "SELECT archived_at FROM facts WHERE id = ?1",
            params![old_id],
            |r| r.get(0),
        ).unwrap();
        assert!(archived_at.is_some(), "superseded fact must be archived");

        // New fact must appear in regular search.
        let results = search(&conn, project, "state management", 5, &SearchOptions::default()).unwrap();
        assert!(results.iter().any(|r| r.fact.id == new_id), "new fact must be searchable");
        assert!(results.iter().all(|r| r.fact.id != old_id), "superseded fact must be hidden");
    }

    #[test]
    fn fix_merges_instead_of_superseding() {
        let (_dir, conn) = test_db();
        let project = Path::new("/test/project");

        let emb: Vec<f32> = vec![1.0, 0.0, 0.0, 0.0];

        let old_id = insert(&conn, "s1", project, "Fixed null pointer in auth/refresh.ts",
            FactCategory::Fix, Some(&emb)).unwrap();
        let returned_id = insert(&conn, "s2", project, "Fixed null pointer in auth/refresh.ts again",
            FactCategory::Fix, Some(&emb)).unwrap();

        assert_eq!(old_id, returned_id, "fix dedup must return existing id, not supersede");

        let archived_at: Option<i64> = conn.query_row(
            "SELECT archived_at FROM facts WHERE id = ?1",
            params![old_id],
            |r| r.get(0),
        ).unwrap();
        assert!(archived_at.is_none(), "fix facts must not be archived on dedup");
    }

    #[test]
    fn empty_query_ranks_convention_above_file() {
        let (_dir, conn) = test_db();
        let project = Path::new("/test/project");

        // File fact: created now, zero accesses
        insert(&conn, "s1", project, "src/utils/helpers.ts utility exports", FactCategory::File, None).unwrap();

        // Convention fact: 10 days old, accessed 10 times
        let cid = insert(&conn, "s1", project, "always use snake_case for Rust modules", FactCategory::Convention, None).unwrap();
        let old_ts = Utc::now().timestamp() - 10 * 86_400;
        conn.execute("UPDATE facts SET created_at = ?1, access_count = 10 WHERE id = ?2", params![old_ts, cid]).unwrap();

        let results = search(&conn, project, "", 10, &SearchOptions::default()).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].fact.category, FactCategory::Convention,
            "convention with accesses must rank above file fact");
    }

    #[test]
    fn empty_query_scored_fallback_excludes_archived() {
        let (_dir, conn) = test_db();
        let project = Path::new("/test/project");
        let id = insert(&conn, "s1", project, "archived old convention", FactCategory::Convention, None).unwrap();
        let old_ts = Utc::now().timestamp() - 200 * 86_400;
        conn.execute("UPDATE facts SET last_accessed = ?1 WHERE id = ?2", params![old_ts, id]).unwrap();
        archive_facts_older_than(&conn, project, 180).unwrap();

        let results = search(&conn, project, "", 10, &SearchOptions::default()).unwrap();
        assert!(results.is_empty(), "archived facts must be excluded from empty-query scored fallback");
    }
}
