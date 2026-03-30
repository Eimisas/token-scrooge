pub mod facts;
pub mod migrations;
pub mod sessions;
pub mod stats;

use crate::config::db_path;
use anyhow::Result;
use rusqlite::{Connection, OpenFlags};
use std::path::Path;

pub fn open(scrooge_dir: &Path) -> Result<Connection> {
    std::fs::create_dir_all(scrooge_dir)?;
    let path = db_path(scrooge_dir);
    let conn = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
    )?;
    configure(&conn)?;
    migrations::run(&conn)?;
    Ok(conn)
}

fn configure(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous  = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA cache_size   = -8000;
         PRAGMA temp_store   = MEMORY;
         PRAGMA mmap_size    = 134217728;",
    )?;
    Ok(())
}
