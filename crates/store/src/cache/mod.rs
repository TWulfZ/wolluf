//! cache.db: disposable derived data (architecture §5.2, §5.4, spec 003).

use std::path::{Path, PathBuf};

use rusqlite::{Connection, ErrorCode as SqliteCode, OpenFlags};

use crate::db::{DbHandle, DbKind};
use crate::error::StoreError;

/// Bumped on any schema change; a mismatch deletes and rebuilds the file.
pub const CACHE_SCHEMA_VERSION: u32 = 3;

const SCHEMA: &str = include_str!("schema.sql");
const SIDECARS: [&str; 2] = ["-wal", "-shm"];

/// Opens cache.db, first deleting it when its version differs or SQLite reports it corrupt.
/// Only this file set is ever deleted; user.db is never touched from here.
pub fn open_cache_db(path: &Path) -> Result<DbHandle, StoreError> {
    if needs_rebuild(path)? {
        delete_file_set(path)?;
    }
    DbHandle::open(path, DbKind::Cache, |conn| {
        let version: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version == 0 {
            let tx = conn.transaction()?;
            tx.execute_batch(SCHEMA)?;
            tx.pragma_update(None, "user_version", CACHE_SCHEMA_VERSION)?;
            tx.commit()?;
        }
        Ok(())
    })
}

fn needs_rebuild(path: &Path) -> Result<bool, StoreError> {
    if !path.exists() {
        return Ok(false);
    }
    match probe_version(path) {
        Ok(version) => Ok(version != CACHE_SCHEMA_VERSION),
        Err(rusqlite::Error::SqliteFailure(e, _))
            if matches!(
                e.code,
                SqliteCode::DatabaseCorrupt | SqliteCode::NotADatabase
            ) =>
        {
            Ok(true)
        }
        Err(e) => Err(e.into()),
    }
}

/// Reading `sqlite_schema` forces SQLite to parse the header and schema pages, which is where a
/// garbage or truncated file shows up; `user_version` alone only reads the header.
fn probe_version(path: &Path) -> Result<u32, rusqlite::Error> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let _: i64 = conn.query_row("SELECT count(*) FROM sqlite_schema", [], |r| r.get(0))?;
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
}

