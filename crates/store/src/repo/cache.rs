//! cache.db repositories (spec 003 Design `repo/cache.rs`).

use std::collections::BTreeMap;

use wolluf_core::{ChartMd5, ErrorCode, StageId, UnixUs, VersionKey};

use rusqlite::{OptionalExtension, Row};

use crate::db::{Conn, Tx};
use crate::error::StoreError;
use crate::repo::ledger::SnapshotId;
use crate::repo::sql::{enum_col, fixed, int, json_value, opt_parsed, opt_time, parsed, str_enum};
use crate::time::format_rfc3339_ms;

/// One mania entry of osu!.db (spec 003 step 1).
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogChart {
    pub md5: ChartMd5,
    /// `round(CS)` as osu! stores it; not validated against `Keymode`, so odd maps still list.
    pub keymode: u8,
    pub title: String,
    pub artist: String,
    pub version: String,
    pub creator: String,
    pub set_id: Option<i32>,
    pub beatmap_id: Option<i32>,
    /// Relative to the install's `Songs` directory.
    pub path: String,
    pub od: f64,
    pub hp: f64,
    pub length_ms: u32,
}

str_enum! {
    pub enum DerivationStatus {
        Ok => "ok",
        Failed => "failed",
        Skipped => "skipped",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Derivation {
    pub stage: StageId,
    pub input_key: String,
    pub vkey: VersionKey,
    pub status: DerivationStatus,
    pub error_code: Option<ErrorCode>,
    pub error_msg: Option<String>,
    pub duration_ms: Option<u32>,
}

str_enum! {
    pub enum JobStatus {
        Queued => "queued",
        Running => "running",
        Ok => "ok",
        Failed => "failed",
        Cancelled => "cancelled",
    }
}

impl JobStatus {
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Ok | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewJobRun {
    pub id: ulid::Ulid,
    pub kind: String,
    pub params: serde_json::Value,
    pub status: JobStatus,
    pub started: Option<UnixUs>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JobRun {
    pub id: ulid::Ulid,
    pub kind: String,
    pub params: serde_json::Value,
    pub status: JobStatus,
    pub started: Option<UnixUs>,
    pub ended: Option<UnixUs>,
    pub summary: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemFailure {
    pub job_id: ulid::Ulid,
    /// A play id hex, chart md5 or file name: whatever identifies the item to the user.
    pub item_ref: String,
    pub code: ErrorCode,
    pub message: String,
}

/// A normalized chart under one `chart_parse` key.
#[derive(Debug, Clone, PartialEq)]
pub struct ChartParsed {
    pub md5: ChartMd5,
    pub vkey: VersionKey,
    /// Opaque to the store: the engine owns the encoding and its format-version header.
    pub rows_blob: Vec<u8>,
    pub n_notes: u32,
    pub n_ln: u32,
    pub ln_ratio: f64,
    pub length_ms: u32,
}

/// `ChartParsed` without its blob, for scans over the whole library.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParsedSummary {
    pub md5: ChartMd5,
    pub n_notes: u32,
    pub n_ln: u32,
    pub ln_ratio: f64,
    pub length_ms: u32,
}

impl ChartParsed {
    pub fn summary(&self) -> ParsedSummary {
        ParsedSummary {
            md5: self.md5,
            n_notes: self.n_notes,
            n_ln: self.n_ln,
            ln_ratio: self.ln_ratio,
            length_ms: self.length_ms,
        }
    }
}

/// One source label of a chart; the chart and key are given by the call that stores it.
#[derive(Debug, Clone, PartialEq)]
pub struct ChartLabel {
    pub source: String,
    pub scale: String,
    /// `None` when the level has no position on its scale (e.g. `gamma_entry`).
    pub level_ord: Option<f64>,
    pub level_text: String,
    pub skill_tag: Option<String>,
    pub is_variant: bool,
}

/// Level bounds are inclusive; a bound excludes labels without a `level_ord`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LabelFilter {
    pub scale: Option<String>,
    pub level_min: Option<f64>,
    pub level_max: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LabelCount {
    pub rows: u64,
    pub charts: u64,
}

fn ms(t: UnixUs) -> String {
    format_rfc3339_ms(t)
}

/// `DELETE … WHERE vkey NOT IN (keep)`; an empty `keep` clears the table.
fn prune_vkeys_except(tx: &Tx<'_>, table: &str, keep: &[VersionKey]) -> Result<u64, StoreError> {
    let placeholders = vec!["?"; keep.len()].join(", ");
    let n = tx.0.execute(
        &format!("DELETE FROM {table} WHERE vkey NOT IN ({placeholders})"),
        rusqlite::params_from_iter(keep.iter().map(|v| v.0)),
    )?;
    Ok(n as u64)
}

pub mod catalog_chart {
    use super::*;

    const COLUMNS: &str = "md5, keymode, title, artist, version, creator, set_id, beatmap_id, path, od, hp, length_ms";

    fn from_row(row: &Row<'_>) -> rusqlite::Result<CatalogChart> {
        Ok(CatalogChart {
            md5: parsed(row, 0, str::parse::<ChartMd5>)?,
            keymode: int(row, 1)?,
            title: row.get(2)?,
            artist: row.get(3)?,
            version: row.get(4)?,
            creator: row.get(5)?,
            set_id: row.get(6)?,
            beatmap_id: row.get(7)?,
            path: row.get(8)?,
            od: row.get(9)?,
            hp: row.get(10)?,
            length_ms: int(row, 11)?,
        })
    }

    /// Replaces the whole catalog inside the caller's transaction, so readers see either the
    /// old snapshot or the new one, never a mix (spec 003 step 1).
    pub fn replace_all(
        tx: &Tx<'_>,
        snapshot_id: SnapshotId,
        rows: &[CatalogChart],
    ) -> Result<(), StoreError> {
        tx.0.execute("DELETE FROM catalog_chart", [])?;
        let mut insert = tx.0.prepare_cached(&format!(
            "INSERT INTO catalog_chart ({COLUMNS}, snapshot_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"
        ))?;
        for c in rows {
            insert.execute(rusqlite::params![
                c.md5.to_string(),
                c.keymode,
                c.title,
                c.artist,
                c.version,
                c.creator,
                c.set_id,
                c.beatmap_id,
                c.path,
                c.od,
                c.hp,
                c.length_ms,
                snapshot_id.0,
            ])?;
        }
        Ok(())
    }

    pub fn get(conn: Conn<'_>, md5: ChartMd5) -> Result<Option<CatalogChart>, StoreError> {
        Ok(conn
            .0
            .query_row(
                &format!("SELECT {COLUMNS} FROM catalog_chart WHERE md5 = ?1"),
                [md5.to_string()],
                from_row,
            )
            .optional()?)
    }

    /// Ordered by md5, so two catalogs compare equal exactly when their contents do (AC16).
    pub fn list_all(conn: Conn<'_>) -> Result<Vec<CatalogChart>, StoreError> {
        let mut stmt = conn
            .0
            .prepare(&format!("SELECT {COLUMNS} FROM catalog_chart ORDER BY md5"))?;
        let rows = stmt.query_map([], from_row)?.collect::<Result<_, _>>()?;
        Ok(rows)
    }

    pub fn list_by_keymode(conn: Conn<'_>, keymode: u8) -> Result<Vec<CatalogChart>, StoreError> {
        let mut stmt = conn.0.prepare_cached(&format!(
            "SELECT {COLUMNS} FROM catalog_chart WHERE keymode = ?1 ORDER BY md5"
        ))?;
        let rows = stmt
            .query_map([keymode], from_row)?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    pub fn keymodes(conn: Conn<'_>) -> Result<BTreeMap<ChartMd5, u8>, StoreError> {
        let mut stmt = conn.0.prepare("SELECT md5, keymode FROM catalog_chart")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((parsed(row, 0, str::parse::<ChartMd5>)?, int(row, 1)?))
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    /// The osu!.db snapshot the catalog was built from; `None` while the catalog is empty.
    pub fn snapshot_id(conn: Conn<'_>) -> Result<Option<SnapshotId>, StoreError> {
        let id: Option<i64> =
            conn.0
                .query_row("SELECT max(snapshot_id) FROM catalog_chart", [], |r| {
                    r.get(0)
                })?;
        Ok(id.map(SnapshotId))
    }
}

pub mod derivation {
    use super::*;

    const COLUMNS: &str = "stage, input_key, vkey, status, error_code, error_msg, duration_ms";

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Derivation> {
        Ok(Derivation {
            stage: parsed(row, 0, StageId::parse)?,
            input_key: row.get(1)?,
            vkey: VersionKey(fixed(row, 2)?),
            status: enum_col(row, 3, DerivationStatus::parse)?,
            error_code: opt_parsed(row, 4, str::parse::<ErrorCode>)?,
            error_msg: row.get(5)?,
            duration_ms: row.get(6)?,
        })
    }

    /// Upsert: a rerun of the same (stage, input, vkey) records its latest outcome.
    pub fn put(tx: &Tx<'_>, d: &Derivation) -> Result<(), StoreError> {
        tx.0.prepare_cached(&format!(
            "INSERT OR REPLACE INTO derivation ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
        ))?
        .execute((
            d.stage.as_str(),
            &d.input_key,
            d.vkey.0,
            d.status.as_str(),
            d.error_code.map(ErrorCode::as_str),
            &d.error_msg,
            d.duration_ms,
        ))?;
        Ok(())
    }

    pub fn get(
        conn: Conn<'_>,
        stage: &StageId,
        input_key: &str,
        vkey: VersionKey,
    ) -> Result<Option<Derivation>, StoreError> {
        Ok(conn
            .0
            .prepare_cached(&format!(
                "SELECT {COLUMNS} FROM derivation
                 WHERE stage = ?1 AND input_key = ?2 AND vkey = ?3"
            ))?
            .query_row((stage.as_str(), input_key, vkey.0), from_row)
            .optional()?)
    }

    pub fn list_all(conn: Conn<'_>) -> Result<Vec<Derivation>, StoreError> {
        let mut stmt = conn.0.prepare(&format!(
            "SELECT {COLUMNS} FROM derivation ORDER BY stage, input_key, vkey"
        ))?;
        let rows = stmt.query_map([], from_row)?.collect::<Result<_, _>>()?;
        Ok(rows)
    }
}

pub mod job_run {
    use super::*;

    const COLUMNS: &str = "id, kind, params_json, status, started, ended, summary_json";

    fn from_row(row: &Row<'_>) -> rusqlite::Result<JobRun> {
        let summary: Option<String> = row.get(6)?;
        Ok(JobRun {
            id: parsed(row, 0, ulid::Ulid::from_string)?,
            kind: row.get(1)?,
            params: json_value(row, 2)?,
            status: enum_col(row, 3, JobStatus::parse)?,
            started: opt_time(row, 4)?,
            ended: opt_time(row, 5)?,
            summary: match summary {
                Some(_) => Some(json_value(row, 6)?),
                None => None,
            },
        })
    }

    pub fn insert(tx: &Tx<'_>, job: &NewJobRun) -> Result<(), StoreError> {
        tx.0.execute(
            &format!("INSERT INTO job_run ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL)"),
            (
                job.id.to_string(),
                &job.kind,
                job.params.to_string(),
                job.status.as_str(),
                job.started.map(ms),
            ),
        )?;
        Ok(())
    }

    pub fn start(tx: &Tx<'_>, id: ulid::Ulid, started: UnixUs) -> Result<bool, StoreError> {
        let n = tx.0.execute(
            "UPDATE job_run SET status = 'running', started = ?2 WHERE id = ?1",
            (id.to_string(), ms(started)),
        )?;
        Ok(n == 1)
    }

    pub fn finish(
        tx: &Tx<'_>,
        id: ulid::Ulid,
        status: JobStatus,
        ended: UnixUs,
        summary: &serde_json::Value,
    ) -> Result<bool, StoreError> {
        if !status.is_terminal() {
            return Err(StoreError::InvalidData(format!(
                "job cannot finish as {}",
                status.as_str()
            )));
        }
        let n = tx.0.execute(
            "UPDATE job_run SET status = ?2, ended = ?3, summary_json = ?4 WHERE id = ?1",
            (
                id.to_string(),
                status.as_str(),
                ms(ended),
                summary.to_string(),
            ),
        )?;
        Ok(n == 1)
    }

    pub fn get(conn: Conn<'_>, id: ulid::Ulid) -> Result<Option<JobRun>, StoreError> {
        Ok(conn
            .0
            .query_row(
                &format!("SELECT {COLUMNS} FROM job_run WHERE id = ?1"),
                [id.to_string()],
                from_row,
            )
            .optional()?)
    }

    /// Newest first; ULIDs sort by creation time.
    pub fn list_recent(conn: Conn<'_>, limit: u32) -> Result<Vec<JobRun>, StoreError> {
        let mut stmt = conn.0.prepare(&format!(
            "SELECT {COLUMNS} FROM job_run ORDER BY id DESC LIMIT ?1"
        ))?;
        let rows = stmt
            .query_map([limit], from_row)?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }
}

pub mod item_failure {
    use super::*;

    /// Keeps the first failure recorded for an item in a job; returns whether this one was new.
    pub fn insert(tx: &Tx<'_>, f: &ItemFailure) -> Result<bool, StoreError> {
        let n =
            tx.0.prepare_cached(
                "INSERT INTO item_failure (job_id, item_ref, code, message) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (job_id, item_ref) DO NOTHING",
            )?
            .execute((
                f.job_id.to_string(),
                &f.item_ref,
                f.code.as_str(),
                &f.message,
            ))?;
        Ok(n == 1)
    }

    pub fn list(conn: Conn<'_>, job_id: ulid::Ulid) -> Result<Vec<ItemFailure>, StoreError> {
        let mut stmt = conn.0.prepare(
            "SELECT job_id, item_ref, code, message FROM item_failure
             WHERE job_id = ?1 ORDER BY item_ref",
        )?;
        let rows = stmt
            .query_map([job_id.to_string()], |row| {
                Ok(ItemFailure {
                    job_id: parsed(row, 0, ulid::Ulid::from_string)?,
                    item_ref: row.get(1)?,
                    code: parsed(row, 2, str::parse::<ErrorCode>)?,
                    message: row.get(3)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }
}

pub mod chart_parsed {
    use super::*;

    const COLUMNS: &str = "md5, vkey, rows_blob, n_notes, n_ln, ln_ratio, length_ms";

    fn from_row(row: &Row<'_>) -> rusqlite::Result<ChartParsed> {
        Ok(ChartParsed {
            md5: parsed(row, 0, str::parse::<ChartMd5>)?,
            vkey: VersionKey(fixed(row, 1)?),
            rows_blob: row.get(2)?,
            n_notes: int(row, 3)?,
            n_ln: int(row, 4)?,
            ln_ratio: row.get(5)?,
            length_ms: int(row, 6)?,
        })
    }

    /// Upsert: a rerun of the same (md5, vkey) keeps its latest output.
    pub fn put(tx: &Tx<'_>, c: &ChartParsed) -> Result<(), StoreError> {
        tx.0.prepare_cached(&format!(
            "INSERT OR REPLACE INTO chart_parsed ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
        ))?
        .execute((
            c.md5.to_string(),
            c.vkey.0,
            &c.rows_blob,
            c.n_notes,
            c.n_ln,
            c.ln_ratio,
            c.length_ms,
        ))?;
        Ok(())
    }

    pub fn get(
        conn: Conn<'_>,
        md5: ChartMd5,
        vkey: VersionKey,
    ) -> Result<Option<ChartParsed>, StoreError> {
        Ok(conn
            .0
            .prepare_cached(&format!(
                "SELECT {COLUMNS} FROM chart_parsed WHERE md5 = ?1 AND vkey = ?2"
            ))?
            .query_row((md5.to_string(), vkey.0), from_row)
            .optional()?)
    }

    pub fn exists(conn: Conn<'_>, md5: ChartMd5, vkey: VersionKey) -> Result<bool, StoreError> {
        Ok(conn
            .0
            .prepare_cached(
                "SELECT EXISTS (SELECT 1 FROM chart_parsed WHERE md5 = ?1 AND vkey = ?2)",
            )?
            .query_row((md5.to_string(), vkey.0), |r| r.get(0))?)
    }

    pub fn count(conn: Conn<'_>, vkey: VersionKey) -> Result<u64, StoreError> {
        Ok(conn
            .0
            .prepare_cached("SELECT count(*) FROM chart_parsed WHERE vkey = ?1")?
            .query_row([vkey.0], |r| int(r, 0))?)
    }

    /// Every chart parsed under `vkey`, by md5.
    pub fn summaries(conn: Conn<'_>, vkey: VersionKey) -> Result<Vec<ParsedSummary>, StoreError> {
        let mut stmt = conn.0.prepare_cached(
            "SELECT md5, n_notes, n_ln, ln_ratio, length_ms FROM chart_parsed
             WHERE vkey = ?1 ORDER BY md5",
        )?;
        let rows = stmt
            .query_map([vkey.0], |row| {
                Ok(ParsedSummary {
                    md5: parsed(row, 0, str::parse::<ChartMd5>)?,
                    n_notes: int(row, 1)?,
                    n_ln: int(row, 2)?,
                    ln_ratio: row.get(3)?,
                    length_ms: int(row, 4)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    /// GC hook (§5.5 keeps recent keys); returns the number of rows deleted.
    pub fn prune_except(tx: &Tx<'_>, keep: &[VersionKey]) -> Result<u64, StoreError> {
        prune_vkeys_except(tx, "chart_parsed", keep)
    }
}

pub mod chart_label {
    use rusqlite::types::Value;

    use super::*;

    const COLUMNS: &str = "source, scale, level_ord, level_text, skill_tag, is_variant";
    const ORDER: &str = "scale, level_ord IS NULL, level_ord, md5, level_text";

    fn from_row(row: &Row<'_>, at: usize) -> rusqlite::Result<ChartLabel> {
        Ok(ChartLabel {
            source: row.get(at)?,
            scale: row.get(at + 1)?,
            level_ord: row.get(at + 2)?,
            level_text: row.get(at + 3)?,
            skill_tag: row.get(at + 4)?,
            is_variant: row.get(at + 5)?,
        })
    }

    /// Replaces the chart's labels under `vkey`; labels under other keys stay until
    /// `prune_except`. Two labels with the same (scale, level_text) fail the transaction.
    pub fn replace_for(
        tx: &Tx<'_>,
        md5: ChartMd5,
        vkey: VersionKey,
        labels: &[ChartLabel],
    ) -> Result<(), StoreError> {
        let md5 = md5.to_string();
        tx.0.prepare_cached("DELETE FROM chart_label WHERE md5 = ?1 AND vkey = ?2")?
            .execute((&md5, vkey.0))?;
        let mut insert = tx.0.prepare_cached(&format!(
            "INSERT INTO chart_label (md5, vkey, {COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"
        ))?;
        for l in labels {
            insert.execute(rusqlite::params![
                md5,
                vkey.0,
                l.source,
                l.scale,
                l.level_ord,
                l.level_text,
                l.skill_tag,
                l.is_variant,
            ])?;
        }
        Ok(())
    }

    pub fn list_for(
        conn: Conn<'_>,
        md5: ChartMd5,
        vkey: VersionKey,
    ) -> Result<Vec<ChartLabel>, StoreError> {
        let mut stmt = conn.0.prepare_cached(&format!(
            "SELECT {COLUMNS} FROM chart_label WHERE md5 = ?1 AND vkey = ?2 ORDER BY {ORDER}"
        ))?;
        let rows = stmt
            .query_map((md5.to_string(), vkey.0), |row| from_row(row, 0))?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    /// Ordered by scale, then level with unlevelled labels last, then md5.
    pub fn list_filtered(
        conn: Conn<'_>,
        vkey: VersionKey,
        filter: &LabelFilter,
        limit: u32,
    ) -> Result<Vec<(ChartMd5, ChartLabel)>, StoreError> {
        // Clauses are added only when set, so the (vkey, scale, level_ord) index stays usable.
        let mut clauses = vec!["vkey = ?"];
        let mut params = vec![Value::Blob(vkey.0.to_vec())];
        if let Some(scale) = &filter.scale {
            clauses.push("scale = ?");
            params.push(Value::Text(scale.clone()));
        }
        if let Some(min) = filter.level_min {
            clauses.push("level_ord >= ?");
            params.push(Value::Real(min));
        }
        if let Some(max) = filter.level_max {
            clauses.push("level_ord <= ?");
            params.push(Value::Real(max));
        }
        params.push(Value::Integer(i64::from(limit)));
        let mut stmt = conn.0.prepare(&format!(
            "SELECT md5, {COLUMNS} FROM chart_label WHERE {} ORDER BY {ORDER} LIMIT ?",
            clauses.join(" AND ")
        ))?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(params), |row| {
                Ok((parsed(row, 0, str::parse::<ChartMd5>)?, from_row(row, 1)?))
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    pub fn counts_by_scale(
        conn: Conn<'_>,
        vkey: VersionKey,
    ) -> Result<BTreeMap<String, LabelCount>, StoreError> {
        let mut stmt = conn.0.prepare_cached(
            "SELECT scale, count(*), count(DISTINCT md5) FROM chart_label
             WHERE vkey = ?1 GROUP BY scale",
        )?;
        let rows = stmt
            .query_map([vkey.0], |row| {
                Ok((
                    row.get(0)?,
                    LabelCount {
                        rows: int(row, 1)?,
                        charts: int(row, 2)?,
                    },
                ))
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    /// Distinct labelled charts; a chart on several scales counts once.
    pub fn count_charts(conn: Conn<'_>, vkey: VersionKey) -> Result<u64, StoreError> {
        Ok(conn
            .0
            .prepare_cached("SELECT count(DISTINCT md5) FROM chart_label WHERE vkey = ?1")?
            .query_row([vkey.0], |r| int(r, 0))?)
    }

    /// GC hook (§5.5 keeps recent keys); returns the number of rows deleted.
    pub fn prune_except(tx: &Tx<'_>, keep: &[VersionKey]) -> Result<u64, StoreError> {
        prune_vkeys_except(tx, "chart_label", keep)
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use serde_json::json;

    use super::*;
    use crate::cache::open_cache_db;

    const T0: UnixUs = UnixUs(1_790_637_236_636_000);

    fn chart(md5: &str, title: &str) -> CatalogChart {
        CatalogChart {
            md5: ChartMd5::from_str(md5).unwrap(),
            keymode: 7,
            title: title.into(),
            artist: "Artist".into(),
            version: "7K Another".into(),
            creator: "Mapper".into(),
            set_id: Some(123),
            beatmap_id: None,
            path: "123 Artist - Title/map.osu".into(),
            od: 8.5,
            hp: 7.0,
            length_ms: 150_000,
        }
    }

    const MD5_A: &str = "e956977ccc1d74a50ae48b43a868cc20";
    const MD5_B: &str = "0123456789abcdef0123456789abcdef";
    const MD5_C: &str = "ffffffffffffffffffffffffffffffff";

    #[test]
    fn catalog_replace_all_is_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let db = open_cache_db(&dir.path().join("cache.db")).unwrap();
        let first = vec![chart(MD5_B, "B"), chart(MD5_A, "A")];
        let rows = first.clone();
        db.write(move |tx| catalog_chart::replace_all(tx, SnapshotId(1), &rows))
            .unwrap();

        // A duplicate md5 fails midway; the whole replacement must roll back.
        let broken = vec![chart(MD5_C, "C"), chart(MD5_C, "C again")];
        assert!(
            db.write(move |tx| catalog_chart::replace_all(tx, SnapshotId(2), &broken))
                .is_err()
        );
        let (all, snapshot) = db
            .read(|c| Ok((catalog_chart::list_all(c)?, catalog_chart::snapshot_id(c)?)))
            .unwrap();
        assert_eq!(all, first);
        assert_eq!(snapshot, Some(SnapshotId(1)));

        let second = vec![chart(MD5_C, "C")];
        let rows = second.clone();
        db.write(move |tx| catalog_chart::replace_all(tx, SnapshotId(3), &rows))
            .unwrap();
        let (got, missing, keymodes) = db
            .read(|c| {
                Ok((
                    catalog_chart::get(c, ChartMd5::from_str(MD5_C).unwrap())?,
                    catalog_chart::get(c, ChartMd5::from_str(MD5_A).unwrap())?,
                    catalog_chart::keymodes(c)?,
                ))
            })
            .unwrap();
        assert_eq!(got, Some(second[0].clone()));
        assert_eq!(missing, None);
        assert_eq!(keymodes, [(ChartMd5::from_str(MD5_C).unwrap(), 7)].into());
    }

    #[test]
    fn catalog_list_by_keymode() {
        let dir = tempfile::tempdir().unwrap();
        let db = open_cache_db(&dir.path().join("cache.db")).unwrap();
        let four = CatalogChart {
            keymode: 4,
            ..chart(MD5_C, "C")
        };
        let rows = vec![chart(MD5_B, "B"), four, chart(MD5_A, "A")];
        db.write(move |tx| catalog_chart::replace_all(tx, SnapshotId(1), &rows))
            .unwrap();
        let (seven, six) = db
            .read(|c| {
                Ok((
                    catalog_chart::list_by_keymode(c, 7)?,
                    catalog_chart::list_by_keymode(c, 6)?,
                ))
            })
            .unwrap();
        assert_eq!(seven, vec![chart(MD5_B, "B"), chart(MD5_A, "A")]);
        assert!(six.is_empty());
    }

    const VKEY_1: VersionKey = VersionKey([1; 32]);
    const VKEY_2: VersionKey = VersionKey([2; 32]);

    fn md5(s: &str) -> ChartMd5 {
        ChartMd5::from_str(s).unwrap()
    }

    fn parsed_chart(md5_hex: &str, vkey: VersionKey, n_notes: u32) -> ChartParsed {
        ChartParsed {
            md5: md5(md5_hex),
            vkey,
            rows_blob: vec![0x01, 0x28, 0xb5, 0x2f, 0xfd, n_notes as u8],
            n_notes,
            n_ln: n_notes / 4,
            ln_ratio: 0.25,
            length_ms: 123_456,
        }
    }

    #[test]
    fn chart_parsed_reads_are_keyed() {
        let dir = tempfile::tempdir().unwrap();
        let db = open_cache_db(&dir.path().join("cache.db")).unwrap();
        let a1 = parsed_chart(MD5_A, VKEY_1, 1_000);
        let a2 = parsed_chart(MD5_A, VKEY_2, 1_004);
        let b1 = parsed_chart(MD5_B, VKEY_1, 800);
        let rows = [a1.clone(), a2.clone(), b1.clone()];
        db.write(move |tx| rows.iter().try_for_each(|r| chart_parsed::put(tx, r)))
            .unwrap();

        let got = db
            .read(|c| {
                Ok((
                    chart_parsed::get(c, md5(MD5_A), VKEY_1)?,
                    chart_parsed::get(c, md5(MD5_A), VKEY_2)?,
                    chart_parsed::get(c, md5(MD5_C), VKEY_1)?,
                    chart_parsed::exists(c, md5(MD5_B), VKEY_1)?,
                    chart_parsed::exists(c, md5(MD5_B), VKEY_2)?,
                    chart_parsed::count(c, VKEY_1)?,
                    chart_parsed::count(c, VKEY_2)?,
                ))
            })
            .unwrap();
        assert_eq!(got, (Some(a1), Some(a2.clone()), None, true, false, 2, 1));

        // A rerun under the same key replaces the row instead of failing.
        let again = ChartParsed { n_notes: 999, ..b1 };
        let row = again.clone();
        db.write(move |tx| chart_parsed::put(tx, &row)).unwrap();
        assert_eq!(
            db.read(|c| chart_parsed::get(c, md5(MD5_B), VKEY_1))
                .unwrap(),
            Some(again)
        );

        let deleted = db
            .write(|tx| chart_parsed::prune_except(tx, &[VKEY_2]))
            .unwrap();
        assert_eq!(deleted, 2);
        let left = db
            .read(|c| {
                Ok((
                    chart_parsed::count(c, VKEY_1)?,
                    chart_parsed::get(c, md5(MD5_A), VKEY_2)?,
                ))
            })
            .unwrap();
        assert_eq!(left, (0, Some(a2)));
    }

    #[test]
    fn chart_parsed_summaries_skip_the_blob_and_other_keys() {
        let dir = tempfile::tempdir().unwrap();
        let db = open_cache_db(&dir.path().join("cache.db")).unwrap();
        let rows = [
            parsed_chart(MD5_B, VKEY_1, 800),
            parsed_chart(MD5_A, VKEY_1, 1_000),
            parsed_chart(MD5_A, VKEY_2, 1_004),
        ];
        db.write(move |tx| rows.iter().try_for_each(|r| chart_parsed::put(tx, r)))
            .unwrap();
        let got = db.read(|c| chart_parsed::summaries(c, VKEY_1)).unwrap();
        let mut want = vec![
            parsed_chart(MD5_A, VKEY_1, 1_000).summary(),
            parsed_chart(MD5_B, VKEY_1, 800).summary(),
        ];
        assert_eq!(want[0].ln_ratio, 0.25);
        want.sort_by_key(|s| s.md5);
        assert_eq!(got, want, "md5 order");
    }

    fn label(scale: &str, level_ord: Option<f64>, level_text: &str) -> ChartLabel {
        ChartLabel {
            source: scale.split('_').next().unwrap().into(),
            scale: scale.into(),
            level_ord,
            level_text: level_text.into(),
            skill_tag: None,
            is_variant: false,
        }
    }

    #[test]
    fn chart_label_replace_and_queries() {
        let dir = tempfile::tempdir().unwrap();
        let db = open_cache_db(&dir.path().join("cache.db")).unwrap();
        let stale = vec![label("bms_st", Some(9.0), "9")];
        let a = vec![
            label("bms_st", Some(3.0), "3"),
            ChartLabel {
                skill_tag: Some("jack".into()),
                is_variant: true,
                ..label("jinjin_dan", Some(12.0), "12th")
            },
        ];
        let b = vec![
            label("bms_st", Some(5.0), "5"),
            label("jinjin_dan", None, "gamma_entry"),
        ];
        let c_other_key = vec![label("bms_st", Some(4.0), "4")];
        let (s, a2, b2, c2) = (stale, a.clone(), b.clone(), c_other_key);
        db.write(move |tx| {
            chart_label::replace_for(tx, md5(MD5_A), VKEY_1, &s)?;
            // Replacing drops the chart's previous labels under the same key.
            chart_label::replace_for(tx, md5(MD5_A), VKEY_1, &a2)?;
            chart_label::replace_for(tx, md5(MD5_B), VKEY_1, &b2)?;
            chart_label::replace_for(tx, md5(MD5_C), VKEY_2, &c2)
        })
        .unwrap();

        let (for_a, for_a_other_key) = db
            .read(|c| {
                Ok((
                    chart_label::list_for(c, md5(MD5_A), VKEY_1)?,
                    chart_label::list_for(c, md5(MD5_A), VKEY_2)?,
                ))
            })
            .unwrap();
        assert_eq!(for_a, a);
        assert!(for_a_other_key.is_empty());

        let all = LabelFilter::default();
        let bms = LabelFilter {
            scale: Some("bms_st".into()),
            ..LabelFilter::default()
        };
        let mid = LabelFilter {
            level_min: Some(4.0),
            level_max: Some(12.0),
            ..LabelFilter::default()
        };
        let (every, bms_rows, mid_rows, first) = db
            .read(|c| {
                Ok((
                    chart_label::list_filtered(c, VKEY_1, &all, 100)?,
                    chart_label::list_filtered(c, VKEY_1, &bms, 100)?,
                    chart_label::list_filtered(c, VKEY_1, &mid, 100)?,
                    chart_label::list_filtered(c, VKEY_1, &all, 1)?,
                ))
            })
            .unwrap();
        let (a_bms, a_dan) = ((md5(MD5_A), a[0].clone()), (md5(MD5_A), a[1].clone()));
        let (b_bms, b_dan) = ((md5(MD5_B), b[0].clone()), (md5(MD5_B), b[1].clone()));
        // By scale, then level with unlevelled rows last.
        assert_eq!(
            every,
            vec![a_bms.clone(), b_bms.clone(), a_dan.clone(), b_dan]
        );
        assert_eq!(bms_rows, vec![a_bms.clone(), b_bms.clone()]);
        assert_eq!(mid_rows, vec![b_bms, a_dan]);
        assert_eq!(first, vec![a_bms]);

        let (counts, charts, charts_other_key) = db
            .read(|c| {
                Ok((
                    chart_label::counts_by_scale(c, VKEY_1)?,
                    chart_label::count_charts(c, VKEY_1)?,
                    chart_label::count_charts(c, VKEY_2)?,
                ))
            })
            .unwrap();
        assert_eq!(
            counts,
            BTreeMap::from([
                ("bms_st".to_owned(), LabelCount { rows: 2, charts: 2 }),
                ("jinjin_dan".to_owned(), LabelCount { rows: 2, charts: 2 }),
            ])
        );
        assert_eq!((charts, charts_other_key), (2, 1));
    }

    #[test]
    fn chart_label_duplicate_rolls_back() {
        let dir = tempfile::tempdir().unwrap();
        let db = open_cache_db(&dir.path().join("cache.db")).unwrap();
        let kept = vec![label("bms_st", Some(3.0), "3")];
        let rows = kept.clone();
        db.write(move |tx| chart_label::replace_for(tx, md5(MD5_A), VKEY_1, &rows))
            .unwrap();
        let dup = vec![
            label("o2jam_hx", Some(1.0), "1"),
            label("o2jam_hx", Some(2.0), "1"),
        ];
        assert!(
            db.write(move |tx| chart_label::replace_for(tx, md5(MD5_A), VKEY_1, &dup))
                .is_err()
        );
        assert_eq!(
            db.read(|c| chart_label::list_for(c, md5(MD5_A), VKEY_1))
                .unwrap(),
            kept
        );
    }

    #[test]
    fn chart_label_prune_except() {
        let dir = tempfile::tempdir().unwrap();
        let db = open_cache_db(&dir.path().join("cache.db")).unwrap();
        db.write(|tx| {
            chart_label::replace_for(tx, md5(MD5_A), VKEY_1, &[label("bms_st", Some(1.0), "1")])?;
            chart_label::replace_for(tx, md5(MD5_A), VKEY_2, &[label("bms_st", Some(1.0), "1")])
        })
        .unwrap();
        assert_eq!(
            db.write(|tx| chart_label::prune_except(tx, &[VKEY_2]))
                .unwrap(),
            1
        );
        assert_eq!(
            db.read(|c| chart_label::count_charts(c, VKEY_2)).unwrap(),
            1
        );
        assert_eq!(
            db.read(|c| chart_label::count_charts(c, VKEY_1)).unwrap(),
            0
        );
    }

    #[test]
    fn derivation_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let db = open_cache_db(&dir.path().join("cache.db")).unwrap();
        let skipped = Derivation {
            stage: StageId::from_static("chart_archive"),
            input_key: format!("{MD5_A}:{}", "ab".repeat(32)),
            vkey: VersionKey([7; 32]),
            status: DerivationStatus::Skipped,
            error_code: Some(ErrorCode::Conflict),
            error_msg: Some("md5 mismatch".into()),
            duration_ms: Some(12),
        };
        let ok = Derivation {
            stage: StageId::from_static("catalog"),
            input_key: "cd".repeat(32),
            vkey: VersionKey([9; 32]),
            status: DerivationStatus::Ok,
            error_code: None,
            error_msg: None,
            duration_ms: None,
        };
        let (a, b) = (skipped.clone(), ok.clone());
        db.write(move |tx| {
            derivation::put(tx, &a)?;
            derivation::put(tx, &b)?;
            // Re-putting the same key replaces the row instead of failing.
            derivation::put(tx, &b)
        })
        .unwrap();
        let (got, other_vkey, all) = db
            .read(|c| {
                Ok((
                    derivation::get(c, &skipped.stage, &skipped.input_key, skipped.vkey)?,
                    derivation::get(c, &skipped.stage, &skipped.input_key, VersionKey([8; 32]))?,
                    derivation::list_all(c)?,
                ))
            })
            .unwrap();
        assert_eq!(got, Some(skipped.clone()));
        assert_eq!(other_vkey, None);
        assert_eq!(all, vec![ok, skipped]);
    }

    #[test]
    fn job_run_finish_sets_summary() {
        let dir = tempfile::tempdir().unwrap();
        let db = open_cache_db(&dir.path().join("cache.db")).unwrap();
        let id = ulid::Ulid::from_parts(1, 1);
        let job = NewJobRun {
            id,
            kind: "sync_plays".into(),
            params: json!({"installId": 1}),
            status: JobStatus::Queued,
            started: None,
        };
        db.write(move |tx| job_run::insert(tx, &job)).unwrap();
        assert!(db.write(move |tx| job_run::start(tx, id, T0)).unwrap());
        let summary = json!({"playsNew": 6, "failedItems": 0});
        let s = summary.clone();
        let ended = UnixUs(T0.0 + 2_000_000);
        assert!(
            db.write(move |tx| job_run::finish(tx, id, JobStatus::Ok, ended, &s))
                .unwrap()
        );
        let run = db.read(|c| job_run::get(c, id)).unwrap().unwrap();
        assert_eq!(
            run,
            JobRun {
                id,
                kind: "sync_plays".into(),
                params: json!({"installId": 1}),
                status: JobStatus::Ok,
                started: Some(T0),
                ended: Some(ended),
                summary: Some(summary),
            }
        );
        // Finishing into a non-terminal status is a caller bug.
        assert!(
            db.write(move |tx| job_run::finish(tx, id, JobStatus::Running, ended, &json!({})))
                .is_err()
        );
        let recent = db.read(|c| job_run::list_recent(c, 10)).unwrap();
        assert_eq!(recent.len(), 1);
    }

    #[test]
    fn item_failure_first_wins() {
        let dir = tempfile::tempdir().unwrap();
        let db = open_cache_db(&dir.path().join("cache.db")).unwrap();
        let id = ulid::Ulid::from_parts(2, 2);
        let job = NewJobRun {
            id,
            kind: "sync_plays".into(),
            params: json!({}),
            status: JobStatus::Running,
            started: Some(T0),
        };
        let first = ItemFailure {
            job_id: id,
            item_ref: "b".repeat(64),
            code: ErrorCode::Conflict,
            message: "stored row wins".into(),
        };
        let mut again = first.clone();
        again.code = ErrorCode::Internal;
        let other = ItemFailure {
            item_ref: "a".repeat(64),
            ..first.clone()
        };
        let (f, a, o) = (first.clone(), again, other.clone());
        let inserted = db
            .write(move |tx| {
                job_run::insert(tx, &job)?;
                Ok((
                    item_failure::insert(tx, &f)?,
                    item_failure::insert(tx, &a)?,
                    item_failure::insert(tx, &o)?,
                ))
            })
            .unwrap();
        assert_eq!(inserted, (true, false, true));
        assert_eq!(
            db.read(|c| item_failure::list(c, id)).unwrap(),
            vec![other, first]
        );
    }
}
