use anyhow::Result;
use rusqlite::Connection;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open;
    use tempfile::TempDir;

    #[test]
    fn migration_2_adds_archived_at_on_fresh_db() {
        let dir  = TempDir::new().unwrap();
        let conn = open(dir.path()).unwrap();
        // Query panics if the column doesn't exist — that's the assertion.
        let _: Option<i64> = conn
            .query_row("SELECT archived_at FROM facts LIMIT 1", [], |r| r.get(0))
            .unwrap_or(None);
    }

    #[test]
    fn migration_2_runs_on_db_with_only_migration_1() {
        let dir  = TempDir::new().unwrap();
        let path = dir.path().join("memory.db");
        {
            // Manually apply only migration 1 and pin user_version = 1.
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(MIGRATIONS[0]).unwrap();
            conn.pragma_update(None, "user_version", 1i32).unwrap();
        }
        // Re-open via db::open — should apply migration 2 only.
        let conn = open(dir.path()).unwrap();
        let _: Option<i64> = conn
            .query_row("SELECT archived_at FROM facts LIMIT 1", [], |r| r.get(0))
            .unwrap_or(None);
    }
}

pub fn run(conn: &Connection) -> Result<()> {
    let version: i32 =
        conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    for (n, migration) in MIGRATIONS.iter().enumerate() {
        let n = n as i32 + 1;
        if version < n {
            conn.execute_batch(migration)?;
            conn.pragma_update(None, "user_version", n)?;
        }
    }
    Ok(())
}

const MIGRATIONS: &[&str] = &[
    // Migration 1: core schema
    "
    CREATE TABLE IF NOT EXISTS sessions (
        id                     TEXT    PRIMARY KEY,
        project_path           TEXT    NOT NULL,
        started_at             INTEGER NOT NULL,
        ended_at               INTEGER,
        last_assistant_message TEXT,
        facts_extracted        INTEGER NOT NULL DEFAULT 0,
        tokens_injected        INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX IF NOT EXISTS idx_sessions_project
        ON sessions(project_path, started_at DESC);

    CREATE TABLE IF NOT EXISTS facts (
        id           TEXT    PRIMARY KEY,
        session_id   TEXT    NOT NULL,
        project_path TEXT    NOT NULL,
        content      TEXT    NOT NULL,
        category     TEXT    NOT NULL,
        content_hash TEXT    NOT NULL,
        created_at   INTEGER NOT NULL,
        last_accessed INTEGER,
        access_count INTEGER NOT NULL DEFAULT 0,
        FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
    );
    CREATE UNIQUE INDEX IF NOT EXISTS idx_facts_hash
        ON facts(project_path, content_hash);
    CREATE INDEX IF NOT EXISTS idx_facts_project_time
        ON facts(project_path, created_at DESC);

    CREATE VIRTUAL TABLE IF NOT EXISTS facts_fts USING fts5(
        content,
        category,
        content='facts',
        content_rowid='rowid',
        tokenize='porter ascii'
    );

    CREATE TRIGGER IF NOT EXISTS facts_ai AFTER INSERT ON facts BEGIN
        INSERT INTO facts_fts(rowid, content, category)
        VALUES (new.rowid, new.content, new.category);
    END;
    CREATE TRIGGER IF NOT EXISTS facts_ad AFTER DELETE ON facts BEGIN
        INSERT INTO facts_fts(facts_fts, rowid, content, category)
        VALUES ('delete', old.rowid, old.content, old.category);
    END;
    CREATE TRIGGER IF NOT EXISTS facts_au AFTER UPDATE ON facts BEGIN
        INSERT INTO facts_fts(facts_fts, rowid, content, category)
        VALUES ('delete', old.rowid, old.content, old.category);
        INSERT INTO facts_fts(rowid, content, category)
        VALUES (new.rowid, new.content, new.category);
    END;

    CREATE TABLE IF NOT EXISTS token_stats (
        id              INTEGER PRIMARY KEY AUTOINCREMENT,
        session_id      TEXT    NOT NULL,
        event           TEXT    NOT NULL,
        tokens_before   INTEGER NOT NULL,
        tokens_injected INTEGER NOT NULL,
        facts_count     INTEGER NOT NULL,
        recorded_at     INTEGER NOT NULL,
        FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
    );
    ",

    // Migration 2: soft-delete / archival
    "
    ALTER TABLE facts ADD COLUMN archived_at INTEGER;
    CREATE INDEX IF NOT EXISTS idx_facts_archived
        ON facts(project_path, archived_at)
        WHERE archived_at IS NULL;
    ",
];