fn delete_file_set(path: &Path) -> Result<(), StoreError> {
    // Sidecars first: a stale -wal left next to a fresh main file would be replayed into it.
    let sidecars = SIDECARS.iter().map(|suffix| {
        let mut os = path.as_os_str().to_owned();
        os.push(suffix);
        PathBuf::from(os)
    });
    for file in sidecars.chain([path.to_path_buf()]) {
        match std::fs::remove_file(&file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(StoreError::io(file, e)),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;
    use sha2::{Digest, Sha256};
    use wolluf_core::UnixUs;

    use super::*;
    use crate::user::open_user_db;

    const TABLES: [&str; 8] = [
        "alias_stats",
        "catalog_chart",
        "chart_label",
        "chart_parsed",
        "derivation",
        "item_failure",
        "job_run",
        "segment",
    ];

    fn tables(db: &DbHandle) -> Vec<String> {
        db.read(|c| {
            let mut stmt = c.0.prepare(
                "SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
            )?;
            let names = stmt
                .query_map([], |r| r.get(0))?
                .collect::<Result<Vec<String>, _>>()?;
            Ok(names)
        })
        .unwrap()
    }

    fn job_rows(db: &DbHandle) -> i64 {
        db.read(|c| {
            Ok(c.0.query_row("SELECT count(*) FROM job_run", [], |r| r.get(0))?)
        })
        .unwrap()
    }

    fn add_job(db: &DbHandle) {
        db.write(|tx| {
            tx.0.execute(
                "INSERT INTO job_run (id, kind, params_json, status) VALUES ('j', 'sync_plays', '{}', 'queued')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    }

    fn version(path: &Path) -> i64 {
        Connection::open(path)
            .unwrap()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap()
    }

    fn sidecar(path: &Path, suffix: &str) -> std::path::PathBuf {
        let mut os = path.as_os_str().to_owned();
        os.push(suffix);
        os.into()
    }

    #[test]
    fn fresh_cache_has_current_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.db");
        let db = open_cache_db(&path).unwrap();
        assert_eq!(tables(&db), TABLES);
        add_job(&db);
        drop(db);
        assert_eq!(version(&path), i64::from(CACHE_SCHEMA_VERSION));
        // Same version: the data survives a reopen.
        assert_eq!(job_rows(&open_cache_db(&path).unwrap()), 1);
    }

    #[test]
    fn version_mismatch_deletes_and_recreates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.db");
        let db = open_cache_db(&path).unwrap();
        add_job(&db);
        drop(db);
        Connection::open(&path)
            .unwrap()
            .execute_batch("PRAGMA user_version = 99;")
            .unwrap();

        let db = open_cache_db(&path).unwrap();
        assert_eq!(tables(&db), TABLES);
        assert_eq!(job_rows(&db), 0);
        drop(db);
        assert_eq!(version(&path), i64::from(CACHE_SCHEMA_VERSION));
    }

    #[test]
    fn older_cache_is_rebuilt_to_current_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.db");
        let v1 = Connection::open(&path).unwrap();
        v1.execute_batch(
            "CREATE TABLE job_run (id TEXT PRIMARY KEY, kind TEXT NOT NULL, params_json TEXT NOT NULL,
                 status TEXT NOT NULL, started TEXT NULL, ended TEXT NULL, summary_json TEXT NULL) STRICT;
             INSERT INTO job_run (id, kind, params_json, status) VALUES ('j', 'sync_plays', '{}', 'queued');
             PRAGMA user_version = 1;",
        )
        .unwrap();
        drop(v1);

        let db = open_cache_db(&path).unwrap();
        assert_eq!(tables(&db), TABLES);
        assert_eq!(job_rows(&db), 0);
        drop(db);
        assert_eq!(CACHE_SCHEMA_VERSION, 3);
        assert_eq!(version(&path), 3);
    }

    #[test]
    fn segment_rows_are_strict_and_keyed_by_a_32_byte_vkey() {
        let dir = tempfile::tempdir().unwrap();
        let db = open_cache_db(&dir.path().join("cache.db")).unwrap();
        let insert = "INSERT INTO segment (md5, vkey, idx, t0_us, t1_us, cols, axis_id, pattern_id,
                secondary_json, purity, strength) VALUES ('m', ?1, 0, 0, 1, 3, 'a', 'p', '[]', 1000, 500)";
        let short = db.write(|tx| Ok(tx.0.execute(insert, [vec![1_u8; 31]])?));
        assert!(short.is_err());
        let backwards = db.write(|tx| {
            Ok(tx.0.execute(
                "INSERT INTO segment VALUES ('m', ?1, 0, 5, 1, 3, 'a', 'p', '[]', 1000, 500)",
                [vec![1_u8; 32]],
            )?)
        });
        assert!(backwards.is_err(), "t1 before t0");
        let not_json = db.write(|tx| {
            Ok(tx.0.execute(
                "INSERT INTO segment VALUES ('m', ?1, 1, 0, 1, 3, 'a', 'p', '[oops', 1000, 500)",
                [vec![1_u8; 32]],
            )?)
        });
        assert!(not_json.is_err(), "secondary_json must be JSON");
        assert_eq!(
            db.write(|tx| Ok(tx.0.execute(insert, [vec![1_u8; 32]])?))
                .unwrap(),
            1
        );
    }

    #[test]
    fn garbage_file_is_rebuilt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.db");
        let garbage = b"this is not an sqlite database, only some bytes ".repeat(64);
        std::fs::write(&path, &garbage).unwrap();
        std::fs::write(sidecar(&path, "-wal"), &garbage).unwrap();
        std::fs::write(sidecar(&path, "-shm"), &garbage).unwrap();

        let db = open_cache_db(&path).unwrap();
        assert_eq!(tables(&db), TABLES);
        assert_eq!(job_rows(&db), 0);
        drop(db);
        // The stale sidecars went with the main file instead of being replayed into it.
        assert_ne!(
            std::fs::read(sidecar(&path, "-wal")).ok().as_deref(),
            Some(garbage.as_slice())
        );
    }

    #[test]
    fn rebuild_leaves_user_db_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user.db");
        drop(open_user_db(&user, &dir.path().join("backups"), UnixUs(0)).unwrap());
        let before = Sha256::digest(std::fs::read(&user).unwrap());

        std::fs::write(dir.path().join("cache.db"), b"garbage".repeat(200)).unwrap();
        drop(open_cache_db(&dir.path().join("cache.db")).unwrap());

        assert_eq!(Sha256::digest(std::fs::read(&user).unwrap()), before);
    }
}
