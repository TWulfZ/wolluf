//! user.db ledger repositories (spec 003 Design `repo/ledger.rs`).

use std::path::{Path, PathBuf};

use rusqlite::{OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wolluf_core::{AliasId, BlobSha256, ChartMd5, FileTime, Game, PlayId, ProfileId, UnixUs};

use crate::db::{Conn, Tx};
use crate::error::StoreError;
use crate::repo::sql::{
    enum_col, fixed, int, json_value, opt_fixed, opt_parsed, parsed, str_enum, time, to_i64,
};
use crate::time::{format_rfc3339_ms, parse_rfc3339_ms};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstallId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SnapshotId(pub i64);

str_enum! {
    pub enum SnapshotKind {
        OsuDb => "osu_db",
        ScoresDb => "scores_db",
        CollectionDb => "collection_db",
        Cfg => "cfg",
        SongsScan => "songs_scan",
    }
}

str_enum! {
    pub enum BlobKind {
        Osr => "osr",
        Osg => "osg",
        Osu => "osu",
    }
}

str_enum! {
    /// Where a play row came from (ADR 0014).
    pub enum PlayOrigin {
        ScoresDb => "scores_db",
        ReplayOnly => "replay_only",
    }
}

str_enum! {
    pub enum ScoreSystem {
        V1 => "v1",
        V2 => "v2",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameInstall {
    pub id: InstallId,
    pub game: Game,
    pub root_path: PathBuf,
    pub client_version: Option<i32>,
    pub detected_at: UnixUs,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSnapshot {
    pub install_id: InstallId,
    pub kind: SnapshotKind,
    /// sha256 of the source file bytes as read (not a vault blob).
    pub sha256: BlobSha256,
    pub size: u64,
    pub mtime: UnixUs,
    pub format_version: Option<i32>,
    pub imported_at: UnixUs,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSnapshot {
    pub id: SnapshotId,
    pub install_id: InstallId,
    pub kind: SnapshotKind,
    pub sha256: BlobSha256,
    pub size: u64,
    /// Millisecond precision, like every persisted timestamp.
    pub mtime: UnixUs,
    pub format_version: Option<i32>,
    pub imported_at: UnixUs,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewBlob {
    pub sha256: BlobSha256,
    pub kind: BlobKind,
    pub size: u64,
    pub origin_path: String,
    pub first_seen: UnixUs,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alias {
    pub id: AliasId,
    pub game: Game,
    pub raw_name: Vec<u8>,
}

/// Judgement counts in `counts_json` key order; MAX is osu!'s geki, 200 its katu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PlayCounts {
    pub max: u16,
    pub n300: u16,
    pub n200: u16,
    pub n100: u16,
    pub n50: u16,
    pub miss: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewPlay {
    pub id: PlayId,
    pub alias_id: AliasId,
    pub chart_md5: ChartMd5,
    pub origin: PlayOrigin,
    pub filetime: FileTime,
    pub played_at: UnixUs,
    pub mods: i32,
    pub score_system: ScoreSystem,
    pub counts: PlayCounts,
    pub max_combo: u16,
    pub score: i32,
    pub native_acc: Option<f64>,
    /// Only positive osu! ids; ids ≤ 0 mean "no online score" and are passed as `None`.
    pub online_score_id: Option<u64>,
    pub client_version: i32,
    pub replay_sha: Option<BlobSha256>,
    pub osg_sha: Option<BlobSha256>,
    pub snapshot_id: Option<SnapshotId>,
    pub ingested_at: UnixUs,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Play {
    pub id: PlayId,
    pub alias_id: AliasId,
    pub chart_md5: ChartMd5,
    pub origin: PlayOrigin,
    pub filetime: FileTime,
    pub played_at: UnixUs,
    pub mods: i32,
    pub score_system: ScoreSystem,
    pub counts: PlayCounts,
    pub max_combo: u16,
    pub score: i32,
    pub native_acc: Option<f64>,
    pub passed: Option<bool>,
    pub online_score_id: Option<u64>,
    pub client_version: i32,
    pub replay_sha: Option<BlobSha256>,
    pub osg_sha: Option<BlobSha256>,
    pub chart_sha: Option<BlobSha256>,
    pub snapshot_id: Option<SnapshotId>,
    pub ingested_at: UnixUs,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InsertOutcome {
    pub new: u32,
    pub existing: u32,
    /// `replay_only` rows promoted to `scores_db` by an identical score record.
    pub upgraded: u32,
    /// Same natural key, different ledger fields: the stored row won.
    pub conflicts: Vec<PlayId>,
}

/// A play still missing its replay or `.osg` link; the archive step decides which files exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnlinkedPlay {
    pub id: PlayId,
    pub chart_md5: ChartMd5,
    pub filetime: FileTime,
    pub needs_replay: bool,
    pub needs_osg: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewFeedbackEvent {
    pub id: ulid::Ulid,
    pub ts: UnixUs,
    pub profile_id: Option<ProfileId>,
    pub kind: String,
    pub subject: serde_json::Value,
    pub payload: serde_json::Value,
    pub context: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FeedbackEvent {
    pub id: ulid::Ulid,
    pub ts: UnixUs,
    pub profile_id: Option<ProfileId>,
    pub kind: String,
    pub subject: serde_json::Value,
    pub payload: serde_json::Value,
    pub context: serde_json::Value,
    pub telemetry_state: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallRow {
    pub id: uuid::Uuid,
    pub secret: [u8; 32],
    pub created_at: UnixUs,
}

fn path_text(path: &Path) -> Result<&str, StoreError> {
    path.to_str()
        .ok_or_else(|| StoreError::InvalidData(format!("non-UTF-8 path {path:?}")))
}

fn ms(t: UnixUs) -> String {
    format_rfc3339_ms(t)
}

pub mod game_install {
    use super::*;

    /// Keeps the first `detected_at`; a newly known client version replaces the stored one.
    pub fn upsert(
        tx: &Tx<'_>,
        game: Game,
        root_path: &Path,
        client_version: Option<i32>,
        now: UnixUs,
    ) -> Result<InstallId, StoreError> {
        let id = tx.0.query_row(
            "INSERT INTO game_install (game, root_path, client_version, detected_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (game, root_path)
             DO UPDATE SET client_version = coalesce(excluded.client_version, client_version)
             RETURNING id",
            (
                game.as_str(),
                path_text(root_path)?,
                client_version,
                ms(now),
            ),
            |r| r.get(0),
        )?;
        Ok(InstallId(id))
    }

    const COLUMNS: &str = "id, game, root_path, client_version, detected_at";

    fn from_row(row: &Row<'_>) -> rusqlite::Result<GameInstall> {
        Ok(GameInstall {
            id: InstallId(row.get(0)?),
            game: parsed(row, 1, str::parse::<Game>)?,
            root_path: PathBuf::from(row.get::<_, String>(2)?),
            client_version: row.get(3)?,
            detected_at: time(row, 4)?,
        })
    }

    pub fn get(conn: Conn<'_>, id: InstallId) -> Result<Option<GameInstall>, StoreError> {
        Ok(conn
            .0
            .query_row(
                &format!("SELECT {COLUMNS} FROM game_install WHERE id = ?1"),
                [id.0],
                from_row,
            )
            .optional()?)
    }

    pub fn list(conn: Conn<'_>) -> Result<Vec<GameInstall>, StoreError> {
        let mut stmt = conn
            .0
            .prepare(&format!("SELECT {COLUMNS} FROM game_install ORDER BY id"))?;
        let rows = stmt.query_map([], from_row)?.collect::<Result<_, _>>()?;
        Ok(rows)
    }
}

pub mod source_snapshot {
    use super::*;

    pub fn insert(tx: &Tx<'_>, s: &NewSnapshot) -> Result<SnapshotId, StoreError> {
        tx.0.execute(
            "INSERT INTO source_snapshot
                 (install_id, kind, sha256, size, mtime, format_version, imported_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            (
                s.install_id.0,
                s.kind.as_str(),
                s.sha256.0,
                to_i64(s.size, "snapshot size")?,
                ms(s.mtime),
                s.format_version,
                ms(s.imported_at),
            ),
        )?;
        Ok(SnapshotId(tx.0.last_insert_rowid()))
    }

    pub fn latest(
        conn: Conn<'_>,
        install_id: InstallId,
        kind: SnapshotKind,
    ) -> Result<Option<SourceSnapshot>, StoreError> {
        Ok(conn
            .0
            .query_row(
                "SELECT id, install_id, kind, sha256, size, mtime, format_version, imported_at
                 FROM source_snapshot WHERE install_id = ?1 AND kind = ?2
                 ORDER BY id DESC LIMIT 1",
                (install_id.0, kind.as_str()),
                |row| {
                    Ok(SourceSnapshot {
                        id: SnapshotId(row.get(0)?),
                        install_id: InstallId(row.get(1)?),
                        kind: enum_col(row, 2, SnapshotKind::parse)?,
                        sha256: BlobSha256(fixed(row, 3)?),
                        size: int(row, 4)?,
                        mtime: time(row, 5)?,
                        format_version: row.get(6)?,
                        imported_at: time(row, 7)?,
                    })
                },
            )
            .optional()?)
    }
}

impl SourceSnapshot {
    /// Compares at the stored millisecond precision; a finer fresh stat is floored first.
    pub fn matches_stat(&self, size: u64, mtime: UnixUs) -> bool {
        const US_PER_MS: i64 = 1_000;
        let floored = UnixUs(mtime.0.div_euclid(US_PER_MS) * US_PER_MS);
        self.size == size && self.mtime == floored
    }
}

pub mod blob {
    use super::*;

    /// Returns whether the row was new. Call only after `Vault::put` returned, so a row never
    /// points at bytes that are not durable yet.
    pub fn insert(tx: &Tx<'_>, b: &NewBlob) -> Result<bool, StoreError> {
        let n = tx.0.execute(
            "INSERT INTO blob (sha256, kind, size, origin_path, first_seen)
             VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT (sha256) DO NOTHING",
            (
                b.sha256.0,
                b.kind.as_str(),
                to_i64(b.size, "blob size")?,
                &b.origin_path,
                ms(b.first_seen),
            ),
        )?;
        Ok(n == 1)
    }

    pub fn exists(conn: Conn<'_>, sha: BlobSha256) -> Result<bool, StoreError> {
        Ok(conn.0.query_row(
            "SELECT EXISTS (SELECT 1 FROM blob WHERE sha256 = ?1)",
            [sha.0],
            |r| r.get(0),
        )?)
    }

    pub fn count(conn: Conn<'_>) -> Result<u64, StoreError> {
        Ok(conn
            .0
            .query_row("SELECT count(*) FROM blob", [], |r| int(r, 0))?)
    }
}

pub mod alias {
    use super::*;

    pub fn upsert(tx: &Tx<'_>, game: Game, raw_name: &[u8]) -> Result<AliasId, StoreError> {
        tx.0.prepare_cached(
            "INSERT INTO alias (game, raw_name) VALUES (?1, ?2)
             ON CONFLICT (game, raw_name) DO NOTHING",
        )?
        .execute((game.as_str(), raw_name))?;
        let id =
            tx.0.prepare_cached("SELECT id FROM alias WHERE game = ?1 AND raw_name = ?2")?
                .query_row((game.as_str(), raw_name), |r| r.get(0))?;
        Ok(AliasId(id))
    }

    pub fn list(conn: Conn<'_>) -> Result<Vec<Alias>, StoreError> {
        let mut stmt = conn
            .0
            .prepare("SELECT id, game, raw_name FROM alias ORDER BY id")?;
        let rows = stmt
            .query_map([], |row| {
                Ok(Alias {
                    id: AliasId(row.get(0)?),
                    game: parsed(row, 1, str::parse::<Game>)?,
                    raw_name: row.get(2)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    pub fn count(conn: Conn<'_>) -> Result<u64, StoreError> {
        Ok(conn
            .0
            .query_row("SELECT count(*) FROM alias", [], |r| int(r, 0))?)
    }
}

pub mod play {
    use std::collections::BTreeSet;

    use super::*;

    const COLUMNS: &str = "id, alias_id, chart_md5, origin, filetime, played_at_utc, mods,
        score_system, counts_json, max_combo, score, native_acc, passed, online_score_id,
        client_version, replay_sha, osg_sha, chart_sha, snapshot_id, ingested_at";

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Play> {
        Ok(Play {
            id: PlayId(fixed(row, 0)?),
            alias_id: AliasId(row.get(1)?),
            chart_md5: parsed(row, 2, str::parse::<ChartMd5>)?,
            origin: enum_col(row, 3, PlayOrigin::parse)?,
            filetime: parsed(row, 4, FileTime::parse_decimal)?,
            played_at: time(row, 5)?,
            mods: row.get(6)?,
            score_system: enum_col(row, 7, ScoreSystem::parse)?,
            counts: json_value(row, 8)?,
            max_combo: int(row, 9)?,
            score: row.get(10)?,
            native_acc: row.get(11)?,
            passed: row.get(12)?,
            online_score_id: opt_parsed(row, 13, str::parse::<u64>)?,
            client_version: row.get(14)?,
            replay_sha: opt_fixed(row, 15)?.map(BlobSha256),
            osg_sha: opt_fixed(row, 16)?.map(BlobSha256),
            chart_sha: opt_fixed(row, 17)?.map(BlobSha256),
            snapshot_id: row.get::<_, Option<i64>>(18)?.map(SnapshotId),
            ingested_at: time(row, 19)?,
        })
    }

    /// The fields that decide "same record" for an existing natural key (spec 003 step 2).
    #[derive(PartialEq)]
    struct LedgerFields {
        mods: i32,
        score: i32,
        max_combo: u16,
        counts: PlayCounts,
        client_version: i32,
    }

    impl From<&NewPlay> for LedgerFields {
        fn from(p: &NewPlay) -> Self {
            Self {
                mods: p.mods,
                score: p.score,
                max_combo: p.max_combo,
                counts: p.counts,
                client_version: p.client_version,
            }
        }
    }

    /// Idempotent through the natural key. `ON CONFLICT (id) DO NOTHING` rather than
    /// `INSERT OR IGNORE`, which would also swallow CHECK violations and report them as
    /// "existing".
    pub fn insert_batch(tx: &Tx<'_>, plays: &[NewPlay]) -> Result<InsertOutcome, StoreError> {
        let mut insert = tx.0.prepare_cached(&format!(
            "INSERT INTO play ({COLUMNS})
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, NULL, ?13, ?14, ?15, ?16,
                     NULL, ?17, ?18)
             ON CONFLICT (id) DO NOTHING"
        ))?;
        let mut existing = tx.0.prepare_cached(
            "SELECT origin, mods, score, max_combo, counts_json, client_version
             FROM play WHERE id = ?1",
        )?;
        let mut upgrade = tx.0.prepare_cached(
            "UPDATE play SET origin = 'scores_db', snapshot_id = ?2
             WHERE id = ?1 AND origin = 'replay_only'",
        )?;

        let mut outcome = InsertOutcome::default();
        for p in plays {
            let counts_json = serde_json::to_string(&p.counts)
                .map_err(|e| StoreError::InvalidData(format!("counts_json: {e}")))?;
            let inserted = insert.execute(rusqlite::params![
                p.id.0,
                p.alias_id.0,
                p.chart_md5.to_string(),
                p.origin.as_str(),
                p.filetime.to_string(),
                ms(p.played_at),
                p.mods,
                p.score_system.as_str(),
                counts_json,
                p.max_combo,
                p.score,
                p.native_acc,
                p.online_score_id.map(|id| id.to_string()),
                p.client_version,
                p.replay_sha.map(|s| s.0),
                p.osg_sha.map(|s| s.0),
                p.snapshot_id.map(|s| s.0),
                ms(p.ingested_at),
            ])?;
            if inserted == 1 {
                outcome.new += 1;
                continue;
            }

            let (stored_origin, stored) = existing.query_row([p.id.0], |row| {
                Ok((
                    enum_col(row, 0, PlayOrigin::parse)?,
                    LedgerFields {
                        mods: row.get(1)?,
                        score: row.get(2)?,
                        max_combo: int(row, 3)?,
                        counts: json_value(row, 4)?,
                        client_version: row.get(5)?,
                    },
                ))
            })?;
            if stored != LedgerFields::from(p) {
                outcome.conflicts.push(p.id);
            } else if stored_origin == PlayOrigin::ReplayOnly && p.origin == PlayOrigin::ScoresDb {
                upgrade.execute((p.id.0, p.snapshot_id.map(|s| s.0)))?;
                outcome.upgraded += 1;
            } else {
                outcome.existing += 1;
            }
        }
        Ok(outcome)
    }

    pub fn get(conn: Conn<'_>, id: PlayId) -> Result<Option<Play>, StoreError> {
        Ok(conn
            .0
            .query_row(
                &format!("SELECT {COLUMNS} FROM play WHERE id = ?1"),
                [id.0],
                from_row,
            )
            .optional()?)
    }

    pub fn count(conn: Conn<'_>) -> Result<u64, StoreError> {
        Ok(conn
            .0
            .query_row("SELECT count(*) FROM play", [], |r| int(r, 0))?)
    }

    pub fn unlinked_replays(conn: Conn<'_>) -> Result<Vec<UnlinkedPlay>, StoreError> {
        let mut stmt = conn.0.prepare(
            "SELECT id, chart_md5, filetime, replay_sha IS NULL, osg_sha IS NULL FROM play
             WHERE replay_sha IS NULL OR osg_sha IS NULL ORDER BY chart_md5, filetime, id",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(UnlinkedPlay {
                    id: PlayId(fixed(row, 0)?),
                    chart_md5: parsed(row, 1, str::parse::<ChartMd5>)?,
                    filetime: parsed(row, 2, FileTime::parse_decimal)?,
                    needs_replay: row.get(3)?,
                    needs_osg: row.get(4)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    /// The `Data/r` name key of every play; replays outside this set are orphans.
    pub fn keys_by_chart_and_time(
        conn: Conn<'_>,
    ) -> Result<BTreeSet<(ChartMd5, FileTime)>, StoreError> {
        let mut stmt = conn.0.prepare("SELECT chart_md5, filetime FROM play")?;
        let keys = stmt
            .query_map([], |row| {
                Ok((
                    parsed(row, 0, str::parse::<ChartMd5>)?,
                    parsed(row, 1, FileTime::parse_decimal)?,
                ))
            })?
            .collect::<Result<_, _>>()?;
        Ok(keys)
    }

    /// Charts with a play under any of `aliases`.
    pub fn charts_played_by(
        conn: Conn<'_>,
        aliases: &[AliasId],
    ) -> Result<BTreeSet<ChartMd5>, StoreError> {
        let placeholders = vec!["?"; aliases.len()].join(", ");
        let mut stmt = conn.0.prepare(&format!(
            "SELECT DISTINCT chart_md5 FROM play WHERE alias_id IN ({placeholders})"
        ))?;
        let rows = stmt
            .query_map(
                rusqlite::params_from_iter(aliases.iter().map(|a| a.0)),
                |row| parsed(row, 0, str::parse::<ChartMd5>),
            )?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    pub fn charts_without_blob(conn: Conn<'_>) -> Result<Vec<ChartMd5>, StoreError> {
        let mut stmt = conn.0.prepare(
            "SELECT DISTINCT chart_md5 FROM play WHERE chart_sha IS NULL ORDER BY chart_md5",
        )?;
        let rows = stmt
            .query_map([], |row| parsed(row, 0, str::parse::<ChartMd5>))?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    /// NULL-guarded: returns false when the play already had a replay, which then stays.
    pub fn set_replay_sha(tx: &Tx<'_>, id: PlayId, sha: BlobSha256) -> Result<bool, StoreError> {
        let n = tx.0.execute(
            "UPDATE play SET replay_sha = ?2 WHERE id = ?1 AND replay_sha IS NULL",
            (id.0, sha.0),
        )?;
        Ok(n == 1)
    }

    pub fn set_osg_sha(tx: &Tx<'_>, id: PlayId, sha: BlobSha256) -> Result<bool, StoreError> {
        let n = tx.0.execute(
            "UPDATE play SET osg_sha = ?2 WHERE id = ?1 AND osg_sha IS NULL",
            (id.0, sha.0),
        )?;
        Ok(n == 1)
    }

    /// Links every play of the chart that has no chart blob yet; returns how many changed.
    pub fn set_chart_sha(
        tx: &Tx<'_>,
        chart_md5: ChartMd5,
        sha: BlobSha256,
    ) -> Result<u64, StoreError> {
        let n = tx.0.execute(
            "UPDATE play SET chart_sha = ?2 WHERE chart_md5 = ?1 AND chart_sha IS NULL",
            (chart_md5.to_string(), sha.0),
        )?;
        Ok(n as u64)
    }
}

fn json_text(value: &serde_json::Value) -> String {
    value.to_string()
}

fn json_col(row: &Row<'_>, idx: usize) -> rusqlite::Result<serde_json::Value> {
    json_value(row, idx)
}

pub mod feedback_event {
    use super::*;

    pub fn append(tx: &Tx<'_>, e: &NewFeedbackEvent) -> Result<(), StoreError> {
        tx.0.execute(
            "INSERT INTO feedback_event
                 (id, ts, profile_id, kind, subject_json, payload_json, context_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            (
                e.id.to_string(),
                ms(e.ts),
                e.profile_id.map(|p| p.0),
                &e.kind,
                json_text(&e.subject),
                json_text(&e.payload),
                json_text(&e.context),
            ),
        )?;
        Ok(())
    }

    /// The greatest id so far; writers use it to keep ids strictly increasing when several
    /// events share one millisecond.
    pub fn last_id(conn: Conn<'_>) -> Result<Option<ulid::Ulid>, StoreError> {
        let max: Option<String> =
            conn.0
                .query_row("SELECT max(id) FROM feedback_event", [], |r| r.get(0))?;
        max.map(|s| {
            ulid::Ulid::from_string(&s)
                .map_err(|e| StoreError::InvalidData(format!("feedback_event id {s}: {e}")))
        })
        .transpose()
    }

    /// ULIDs sort by creation time, so id order is append order.
    pub fn list(conn: Conn<'_>) -> Result<Vec<FeedbackEvent>, StoreError> {
        let mut stmt = conn.0.prepare(
            "SELECT id, ts, profile_id, kind, subject_json, payload_json, context_json,
                    telemetry_state
             FROM feedback_event ORDER BY id",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(FeedbackEvent {
                    id: parsed(row, 0, ulid::Ulid::from_string)?,
                    ts: time(row, 1)?,
                    profile_id: row.get::<_, Option<i64>>(2)?.map(ProfileId),
                    kind: row.get(3)?,
                    subject: json_col(row, 4)?,
                    payload: json_col(row, 5)?,
                    context: json_col(row, 6)?,
                    telemetry_state: row.get(7)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }
}

pub mod settings {
    use super::*;

    pub fn get(conn: Conn<'_>, key: &str) -> Result<Option<serde_json::Value>, StoreError> {
        Ok(conn
            .0
            .query_row("SELECT json FROM settings WHERE key = ?1", [key], |r| {
                json_col(r, 0)
            })
            .optional()?)
    }

    pub fn set(tx: &Tx<'_>, key: &str, value: &serde_json::Value) -> Result<(), StoreError> {
        tx.0.execute(
            "INSERT INTO settings (key, json) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET json = excluded.json",
            (key, json_text(value)),
        )?;
        Ok(())
    }
}

pub mod meta {
    use super::*;

    pub fn get(conn: Conn<'_>, key: &str) -> Result<Option<String>, StoreError> {
        Ok(conn
            .0
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn set(tx: &Tx<'_>, key: &str, value: &str) -> Result<(), StoreError> {
        tx.0.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            (key, value),
        )?;
        Ok(())
    }
}

pub mod install {
    use super::*;

    const UUIDS_PER_SECRET: usize = 3;

    /// 32 uniform bytes without a direct CSPRNG dependency: three v4 UUIDs carry 366 bits of OS
    /// randomness (getrandom, through uuid), and sha256 compresses them to 256.
    fn new_secret() -> [u8; 32] {
        let mut hasher = Sha256::new();
        for _ in 0..UUIDS_PER_SECRET {
            hasher.update(uuid::Uuid::new_v4().as_bytes());
        }
        hasher.finalize().into()
    }

    /// Creates the single install row on first call; later calls return it unchanged.
    pub fn ensure(tx: &Tx<'_>, now: UnixUs) -> Result<InstallRow, StoreError> {
        if let Some(row) = get(tx.conn())? {
            return Ok(row);
        }
        let row = InstallRow {
            id: uuid::Uuid::new_v4(),
            secret: new_secret(),
            created_at: parse_rfc3339_ms(&ms(now))?,
        };
        tx.0.execute(
            "INSERT INTO install (id, secret, created_at) VALUES (?1, ?2, ?3)",
            (row.id.to_string(), row.secret, ms(row.created_at)),
        )?;
        Ok(row)
    }

    pub fn get(conn: Conn<'_>) -> Result<Option<InstallRow>, StoreError> {
        Ok(conn
            .0
            .query_row("SELECT id, secret, created_at FROM install", [], |row| {
                Ok(InstallRow {
                    id: parsed(row, 0, uuid::Uuid::parse_str)?,
                    secret: fixed(row, 1)?,
                    created_at: time(row, 2)?,
                })
            })
            .optional()?)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::Path;
    use std::str::FromStr;

    use rusqlite::Connection;

    use super::*;
    use crate::user::testkit::migrated;

    pub(crate) const T0: UnixUs = UnixUs(1_790_637_236_636_000);
    pub(crate) const MD5_A: &str = "e956977ccc1d74a50ae48b43a868cc20";
    pub(crate) const MD5_B: &str = "0123456789abcdef0123456789abcdef";

    pub(crate) fn md5(s: &str) -> ChartMd5 {
        ChartMd5::from_str(s).unwrap()
    }

    pub(crate) fn sha(byte: u8) -> BlobSha256 {
        BlobSha256([byte; 32])
    }

    pub(crate) fn tx(conn: &mut Connection) -> Tx<'_> {
        Tx(conn.transaction().unwrap())
    }

    pub(crate) fn scores_snapshot(tx: &Tx<'_>) -> SnapshotId {
        let install =
            game_install::upsert(tx, Game::OsuStable, Path::new("/osu"), Some(20260924), T0)
                .unwrap();
        source_snapshot::insert(
            tx,
            &NewSnapshot {
                install_id: install,
                kind: SnapshotKind::ScoresDb,
                sha256: sha(0x5a),
                size: 650_000,
                mtime: T0,
                format_version: Some(20260924),
                imported_at: T0,
            },
        )
        .unwrap()
    }

    pub(crate) fn osr_blob(tx: &Tx<'_>, byte: u8) -> BlobSha256 {
        blob::insert(
            tx,
            &NewBlob {
                sha256: sha(byte),
                kind: BlobKind::Osr,
                size: 100,
                origin_path: format!("Data/r/{byte}.osr"),
                first_seen: T0,
            },
        )
        .unwrap();
        sha(byte)
    }

    pub(crate) fn new_play(
        alias_id: AliasId,
        raw_name: &[u8],
        md5_text: &str,
        filetime: i64,
        snapshot: Option<SnapshotId>,
    ) -> NewPlay {
        let filetime = FileTime::new(filetime).unwrap();
        NewPlay {
            id: PlayId::derive(Game::OsuStable, md5(md5_text), raw_name, filetime),
            alias_id,
            chart_md5: md5(md5_text),
            origin: PlayOrigin::ScoresDb,
            filetime,
            played_at: T0,
            mods: 1 << 29,
            score_system: ScoreSystem::V2,
            counts: PlayCounts {
                max: 900,
                n300: 50,
                n200: 10,
                n100: 3,
                n50: 1,
                miss: 2,
            },
            max_combo: 812,
            score: 954_321,
            native_acc: Some(0.975),
            online_score_id: Some(4_567_890_123),
            client_version: 20260924,
            replay_sha: None,
            osg_sha: None,
            snapshot_id: snapshot,
            ingested_at: T0,
        }
    }

    #[test]
    fn alias_upsert_empty_and_non_utf8_names() {
        let mut conn = migrated();
        let tx = tx(&mut conn);
        let empty = alias::upsert(&tx, Game::OsuStable, b"").unwrap();
        let bytes = alias::upsert(&tx, Game::OsuStable, &[0xff, 0xfe, b'A']).unwrap();
        let named = alias::upsert(&tx, Game::OsuStable, b"Alice").unwrap();
        assert_eq!(alias::upsert(&tx, Game::OsuStable, b"").unwrap(), empty);
        assert_eq!(
            alias::upsert(&tx, Game::OsuStable, &[0xff, 0xfe, b'A']).unwrap(),
            bytes
        );
        // Case matters: raw bytes are the key, normalization is for matching only (§5.3).
        assert_ne!(
            alias::upsert(&tx, Game::OsuStable, b"alice").unwrap(),
            named
        );
        assert_eq!(alias::count(tx.conn()).unwrap(), 4);
        let names: Vec<Vec<u8>> = alias::list(tx.conn())
            .unwrap()
            .into_iter()
            .map(|a| a.raw_name)
            .collect();
        assert!(names.contains(&Vec::new()));
        assert!(names.contains(&vec![0xff, 0xfe, b'A']));
    }

    #[test]
    fn insert_batch_idempotent() {
        let mut conn = migrated();
        let tx = tx(&mut conn);
        let snap = scores_snapshot(&tx);
        let alias_id = alias::upsert(&tx, Game::OsuStable, b"Alice").unwrap();
        let plays = vec![
            new_play(
                alias_id,
                b"Alice",
                MD5_A,
                134_350_010_443_098_880,
                Some(snap),
            ),
            new_play(
                alias_id,
                b"Alice",
                MD5_B,
                134_350_010_443_098_881,
                Some(snap),
            ),
            // An exact duplicate record inside one scores.db collapses to one play.
            new_play(
                alias_id,
                b"Alice",
                MD5_A,
                134_350_010_443_098_880,
                Some(snap),
            ),
        ];
        let first = play::insert_batch(&tx, &plays).unwrap();
        assert_eq!(
            first,
            InsertOutcome {
                new: 2,
                existing: 1,
                upgraded: 0,
                conflicts: vec![]
            }
        );
        let second = play::insert_batch(&tx, &plays).unwrap();
        assert_eq!((second.new, second.existing), (0, 3));
        assert_eq!(play::count(tx.conn()).unwrap(), 2);
        let stored = play::get(tx.conn(), plays[0].id).unwrap().unwrap();
        assert_eq!(stored, expected_row(&plays[0]));
    }

    pub(crate) fn expected_row(p: &NewPlay) -> Play {
        Play {
            id: p.id,
            alias_id: p.alias_id,
            chart_md5: p.chart_md5,
            origin: p.origin,
            filetime: p.filetime,
            played_at: p.played_at,
            mods: p.mods,
            score_system: p.score_system,
            counts: p.counts,
            max_combo: p.max_combo,
            score: p.score,
            native_acc: p.native_acc,
            passed: None,
            online_score_id: p.online_score_id,
            client_version: p.client_version,
            replay_sha: p.replay_sha,
            osg_sha: p.osg_sha,
            chart_sha: None,
            snapshot_id: p.snapshot_id,
            ingested_at: p.ingested_at,
        }
    }

    #[test]
    fn counts_json_has_spec_shape() {
        let mut conn = migrated();
        let tx = tx(&mut conn);
        let snap = scores_snapshot(&tx);
        let alias_id = alias::upsert(&tx, Game::OsuStable, b"Alice").unwrap();
        let p = new_play(alias_id, b"Alice", MD5_A, 1, Some(snap));
        play::insert_batch(&tx, std::slice::from_ref(&p)).unwrap();
        let (json, online, filetime): (String, String, String) =
            tx.0.query_row(
                "SELECT counts_json, online_score_id, filetime FROM play",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            json,
            r#"{"max":900,"n300":50,"n200":10,"n100":3,"n50":1,"miss":2}"#
        );
        assert_eq!(online, "4567890123");
        assert_eq!(filetime, "1");
    }

    #[test]
    fn insert_batch_reports_conflict() {
        let mut conn = migrated();
        let tx = tx(&mut conn);
        let snap = scores_snapshot(&tx);
        let alias_id = alias::upsert(&tx, Game::OsuStable, b"Alice").unwrap();
        let original = new_play(alias_id, b"Alice", MD5_A, 7, Some(snap));
        let mut changed = original.clone();
        changed.score += 1;
        let outcome = play::insert_batch(&tx, &[original.clone(), changed]).unwrap();
        assert_eq!(outcome.new, 1);
        assert_eq!(outcome.conflicts, vec![original.id]);
        assert_eq!(
            play::get(tx.conn(), original.id).unwrap().unwrap().score,
            original.score
        );
    }

    #[test]
    fn insert_batch_upgrades_replay_only() {
        let mut conn = migrated();
        let tx = tx(&mut conn);
        let snap = scores_snapshot(&tx);
        let alias_id = alias::upsert(&tx, Game::OsuStable, b"Alice").unwrap();
        let replay = osr_blob(&tx, 0x01);
        let mut orphan = new_play(alias_id, b"Alice", MD5_A, 9, None);
        orphan.origin = PlayOrigin::ReplayOnly;
        orphan.replay_sha = Some(replay);
        assert_eq!(play::insert_batch(&tx, &[orphan.clone()]).unwrap().new, 1);

        let scored = new_play(alias_id, b"Alice", MD5_A, 9, Some(snap));
        let outcome = play::insert_batch(&tx, std::slice::from_ref(&scored)).unwrap();
        assert_eq!(
            outcome,
            InsertOutcome {
                new: 0,
                existing: 0,
                upgraded: 1,
                conflicts: vec![]
            }
        );
        let stored = play::get(tx.conn(), orphan.id).unwrap().unwrap();
        assert_eq!(stored.origin, PlayOrigin::ScoresDb);
        assert_eq!(stored.snapshot_id, Some(snap));
        assert_eq!(stored.replay_sha, Some(replay));

        // A second identical record is plain "existing"; a replay_only re-import never downgrades.
        assert_eq!(play::insert_batch(&tx, &[scored]).unwrap().existing, 1);
        assert_eq!(play::insert_batch(&tx, &[orphan]).unwrap().existing, 1);
        assert_eq!(
            play::get(tx.conn(), stored.id).unwrap().unwrap().origin,
            PlayOrigin::ScoresDb
        );
    }

    #[test]
    fn insert_batch_rejects_constraint_violations() {
        let mut conn = migrated();
        let tx = tx(&mut conn);
        let alias_id = alias::upsert(&tx, Game::OsuStable, b"Alice").unwrap();
        // scores_db without a snapshot violates a CHECK; it must fail, not count as existing.
        let bad = new_play(alias_id, b"Alice", MD5_A, 3, None);
        assert!(play::insert_batch(&tx, &[bad]).is_err());
    }

    #[test]
    fn set_replay_sha_only_when_null() {
        let mut conn = migrated();
        let tx = tx(&mut conn);
        let snap = scores_snapshot(&tx);
        let alias_id = alias::upsert(&tx, Game::OsuStable, b"Alice").unwrap();
        let p = new_play(alias_id, b"Alice", MD5_A, 11, Some(snap));
        play::insert_batch(&tx, std::slice::from_ref(&p)).unwrap();
        assert_eq!(
            play::unlinked_replays(tx.conn()).unwrap(),
            vec![UnlinkedPlay {
                id: p.id,
                chart_md5: p.chart_md5,
                filetime: p.filetime,
                needs_replay: true,
                needs_osg: true
            }]
        );

        let first = osr_blob(&tx, 0x01);
        let second = osr_blob(&tx, 0x02);
        assert!(play::set_replay_sha(&tx, p.id, first).unwrap());
        assert!(!play::set_replay_sha(&tx, p.id, second).unwrap());
        assert_eq!(
            play::get(tx.conn(), p.id).unwrap().unwrap().replay_sha,
            Some(first)
        );
        assert!(play::set_osg_sha(&tx, p.id, second).unwrap());
        assert!(!play::set_osg_sha(&tx, p.id, first).unwrap());
        assert!(play::unlinked_replays(tx.conn()).unwrap().is_empty());

        assert_eq!(
            play::charts_without_blob(tx.conn()).unwrap(),
            vec![p.chart_md5]
        );
        let chart = osr_blob(&tx, 0x03);
        assert_eq!(play::set_chart_sha(&tx, p.chart_md5, chart).unwrap(), 1);
        assert_eq!(play::set_chart_sha(&tx, p.chart_md5, first).unwrap(), 0);
        assert!(play::charts_without_blob(tx.conn()).unwrap().is_empty());
        assert_eq!(
            play::keys_by_chart_and_time(tx.conn()).unwrap(),
            [(p.chart_md5, p.filetime)].into()
        );
    }

    #[test]
    fn install_row_created_once() {
        let mut conn = migrated();
        let tx = tx(&mut conn);
        assert_eq!(install::get(tx.conn()).unwrap(), None);
        let first = install::ensure(&tx, T0).unwrap();
        let again = install::ensure(&tx, UnixUs(T0.0 + 1_000_000)).unwrap();
        assert_eq!(first, again);
        assert_eq!(first.id.get_version_num(), 4);
        assert_ne!(first.secret, [0; 32]);
        assert_eq!(install::get(tx.conn()).unwrap(), Some(first));
        let rows: i64 =
            tx.0.query_row("SELECT count(*) FROM install", [], |r| r.get(0))
                .unwrap();
        assert_eq!(rows, 1);
    }

    #[test]
    fn game_install_upsert_and_snapshots() {
        let mut conn = migrated();
        let tx = tx(&mut conn);
        let root = Path::new("/mnt/e/Games/osu!");
        let id = game_install::upsert(&tx, Game::OsuStable, root, None, T0).unwrap();
        let later = UnixUs(T0.0 + 1_000);
        assert_eq!(
            game_install::upsert(&tx, Game::OsuStable, root, Some(20260924), later).unwrap(),
            id
        );
        let row = game_install::get(tx.conn(), id).unwrap().unwrap();
        assert_eq!(row.root_path, root);
        assert_eq!(row.client_version, Some(20260924));
        assert_eq!(row.detected_at, T0);
        assert_eq!(game_install::list(tx.conn()).unwrap(), vec![row]);

        assert_eq!(
            source_snapshot::latest(tx.conn(), id, SnapshotKind::OsuDb).unwrap(),
            None
        );
        let mut snap = NewSnapshot {
            install_id: id,
            kind: SnapshotKind::OsuDb,
            sha256: sha(1),
            size: 10,
            mtime: UnixUs(T0.0 + 123),
            format_version: None,
            imported_at: T0,
        };
        source_snapshot::insert(&tx, &snap).unwrap();
        snap.sha256 = sha(2);
        let newest = source_snapshot::insert(&tx, &snap).unwrap();
        let latest = source_snapshot::latest(tx.conn(), id, SnapshotKind::OsuDb)
            .unwrap()
            .unwrap();
        assert_eq!((latest.id, latest.sha256), (newest, sha(2)));
        // The stored mtime keeps milliseconds, so a fresh stat compares at that precision.
        assert!(latest.matches_stat(10, UnixUs(T0.0 + 999)));
        assert!(!latest.matches_stat(10, UnixUs(T0.0 + 1_000)));
        assert!(!latest.matches_stat(11, T0));
        assert_eq!(
            source_snapshot::latest(tx.conn(), id, SnapshotKind::ScoresDb).unwrap(),
            None
        );
    }

    #[test]
    fn blob_insert_is_idempotent() {
        let mut conn = migrated();
        let tx = tx(&mut conn);
        let b = NewBlob {
            sha256: sha(9),
            kind: BlobKind::Osg,
            size: 5,
            origin_path: "Data/r/x.osg".into(),
            first_seen: T0,
        };
        assert!(blob::insert(&tx, &b).unwrap());
        assert!(!blob::insert(&tx, &b).unwrap());
        assert!(blob::exists(tx.conn(), sha(9)).unwrap());
        assert!(!blob::exists(tx.conn(), sha(8)).unwrap());
        assert_eq!(blob::count(tx.conn()).unwrap(), 1);
    }

    #[test]
    fn charts_played_by_is_scoped_to_the_aliases() {
        let conn = migrated();
        crate::user::testkit::seed(&conn);
        let mut conn = conn;
        let tx = tx(&mut conn);
        let md5_a = md5(MD5_A);
        assert_eq!(
            play::charts_played_by(tx.conn(), &[AliasId(2)]).unwrap(),
            [md5_a].into()
        );
        assert_eq!(
            play::charts_played_by(tx.conn(), &[AliasId(1), AliasId(3)]).unwrap(),
            [md5_a].into()
        );
        assert!(
            play::charts_played_by(tx.conn(), &[AliasId(3)])
                .unwrap()
                .is_empty()
        );
        assert!(play::charts_played_by(tx.conn(), &[]).unwrap().is_empty());
    }

    #[test]
    fn feedback_settings_meta_roundtrip() {
        let mut conn = migrated();
        let tx = tx(&mut conn);
        let event = NewFeedbackEvent {
            id: ulid::Ulid::from_parts(1, 2),
            ts: T0,
            profile_id: None,
            kind: "identity_decision".into(),
            subject: serde_json::json!({"alias": {"game": "osu_stable"}}),
            payload: serde_json::json!({"decision": "me"}),
            context: serde_json::json!({"app_version": "0.1.0"}),
        };
        feedback_event::append(&tx, &event).unwrap();
        let stored = feedback_event::list(tx.conn()).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(
            (
                &stored[0].id,
                &stored[0].subject,
                stored[0].telemetry_state.as_str()
            ),
            (&event.id, &event.subject, "local_only")
        );

        assert_eq!(settings::get(tx.conn(), "ui.theme").unwrap(), None);
        settings::set(&tx, "ui.theme", &serde_json::json!("dark")).unwrap();
        settings::set(&tx, "ui.theme", &serde_json::json!("light")).unwrap();
        assert_eq!(
            settings::get(tx.conn(), "ui.theme").unwrap(),
            Some(serde_json::json!("light"))
        );

        meta::set(&tx, "k", "v1").unwrap();
        meta::set(&tx, "k", "v2").unwrap();
        assert_eq!(meta::get(tx.conn(), "k").unwrap().as_deref(), Some("v2"));
    }

    #[test]
    fn enum_strings_are_stable() {
        let all: Vec<&str> = SnapshotKind::ALL
            .iter()
            .map(|k| k.as_str())
            .chain(BlobKind::ALL.iter().map(|k| k.as_str()))
            .chain(PlayOrigin::ALL.iter().map(|k| k.as_str()))
            .chain(ScoreSystem::ALL.iter().map(|k| k.as_str()))
            .collect();
        assert_eq!(
            all,
            [
                "osu_db",
                "scores_db",
                "collection_db",
                "cfg",
                "songs_scan",
                "osr",
                "osg",
                "osu",
                "scores_db",
                "replay_only",
                "v1",
                "v2"
            ]
        );
        for k in SnapshotKind::ALL {
            assert_eq!(SnapshotKind::parse(k.as_str()), Some(*k));
        }
    }
}
