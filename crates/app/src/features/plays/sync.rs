//! `SyncPlays` (spec 003 Behaviour): catalog → ingest → archive → finish, one idempotent pass.
//! "Incremental" only means the pass finds little new work; there is no second code path.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use wolluf_core::{
    BlobSha256, ChartMd5, DotNetTicks, ErrorCode, FileTime, Game, PlayId, StageId, UnixUs,
    VersionKey, VersionKeyBuilder,
};
use wolluf_source_osu::cfg_files::{list_user_cfgs, read_user_cfg};
use wolluf_source_osu::codec::osr::{check_name_consistency, decode_osr};
use wolluf_source_osu::codec::osu_db::{OsuDb, OsuDbBeatmap, decode_osu_db};
use wolluf_source_osu::codec::replay_name::{ReplayFileKind, ReplayFileName};
use wolluf_source_osu::codec::score_header::ScoreHeader;
use wolluf_source_osu::codec::scores_db::decode_scores_db;
use wolluf_source_osu::install::Platform;
use wolluf_source_osu::paths::{is_drvfs_path, resolve_songs_dir};
use wolluf_source_osu::replay_dir;
use wolluf_source_osu::snapshot::{Snapshot, SnapshotPolicy, read_stable};
use wolluf_source_osu::songs::{ChartReadError, read_chart_verified};
use wolluf_source_osu::{CodecError, Diagnostics, SourceError};
use wolluf_store::Vault;
use wolluf_store::repo::cache::{
    CatalogChart, Derivation, DerivationStatus, catalog_chart, derivation,
};
use wolluf_store::repo::ledger::{
    BlobKind, GameInstall, InsertOutcome, InstallId, NewBlob, NewSnapshot, PlayOrigin, SnapshotId,
    SnapshotKind, SourceSnapshot, UnlinkedPlay, alias, blob, game_install, play, source_snapshot,
};

use crate::context::blocking_join_error;
use crate::errors::AppError;
use crate::features::players::RefreshIdentityJob;
use crate::features::plays::record::{Links, PlayDraft, chart_md5, draft};
use crate::jobs::dto::{JobKindDto, JobStageDto, JobSummaryDto, SyncSummaryDto};
use crate::jobs::{ItemError, ItemResult, Job, JobCtx, JobFuture, JobSummary};

pub const CATALOG_STAGE: StageId = StageId::from_static("catalog");
/// Bump when the catalog rows derived from one osu!.db change (spec 003 "Versioned stages").
pub const CATALOG_VERSION: u32 = 1;
const CHART_ARCHIVE_STAGE: StageId = StageId::from_static("chart_archive");
/// Bump when the rule deciding "this chart cannot be archived" changes.
const CHART_ARCHIVE_VERSION: u32 = 1;

pub(crate) const OSU_DB: &str = "osu!.db";
pub(crate) const SCORES_DB: &str = "scores.db";
/// Spec 003: write transactions of at most 2,000 plays keep the writer responsive.
const BATCH_ROWS: usize = 2_000;
pub(crate) const MANIA_MODE: u8 = 3;
/// Spec 003 "Stable read": one extra attempt for a DB osu! may still be writing.
const TORN_WRITE_RETRY: Duration = Duration::from_secs(2);
const DOMAIN_PLAYS: &str = "plays";

pub struct SyncPlaysJob {
    install_id: InstallId,
    policy: SnapshotPolicy,
}

impl SyncPlaysJob {
    pub fn new(install_id: InstallId) -> Self {
        Self {
            install_id,
            policy: SnapshotPolicy::default(),
        }
    }
}

impl Job for SyncPlaysJob {
    fn kind(&self) -> JobKindDto {
        JobKindDto::SyncPlays
    }

    fn dedupe_key(&self) -> String {
        format!("sync_plays:{}", self.install_id.0)
    }

    fn params(&self) -> serde_json::Value {
        serde_json::json!({ "installId": self.install_id.0 })
    }

    fn run(self: Box<Self>, ctx: JobCtx) -> JobFuture {
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let install = ctx
                    .user
                    .read(|c| game_install::get(c, self.install_id))?
                    .ok_or_else(|| {
                        AppError::not_found().with_arg("installId", self.install_id.0.to_string())
                    })?;
                SyncRun {
                    ctx: &ctx,
                    install,
                    policy: &self.policy,
                    summary: SyncSummaryDto::default(),
                    changed: false,
                    non_mania_scores: BTreeSet::new(),
                }
                .run()
            })
            .await
            .map_err(blocking_join_error)?
        })
    }
}

struct SyncRun<'a> {
    ctx: &'a JobCtx,
    install: GameInstall,
    policy: &'a SnapshotPolicy,
    summary: SyncSummaryDto,
    changed: bool,
    /// Non-mania scores.db rows counted by this pass: their `Data/r` replays are never
    /// ingested, so orphan import would otherwise count the same play a second time.
    non_mania_scores: BTreeSet<(ChartMd5, FileTime)>,
}

pub(crate) fn unix_us(t: SystemTime) -> UnixUs {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => UnixUs(i64::try_from(d.as_micros()).unwrap_or(i64::MAX)),
        Err(e) => UnixUs(i64::try_from(e.duration().as_micros()).map_or(i64::MIN, |us| -us)),
    }
}

/// osu! runs on case-insensitive file systems, so `Scores.db` must be found too.
pub(crate) fn root_file(root: &Path, name: &str) -> PathBuf {
    std::fs::read_dir(root)
        .ok()
        .and_then(|entries| {
            entries
                .flatten()
                .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name))
                .map(|e| e.path())
        })
        .unwrap_or_else(|| root.join(name))
}

fn stat(path: &Path) -> Result<(u64, UnixUs), AppError> {
    let meta =
        std::fs::metadata(path).map_err(|_| AppError::osu_dir_not_found(path.to_string_lossy()))?;
    let mtime = meta
        .modified()
        .map_err(|e| AppError::internal(format!("mtime of {}: {e}", path.display())))?;
    Ok((meta.len(), unix_us(mtime)))
}

fn lossy(s: &wolluf_source_osu::codec::OsuString) -> String {
    s.to_string_lossy()
        .map(|c| c.into_owned())
        .unwrap_or_default()
}

/// Mania entries only; the first entry wins when osu!.db lists one md5 twice (a map copied into
/// two folders), because `catalog_chart.md5` is the key.
fn catalog_rows(db: &OsuDb) -> Vec<CatalogChart> {
    let mut rows = std::collections::BTreeMap::new();
    for b in db.beatmaps.iter().filter(|b| b.mode == MANIA_MODE) {
        let Some(md5) = b.beatmap_md5() else { continue };
        rows.entry(md5).or_insert_with(|| catalog_row(md5, b));
    }
    rows.into_values().collect()
}

fn catalog_row(md5: wolluf_core::ChartMd5, b: &OsuDbBeatmap) -> CatalogChart {
    let positive = |id: i32| (id > 0).then_some(id);
    CatalogChart {
        md5,
        // osu!mania keeps the key count in CS (spec 003 step 1: `round(CS)`).
        keymode: b.circle_size.round().clamp(0.0, f32::from(u8::MAX)) as u8,
        title: lossy(&b.title),
        artist: lossy(&b.artist),
        version: lossy(&b.difficulty),
        creator: lossy(&b.creator),
        set_id: positive(b.beatmapset_id),
        beatmap_id: positive(b.beatmap_id),
        path: format!("{}/{}", lossy(&b.folder), lossy(&b.osu_file)),
        od: f64::from(b.overall_difficulty),
        hp: f64::from(b.hp_drain),
        length_ms: u32::try_from(b.total_time_ms).unwrap_or(0),
    }
}

/// Upserts each distinct raw name once per transaction.
fn alias_ids<'a>(
    tx: &wolluf_store::Tx<'_>,
    names: impl IntoIterator<Item = &'a [u8]>,
) -> Result<BTreeMap<Vec<u8>, wolluf_core::AliasId>, wolluf_store::StoreError> {
    let mut ids = BTreeMap::new();
    for name in names {
        if !ids.contains_key(name) {
            ids.insert(name.to_vec(), alias::upsert(tx, Game::OsuStable, name)?);
        }
    }
    Ok(ids)
}

fn alias_of(
    ids: &BTreeMap<Vec<u8>, wolluf_core::AliasId>,
    raw_name: &[u8],
) -> Result<wolluf_core::AliasId, wolluf_store::StoreError> {
    ids.get(raw_name)
        .copied()
        .ok_or_else(|| wolluf_store::StoreError::InvalidData("alias id missing".into()))
}

/// `Data/r/<name>` or `Songs/<path>`: where the bytes came from, relative to the install.
fn origin_path(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn read_file(path: &Path) -> Result<Vec<u8>, ItemError> {
    std::fs::read(path).map_err(|e| {
        ItemError::new(
            ErrorCode::Internal,
            format!("read {}: {:?}", path.display(), e.kind()),
        )
    })
}

/// Vault first, row second: the blob is durable before anything points at it (spec 003).
fn vault_blob(
    vault: &Vault,
    root: &Path,
    path: &Path,
    kind: BlobKind,
    bytes: &[u8],
    now: UnixUs,
) -> Result<NewBlob, ItemError> {
    let sha256 = vault
        .put(bytes)
        .map_err(|e| ItemError::new(ErrorCode::Internal, e.to_string()))?;
    Ok(NewBlob {
        sha256,
        kind,
        size: bytes.len() as u64,
        origin_path: origin_path(root, path),
        first_seen: now,
    })
}

/// The newest cfg decides `BeatmapDirectory` (002 R-d); anything unreadable means `Songs`.
fn songs_dir(root: &Path) -> PathBuf {
    let beatmap_directory = list_user_cfgs(root)
        .ok()
        .and_then(|cfgs| cfgs.into_iter().next())
        .and_then(|cfg| read_user_cfg(&cfg.path).ok())
        .and_then(|(cfg, _)| cfg.beatmap_directory);
    let platform = if cfg!(windows) {
        Platform::Windows
    } else if is_drvfs_path(root) {
        Platform::Wsl
    } else {
        Platform::Linux
    };
    resolve_songs_dir(root, beatmap_directory.as_deref(), platform)
}

fn chart_archive_key(
    md5: ChartMd5,
    osu_db_sha: BlobSha256,
) -> Result<(String, VersionKey), AppError> {
    let input = blake3::Hasher::new()
        .update(&md5.0)
        .update(&osu_db_sha.0)
        .finalize();
    let vkey = VersionKeyBuilder::new(CHART_ARCHIVE_STAGE, CHART_ARCHIVE_VERSION)
        .input(*input.as_bytes())
        .finish()
        .map_err(|e| AppError::internal(format!("chart_archive vkey: {e}")))?;
    Ok((format!("{md5}:{osu_db_sha}"), vkey))
}

struct LinkItem {
    play: UnlinkedPlay,
    osr: Option<PathBuf>,
    osg: Option<PathBuf>,
}

struct LinkReady {
    play: PlayId,
    replay: Option<NewBlob>,
    osg: Option<NewBlob>,
    /// The `.osr` header disagreed with the play; its `.osg` may still link.
    refused: Option<ItemError>,
}

fn prepare_link(
    vault: &Vault,
    root: &Path,
    item: &LinkItem,
    now: UnixUs,
) -> Result<LinkReady, ItemError> {
    let mut ready = LinkReady {
        play: item.play.id,
        replay: None,
        osg: None,
        refused: None,
    };
    if let Some(path) = &item.osr {
        let bytes = read_file(path)?;
        let (osr, _) = decode_osr(&bytes).map_err(|e| ItemError::new(e.code(), e.to_string()))?;
        let name = ReplayFileName {
            md5: item.play.chart_md5,
            filetime: item.play.filetime,
            kind: ReplayFileKind::Osr,
        };
        if check_name_consistency(&name, &osr.header).is_empty() {
            ready.replay = Some(vault_blob(vault, root, path, BlobKind::Osr, &bytes, now)?);
        } else {
            ready.refused = Some(ItemError::new(
                ErrorCode::Conflict,
                ".osr header md5 or time differs from the play",
            ));
        }
    }
    if let Some(path) = &item.osg {
        let bytes = read_file(path)?;
        ready.osg = Some(vault_blob(vault, root, path, BlobKind::Osg, &bytes, now)?);
    }
    Ok(ready)
}

struct OrphanItem {
    name: ReplayFileName,
    osr: PathBuf,
    osg: Option<PathBuf>,
}

enum Orphan {
    Play {
        draft: PlayDraft,
        replay: NewBlob,
        osg: Option<NewBlob>,
    },
    /// Kept in the vault (the replay may be the only copy) but not a play.
    BlobOnly {
        replay: NewBlob,
        why: ItemError,
    },
    NonMania,
}

/// The `Data/r` name key of a score; `None` when no replay name could carry it.
fn play_key(header: &ScoreHeader) -> Option<(ChartMd5, FileTime)> {
    let md5 = chart_md5(header).ok()?;
    let filetime = DotNetTicks(header.timestamp_ticks).to_filetime()?;
    Some((md5, filetime))
}

/// A replay without a score row is still a play (user decision 2026-09-28), built from its
/// header exactly like a scores.db record, provided the header agrees with its name.
fn prepare_orphan(
    vault: &Vault,
    root: &Path,
    item: &OrphanItem,
    now: UnixUs,
) -> Result<Orphan, ItemError> {
    let bytes = read_file(&item.osr)?;
    let blob_only = |why: ItemError| -> Result<Orphan, ItemError> {
        let replay = vault_blob(vault, root, &item.osr, BlobKind::Osr, &bytes, now)?;
        Ok(Orphan::BlobOnly { replay, why })
    };
    let osr = match decode_osr(&bytes) {
        Ok((osr, _)) => osr,
        Err(e) => return blob_only(ItemError::new(e.code(), e.to_string())),
    };
    if osr.header.mode != MANIA_MODE {
        return Ok(Orphan::NonMania);
    }
    if !check_name_consistency(&item.name, &osr.header).is_empty() {
        return blob_only(ItemError::new(
            ErrorCode::Conflict,
            ".osr header md5 or time differs from its file name",
        ));
    }
    let draft = match draft(&osr.header, osr.online_id) {
        Ok(d) => d,
        Err(e) => return blob_only(e),
    };
    let replay = vault_blob(vault, root, &item.osr, BlobKind::Osr, &bytes, now)?;
    let osg = match &item.osg {
        Some(path) => Some(vault_blob(
            vault,
            root,
            path,
            BlobKind::Osg,
            &read_file(path)?,
            now,
        )?),
        None => None,
    };
    Ok(Orphan::Play { draft, replay, osg })
}

enum ChartOutcome {
    Archived(NewBlob),
    Mismatch,
    Missing,
}

fn prepare_chart(
    vault: &Vault,
    songs: &Path,
    chart: &(ChartMd5, String),
    now: UnixUs,
) -> Result<ChartOutcome, ItemError> {
    let (md5, rel) = chart;
    match read_chart_verified(songs, Path::new(rel), *md5) {
        Ok(bytes) => {
            let path = songs.join(rel);
            let sha256 = vault
                .put(&bytes)
                .map_err(|e| ItemError::new(ErrorCode::Internal, e.to_string()))?;
            Ok(ChartOutcome::Archived(NewBlob {
                sha256,
                kind: BlobKind::Osu,
                size: bytes.len() as u64,
                origin_path: format!("Songs/{}", origin_path(songs, &path)),
                first_seen: now,
            }))
        }
        Err(ChartReadError::Md5Mismatch { .. }) => Ok(ChartOutcome::Mismatch),
        Err(ChartReadError::Missing) => Ok(ChartOutcome::Missing),
        Err(e @ ChartReadError::Io { .. }) => Err(ItemError::new(e.code(), e.to_string())),
    }
}

fn catalog_vkey(osu_db_sha: wolluf_core::BlobSha256) -> Result<VersionKey, AppError> {
    VersionKeyBuilder::new(CATALOG_STAGE, CATALOG_VERSION)
        .input(osu_db_sha.0)
        .finish()
        .map_err(|e| AppError::internal(format!("catalog vkey: {e}")))
}

impl SyncRun<'_> {
    fn run(mut self) -> Result<JobSummary, AppError> {
        let osu_db = self.catalog()?;
        self.ctx.check_cancelled()?;
        self.ingest()?;
        self.ctx.check_cancelled()?;
        self.archive(&osu_db)?;
        self.summary.failed_items = self.ctx.failed_items();
        Ok(JobSummary {
            summary: Some(JobSummaryDto::SyncPlays(self.summary)),
            changed: if self.changed {
                vec![DOMAIN_PLAYS]
            } else {
                Vec::new()
            },
            // Every sync refreshes identity, so the wizard never reads stale stats (spec 004).
            follow_ups: vec![Box::new(RefreshIdentityJob)],
        })
    }

    fn root(&self) -> &Path {
        &self.install.root_path
    }

    /// Reads with the stable-read rule and decodes, giving a torn write one late retry.
    fn read_decoded<T>(
        &self,
        path: &Path,
        decode: impl Fn(&[u8]) -> Result<(T, Diagnostics), CodecError>,
    ) -> Result<(Snapshot, T), AppError> {
        let snapshot = read_stable(path, self.policy)?;
        match decode(&snapshot.bytes) {
            Ok((value, _)) => Ok((snapshot, value)),
            Err(e) if e.is_possibly_torn_write() => {
                (self.policy.sleep)(TORN_WRITE_RETRY);
                let snapshot = read_stable(path, self.policy)?;
                let (value, _) = decode(&snapshot.bytes).map_err(SourceError::from)?;
                Ok((snapshot, value))
            }
            Err(e) => Err(SourceError::from(e).into()),
        }
    }

    fn catalog_built(&self, sha: wolluf_core::BlobSha256) -> Result<bool, AppError> {
        let vkey = catalog_vkey(sha)?;
        let key = sha.to_string();
        Ok(self
            .ctx
            .cache
            .read(|c| derivation::get(c, &CATALOG_STAGE, &key, vkey))?
            .is_some())
    }

    /// Step 1. Returns the osu!.db snapshot the catalog now reflects.
    fn catalog(&mut self) -> Result<SourceSnapshot, AppError> {
        let progress = &self.ctx.progress;
        progress.report(JobStageDto::Catalog, 0, 1);
        let path = root_file(self.root(), OSU_DB);
        let (size, mtime) = stat(&path)?;
        let install_id = self.install.id;
        let latest = self
            .ctx
            .user
            .read(|c| source_snapshot::latest(c, install_id, SnapshotKind::OsuDb))?;
        if let Some(latest) = latest.as_ref().filter(|l| l.matches_stat(size, mtime))
            && self.catalog_built(latest.sha256)?
        {
            progress.report(JobStageDto::Catalog, 1, 1);
            return Ok(latest.clone());
        }

        let snapshot = read_stable(&path, self.policy)?;
        let reused = latest.filter(|l| l.sha256 == snapshot.sha256);
        if let Some(known) = &reused
            && self.catalog_built(known.sha256)?
        {
            progress.report(JobStageDto::Catalog, 1, 1);
            return Ok(known.clone());
        }
        let (snapshot, db) = match decode_osu_db(&snapshot.bytes) {
            Ok((db, _)) => (snapshot, db),
            Err(e) if e.is_possibly_torn_write() => self.read_decoded(&path, decode_osu_db)?,
            Err(e) => return Err(SourceError::from(e).into()),
        };
        let row = match reused.filter(|r| r.sha256 == snapshot.sha256) {
            Some(row) => row,
            None => self.insert_snapshot(SnapshotKind::OsuDb, &snapshot, db.version)?,
        };

        let rows = catalog_rows(&db);
        let vkey = catalog_vkey(row.sha256)?;
        let snapshot_id = row.id;
        let input_key = row.sha256.to_string();
        self.ctx.cache.write(move |tx| {
            catalog_chart::replace_all(tx, snapshot_id, &rows)?;
            derivation::put(
                tx,
                &Derivation {
                    stage: CATALOG_STAGE,
                    input_key,
                    vkey,
                    status: DerivationStatus::Ok,
                    error_code: None,
                    error_msg: None,
                    duration_ms: None,
                },
            )
        })?;
        self.changed = true;
        progress.report(JobStageDto::Catalog, 1, 1);
        Ok(row)
    }

    /// Step 2. Not cancellable midway: once started it commits every batch, because an
    /// unchanged scores.db sha skips the step next time. A crash between batches is healed by
    /// the next scores.db change (osu! rewrites it on every play), which re-ingests all records
    /// idempotently.
    fn ingest(&mut self) -> Result<(), AppError> {
        let path = root_file(self.root(), SCORES_DB);
        if !path.is_file() {
            return Err(AppError::osu_dir_not_found(path.to_string_lossy()));
        }
        let snapshot = read_stable(&path, self.policy)?;
        let install_id = self.install.id;
        let latest = self
            .ctx
            .user
            .read(|c| source_snapshot::latest(c, install_id, SnapshotKind::ScoresDb))?;
        if latest.is_some_and(|l| l.sha256 == snapshot.sha256) {
            return Ok(());
        }
        let (snapshot, db) = match decode_scores_db(&snapshot.bytes) {
            Ok((db, _)) => (snapshot, db),
            Err(e) if e.is_possibly_torn_write() => self.read_decoded(&path, decode_scores_db)?,
            Err(e) => return Err(SourceError::from(e).into()),
        };

        let mut drafts: Vec<PlayDraft> = Vec::new();
        let mut failures: Vec<(String, ItemError)> = Vec::new();
        for (index, record) in db.scores().enumerate() {
            if record.header.mode != MANIA_MODE {
                self.summary.skipped_non_mania += 1;
                if let Some(key) = play_key(&record.header) {
                    self.non_mania_scores.insert(key);
                }
                continue;
            }
            match draft(&record.header, record.online_id) {
                Ok(d) => drafts.push(d),
                Err(e) => failures.push((format!("scores.db#{index}"), e)),
            }
        }

        let new_snapshot = NewSnapshot {
            install_id,
            kind: SnapshotKind::ScoresDb,
            sha256: snapshot.sha256,
            size: snapshot.size,
            mtime: unix_us(snapshot.mtime),
            format_version: Some(db.version),
            imported_at: self.ctx.clock.now(),
        };
        let total = u32::try_from(drafts.len()).unwrap_or(u32::MAX);
        self.ctx.progress.report(JobStageDto::Ingest, 0, total);
        let mut snapshot_id: Option<SnapshotId> = None;
        let mut outcome = InsertOutcome::default();
        let mut done = 0_u32;
        // An empty chunk list still records the snapshot, so the next pass skips it.
        let chunks: Vec<Vec<PlayDraft>> = if drafts.is_empty() {
            vec![Vec::new()]
        } else {
            drafts
                .chunks(BATCH_ROWS)
                .map(<[PlayDraft]>::to_vec)
                .collect()
        };
        for chunk in chunks {
            let n = u32::try_from(chunk.len()).unwrap_or(u32::MAX);
            let (sid, batch) = self.insert_plays(snapshot_id, &new_snapshot, chunk)?;
            snapshot_id = Some(sid);
            outcome.new += batch.new;
            outcome.existing += batch.existing;
            outcome.upgraded += batch.upgraded;
            outcome.conflicts.extend(batch.conflicts);
            done += n;
            self.ctx.progress.report(JobStageDto::Ingest, done, total);
        }

        self.summary.plays_new += outcome.new;
        self.summary.plays_existing += outcome.existing + outcome.upgraded;
        self.summary.conflicts += u32::try_from(outcome.conflicts.len()).unwrap_or(u32::MAX);
        self.changed |= outcome.new > 0 || outcome.upgraded > 0;
        for id in &outcome.conflicts {
            let e = ItemError::new(
                ErrorCode::Conflict,
                "a stored play has the same natural key and different ledger fields",
            );
            self.ctx.record_failure(&id.to_string(), &e)?;
        }
        for (item, e) in &failures {
            self.ctx.record_failure(item, e)?;
        }
        Ok(())
    }

    /// One transaction: the snapshot row (first batch only), the aliases and the plays.
    fn insert_plays(
        &self,
        snapshot_id: Option<SnapshotId>,
        new_snapshot: &NewSnapshot,
        chunk: Vec<PlayDraft>,
    ) -> Result<(SnapshotId, InsertOutcome), AppError> {
        let new_snapshot = new_snapshot.clone();
        let now = self.ctx.clock.now();
        Ok(self.ctx.user.write(move |tx| {
            let sid = match snapshot_id {
                Some(sid) => sid,
                None => source_snapshot::insert(tx, &new_snapshot)?,
            };
            let ids = alias_ids(tx, chunk.iter().map(|d| d.raw_name.as_slice()))?;
            let plays = chunk
                .into_iter()
                .map(|d| {
                    let alias_id = alias_of(&ids, &d.raw_name)?;
                    let links = Links {
                        origin: PlayOrigin::ScoresDb,
                        snapshot_id: Some(sid),
                        replay_sha: None,
                        osg_sha: None,
                    };
                    Ok(d.into_new_play(alias_id, links, now))
                })
                .collect::<Result<Vec<_>, wolluf_store::StoreError>>()?;
            Ok((sid, play::insert_batch(tx, &plays)?))
        })?)
    }

    /// Step 3. Everything committed stays committed; a cancelled pass resumes from the NULL
    /// links on the next run.
    fn archive(&mut self, osu_db: &SourceSnapshot) -> Result<(), AppError> {
        let index = replay_dir::index(self.root())?;
        self.link_replays(&index)?;
        self.ctx.check_cancelled()?;
        self.import_orphans(&index)?;
        self.ctx.check_cancelled()?;
        self.archive_charts(osu_db)
    }

    fn link_replays(
        &mut self,
        index: &BTreeMap<(ChartMd5, FileTime), replay_dir::ReplayFiles>,
    ) -> Result<(), AppError> {
        let unlinked = self.ctx.user.read(play::unlinked_replays)?;
        let items: Vec<LinkItem> = unlinked
            .into_iter()
            .filter_map(|p| {
                let files = index.get(&(p.chart_md5, p.filetime))?;
                let osr = files.osr.clone().filter(|_| p.needs_replay);
                let osg = files.osg.clone().filter(|_| p.needs_osg);
                (osr.is_some() || osg.is_some()).then_some(LinkItem { play: p, osr, osg })
            })
            .collect();
        let (vault, root, now) = (&self.ctx.vault, self.root(), self.ctx.clock.now());
        let results = self.ctx.run_items(
            JobStageDto::Archive,
            &items,
            |i| i.play.id.to_string(),
            |i| prepare_link(vault, root, i, now),
        )?;
        let ready: Vec<LinkReady> = results
            .into_iter()
            .filter_map(|r| match r {
                ItemResult::Done(ready) => Some(ready),
                ItemResult::Failed(_) | ItemResult::Skipped => None,
            })
            .collect();
        for r in &ready {
            if let Some(why) = &r.refused {
                self.ctx.record_failure(&r.play.to_string(), why)?;
            }
        }
        for chunk in ready.chunks(BATCH_ROWS) {
            let chunk: Vec<(PlayId, Option<NewBlob>, Option<NewBlob>)> = chunk
                .iter()
                .map(|r| (r.play, r.replay.clone(), r.osg.clone()))
                .collect();
            let (replays, osgs) = self.ctx.user.write(move |tx| {
                let (mut replays, mut osgs) = (0_u32, 0_u32);
                for (id, replay, osg) in &chunk {
                    if let Some(b) = replay {
                        blob::insert(tx, b)?;
                        replays += u32::from(play::set_replay_sha(tx, *id, b.sha256)?);
                    }
                    if let Some(b) = osg {
                        blob::insert(tx, b)?;
                        osgs += u32::from(play::set_osg_sha(tx, *id, b.sha256)?);
                    }
                }
                Ok((replays, osgs))
            })?;
            self.summary.replays_linked += replays;
            self.summary.osg_linked += osgs;
            self.changed |= replays + osgs > 0;
        }
        Ok(())
    }

    fn import_orphans(
        &mut self,
        index: &BTreeMap<(ChartMd5, FileTime), replay_dir::ReplayFiles>,
    ) -> Result<(), AppError> {
        let keys = self.ctx.user.read(play::keys_by_chart_and_time)?;
        let items: Vec<OrphanItem> = index
            .iter()
            .filter(|(key, _)| !keys.contains(key))
            .filter_map(|(&(md5, filetime), files)| {
                Some(OrphanItem {
                    name: ReplayFileName {
                        md5,
                        filetime,
                        kind: ReplayFileKind::Osr,
                    },
                    osr: files.osr.clone()?,
                    osg: files.osg.clone(),
                })
            })
            .collect();
        let root = self.install.root_path.clone();
        let (vault, now) = (&self.ctx.vault, self.ctx.clock.now());
        let results = self.ctx.run_items(
            JobStageDto::Archive,
            &items,
            |i| origin_path(&root, &i.osr),
            |i| prepare_orphan(vault, &root, i, now),
        )?;

        let mut plays: Vec<(PlayDraft, NewBlob, Option<NewBlob>)> = Vec::new();
        let mut blobs_only: Vec<NewBlob> = Vec::new();
        for (item, result) in items.iter().zip(results) {
            match result {
                ItemResult::Done(Orphan::Play { draft, replay, osg }) => {
                    plays.push((draft, replay, osg));
                }
                ItemResult::Done(Orphan::BlobOnly { replay, why }) => {
                    self.ctx
                        .record_failure(&origin_path(&root, &item.osr), &why)?;
                    self.summary.orphan_replays += 1;
                    blobs_only.push(replay);
                }
                ItemResult::Done(Orphan::NonMania) => {
                    if !self
                        .non_mania_scores
                        .contains(&(item.name.md5, item.name.filetime))
                    {
                        self.summary.skipped_non_mania += 1;
                    }
                }
                ItemResult::Failed(_) | ItemResult::Skipped => {}
            }
        }
        if plays.is_empty() && blobs_only.is_empty() {
            return Ok(());
        }
        let now = self.ctx.clock.now();
        let outcome = self.ctx.user.write(move |tx| {
            for b in &blobs_only {
                blob::insert(tx, b)?;
            }
            let ids = alias_ids(tx, plays.iter().map(|(d, _, _)| d.raw_name.as_slice()))?;
            let mut rows = Vec::with_capacity(plays.len());
            for (draft, replay, osg) in plays {
                blob::insert(tx, &replay)?;
                if let Some(osg) = &osg {
                    blob::insert(tx, osg)?;
                }
                let alias_id = alias_of(&ids, &draft.raw_name)?;
                let links = Links {
                    origin: PlayOrigin::ReplayOnly,
                    snapshot_id: None,
                    replay_sha: Some(replay.sha256),
                    osg_sha: osg.map(|o| o.sha256),
                };
                rows.push(draft.into_new_play(alias_id, links, now));
            }
            play::insert_batch(tx, &rows)
        })?;
        self.summary.plays_replay_only += outcome.new;
        self.summary.plays_existing += outcome.existing + outcome.upgraded;
        self.changed |= outcome.new > 0;
        for id in &outcome.conflicts {
            let e = ItemError::new(
                ErrorCode::Conflict,
                "orphan replay collides with a stored play",
            );
            self.ctx.record_failure(&id.to_string(), &e)?;
        }
        Ok(())
    }

    fn archive_charts(&mut self, osu_db: &SourceSnapshot) -> Result<(), AppError> {
        let wanted = self.ctx.user.read(play::charts_without_blob)?;
        let mut items: Vec<(ChartMd5, String)> = Vec::new();
        for md5 in wanted {
            let Some(chart) = self.ctx.cache.read(|c| catalog_chart::get(c, md5))? else {
                self.summary.chart_unavailable += 1;
                continue;
            };
            let (key, vkey) = chart_archive_key(md5, osu_db.sha256)?;
            let known_mismatch = self
                .ctx
                .cache
                .read(|c| derivation::get(c, &CHART_ARCHIVE_STAGE, &key, vkey))?
                .is_some();
            if known_mismatch {
                // Not reread until the osu!.db snapshot changes (spec 003 step 3).
                self.summary.chart_md5_mismatch += 1;
            } else {
                items.push((md5, chart.path));
            }
        }
        let songs = songs_dir(self.root());
        let (vault, now) = (&self.ctx.vault, self.ctx.clock.now());
        let results = self.ctx.run_items(
            JobStageDto::Archive,
            &items,
            |(md5, _)| md5.to_string(),
            |chart| prepare_chart(vault, &songs, chart, now),
        )?;
        let mut archived: Vec<(ChartMd5, NewBlob)> = Vec::new();
        let mut mismatched: Vec<Derivation> = Vec::new();
        for ((md5, _), result) in items.iter().zip(results) {
            match result {
                ItemResult::Done(ChartOutcome::Archived(b)) => archived.push((*md5, b)),
                ItemResult::Done(ChartOutcome::Mismatch) => {
                    let (input_key, vkey) = chart_archive_key(*md5, osu_db.sha256)?;
                    mismatched.push(Derivation {
                        stage: CHART_ARCHIVE_STAGE,
                        input_key,
                        vkey,
                        status: DerivationStatus::Skipped,
                        error_code: Some(ErrorCode::Conflict),
                        error_msg: Some("chart md5 differs from osu!.db".to_owned()),
                        duration_ms: None,
                    });
                }
                ItemResult::Done(ChartOutcome::Missing) => self.summary.chart_unavailable += 1,
                ItemResult::Failed(_) | ItemResult::Skipped => {}
            }
        }
        self.summary.chart_md5_mismatch += u32::try_from(mismatched.len()).unwrap_or(u32::MAX);
        if !mismatched.is_empty() {
            self.ctx
                .cache
                .write(move |tx| mismatched.iter().try_for_each(|d| derivation::put(tx, d)))?;
        }
        for chunk in archived.chunks(BATCH_ROWS) {
            let chunk = chunk.to_vec();
            let n = self.ctx.user.write(move |tx| {
                let mut n = 0_u32;
                for (md5, b) in &chunk {
                    blob::insert(tx, b)?;
                    n += u32::from(play::set_chart_sha(tx, *md5, b.sha256)? > 0);
                }
                Ok(n)
            })?;
            self.summary.charts_archived += n;
            self.changed |= n > 0;
        }
        Ok(())
    }

    fn insert_snapshot(
        &self,
        kind: SnapshotKind,
        snapshot: &Snapshot,
        format_version: i32,
    ) -> Result<SourceSnapshot, AppError> {
        let new = NewSnapshot {
            install_id: self.install.id,
            kind,
            sha256: snapshot.sha256,
            size: snapshot.size,
            mtime: unix_us(snapshot.mtime),
            format_version: Some(format_version),
            imported_at: self.ctx.clock.now(),
        };
        let install_id = self.install.id;
        let row = self.ctx.user.write(move |tx| {
            source_snapshot::insert(tx, &new)?;
            source_snapshot::latest(tx.conn(), install_id, kind)
        })?;
        row.ok_or_else(|| AppError::internal("snapshot row vanished after insert"))
    }
}

#[cfg(test)]
mod tests {
    use wolluf_core::{DotNetTicks, FileTime, PlayId};
    use wolluf_source_osu::codec::OsuString;
    use wolluf_source_osu::codec::score_header::mods;
    use wolluf_source_osu::testkit::{
        BeatmapBuilder, FakeInstall, OsrBuilder, OsuDbBuilder, ScoreBuilder,
    };
    use wolluf_store::repo::cache::{catalog_chart, item_failure};
    use wolluf_store::repo::ledger::{Play, ScoreSystem};

    use super::*;
    use crate::features::plays::testkit::{
        Fixture, NON_UTF8_NAME, ac10, md5_hex, osg_bytes, osg_name, osr_name, scores_db,
    };

    fn plays(f: &Fixture) -> Vec<Play> {
        let ids: Vec<PlayId> = f
            .ctx
            .user_db()
            .read(|c| {
                let facts = wolluf_store::repo::players::identity_facts(c)?;
                Ok(facts.plays.iter().map(|p| p.play_id).collect())
            })
            .unwrap();
        ids.into_iter()
            .map(|id| f.ctx.user_db().read(|c| play::get(c, id)).unwrap().unwrap())
            .collect()
    }

    fn counts(f: &Fixture) -> (u64, u64) {
        f.ctx
            .user_db()
            .read(|c| Ok((play::count(c)?, alias::count(c)?)))
            .unwrap()
    }

    fn blob_count(f: &Fixture) -> u64 {
        f.ctx.user_db().read(blob::count).unwrap()
    }

    fn vault_files(f: &Fixture) -> usize {
        fn walk(dir: &Path) -> usize {
            std::fs::read_dir(dir).map_or(0, |entries| {
                entries
                    .flatten()
                    .map(|e| {
                        let p = e.path();
                        if p.is_dir() { walk(&p) } else { 1 }
                    })
                    .sum()
            })
        }
        walk(&f.ctx.paths().vault_dir())
    }

    fn stored(f: &Fixture, score: &ScoreBuilder, raw_name: &[u8]) -> Option<Play> {
        f.ctx
            .user_db()
            .read(|c| play::get(c, play_id(score, raw_name)))
            .unwrap()
    }

    fn failures(
        f: &Fixture,
        job: &crate::jobs::JobId,
    ) -> Vec<wolluf_store::repo::cache::ItemFailure> {
        let ulid = ulid::Ulid::from_string(&job.0).unwrap();
        f.ctx
            .cache_db()
            .read(|c| item_failure::list(c, ulid))
            .unwrap()
    }

    fn play_id(score: &ScoreBuilder, raw_name: &[u8]) -> PlayId {
        let h = score.header();
        let md5 = std::str::from_utf8(h.beatmap_md5.as_bytes().unwrap())
            .unwrap()
            .parse()
            .unwrap();
        let filetime: FileTime = DotNetTicks(h.timestamp_ticks).to_filetime().unwrap();
        PlayId::derive(Game::OsuStable, md5, raw_name, filetime)
    }

    fn osu_db(md5s: &[&str]) -> Vec<u8> {
        md5s.iter()
            .fold(OsuDbBuilder::new(), |db, md5| {
                db.beatmap(BeatmapBuilder::mania(md5, 7).build())
            })
            .beatmap(
                BeatmapBuilder::mania(&md5_hex(b"std map"), 4)
                    .mode(0)
                    .build(),
            )
            .encode()
    }

    fn snapshots(f: &Fixture, kind: SnapshotKind) -> Option<SourceSnapshot> {
        f.ctx
            .user_db()
            .read(|c| source_snapshot::latest(c, f.install, kind))
            .unwrap()
    }

    fn catalog(f: &Fixture) -> Vec<CatalogChart> {
        f.ctx.cache_db().read(catalog_chart::list_all).unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn catalog_skipped_when_osu_db_unchanged() {
        let a = md5_hex(b"chart a");
        let f = Fixture::new(&FakeInstall::new().osu_db(osu_db(&[&a]))).await;
        let (_, first) = f.sync().await;
        assert_eq!(first.failed_items, 0);
        let snap = snapshots(&f, SnapshotKind::OsuDb).unwrap();
        assert_eq!(catalog(&f).len(), 1, "std maps stay out of the catalog");

        // Same size and mtime but garbage bytes: only a real read would notice.
        let path = f.root.join(OSU_DB);
        let meta = std::fs::metadata(&path).unwrap();
        let garbage = vec![0xee; usize::try_from(meta.len()).unwrap()];
        std::fs::write(&path, &garbage).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(meta.modified().unwrap())
            .unwrap();
        let (fin, _) = f.sync().await;
        assert_eq!(fin.failed_items, 0);
        assert_eq!(snapshots(&f, SnapshotKind::OsuDb).unwrap().id, snap.id);
        assert_eq!(catalog(&f).len(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn catalog_replaced_on_new_sha() {
        let (a, b, c) = (md5_hex(b"a"), md5_hex(b"b"), md5_hex(b"c"));
        let f = Fixture::new(&FakeInstall::new().osu_db(osu_db(&[&a, &b]))).await;
        f.sync().await;
        let first = snapshots(&f, SnapshotKind::OsuDb).unwrap();
        let md5s =
            |f: &Fixture| -> Vec<String> { catalog(f).iter().map(|c| c.md5.to_string()).collect() };
        let mut expected = vec![a.clone(), b.clone()];
        expected.sort();
        assert_eq!(md5s(&f), expected);

        f.write(OSU_DB, &osu_db(&[&c]));
        f.sync().await;
        let second = snapshots(&f, SnapshotKind::OsuDb).unwrap();
        assert_ne!(first.id, second.id);
        assert_ne!(first.sha256, second.sha256);
        assert_eq!(md5s(&f), vec![c]);
        let charts = catalog(&f);
        assert_eq!(charts[0].keymode, 7);
        assert_eq!(charts[0].path, format!("{0}/{0}.osu", charts[0].md5));
        let vkey = catalog_vkey(second.sha256).unwrap();
        let row = f
            .ctx
            .cache_db()
            .read(|c| derivation::get(c, &CATALOG_STAGE, &second.sha256.to_string(), vkey))
            .unwrap()
            .unwrap();
        assert_eq!(row.status, DerivationStatus::Ok);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn catalog_rebuilt_after_cache_loss() {
        let a = md5_hex(b"a");
        let f = Fixture::new(&FakeInstall::new().osu_db(osu_db(&[&a]))).await;
        f.sync().await;
        let snap = snapshots(&f, SnapshotKind::OsuDb).unwrap();
        let f = f.reopen_without_cache();
        f.sync().await;
        assert_eq!(catalog(&f).len(), 1);
        assert_eq!(
            snapshots(&f, SnapshotKind::OsuDb).unwrap().id,
            snap.id,
            "an unchanged osu!.db reuses its snapshot row"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn sync_ingests_fixture_install() {
        let fx = ac10();
        let f = Fixture::new(&fx.install).await;
        let (fin, s) = f.sync().await;
        let expected = SyncSummaryDto {
            plays_new: 6,
            plays_replay_only: 1,
            plays_existing: 1,
            conflicts: 0,
            skipped_non_mania: 1,
            replays_linked: 5,
            osg_linked: 3,
            charts_archived: 1,
            // chart_c has plays but is not in osu!.db.
            chart_unavailable: 1,
            chart_md5_mismatch: 1,
            orphan_replays: 1,
            // The orphan whose header md5 differs from its name.
            failed_items: 1,
        };
        assert_eq!(s, expected);
        assert_eq!(counts(&f).0, 7);
        let names: BTreeMap<Vec<u8>, ()> = f
            .ctx
            .user_db()
            .read(alias::list)
            .unwrap()
            .into_iter()
            .map(|a| (a.raw_name, ()))
            .collect();
        assert!(names.contains_key(&Vec::new()), "\"\" is a valid alias");
        assert!(
            names.contains_key(NON_UTF8_NAME),
            "non-UTF-8 bytes kept as they are"
        );
        assert!(!names.contains_key(b"x".as_slice()));
        let all = plays(&f);
        let charts: std::collections::BTreeSet<String> =
            all.iter().map(|p| p.chart_md5.to_string()).collect();
        assert_eq!(
            charts,
            [&fx.chart_a, &fx.chart_b, &fx.chart_c]
                .into_iter()
                .cloned()
                .collect()
        );
        assert_eq!(fin.failed_items, 1);
        let chart_a: ChartMd5 = fx.chart_a.parse().unwrap();
        assert!(
            all.iter()
                .all(|p| p.chart_sha.is_some() == (p.chart_md5 == chart_a)),
            "only the unedited catalog chart is archived"
        );
        let with_replay = all.iter().filter(|p| p.replay_sha.is_some()).count();
        assert_eq!(with_replay, 6, "5 linked + 1 replay-only");
        let orphan = stored(&f, &fx.orphan, b"TWulfZ").unwrap();
        assert_eq!(orphan.origin, PlayOrigin::ReplayOnly);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn ingest_uses_core_play_id() {
        let fx = ac10();
        let f = Fixture::new(&fx.install).await;
        f.sync().await;
        let score = &fx.scores[4];
        let stored = f
            .ctx
            .user_db()
            .read(|c| play::get(c, play_id(score, b"TWulfZ")))
            .unwrap()
            .expect("play keyed by PlayId::derive over its record");
        let h = score.header();
        assert_eq!(stored.origin, PlayOrigin::ScoresDb);
        assert!(stored.snapshot_id.is_some());
        assert_eq!(stored.online_score_id, Some(4_567_890_123));
        assert_eq!(stored.client_version, h.version);
        assert_eq!(stored.score, h.score);
        assert_eq!(stored.counts.max, h.counts.geki);
        assert_eq!(stored.passed, None, "pass/fail is derived later (ADR 0014)");
        assert_eq!(
            stored.played_at,
            wolluf_store::time::parse_rfc3339_ms(&wolluf_store::time::format_rfc3339_ms(
                DotNetTicks(h.timestamp_ticks).to_unix_us()
            ))
            .unwrap()
        );
        let v2 = f
            .ctx
            .user_db()
            .read(|c| play::get(c, play_id(&fx.scores[3], b"W")))
            .unwrap()
            .unwrap();
        assert_eq!(v2.score_system, ScoreSystem::V2);
        assert_eq!(stored.score_system, ScoreSystem::V1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn sync_twice_adds_zero_rows() {
        let fx = ac10();
        let f = Fixture::new(&fx.install).await;
        f.sync().await;
        let before = (counts(&f), blob_count(&f), vault_files(&f));
        let (_, second) = f.sync().await;
        assert_eq!((counts(&f), blob_count(&f), vault_files(&f)), before);
        assert_eq!(second.plays_new, 0);
        assert_eq!(second.plays_replay_only, 0);
        assert_eq!(second.replays_linked, 0);
        assert_eq!(second.charts_archived, 0);
        // A rewritten scores.db with the same records (new sha) is still idempotent.
        let mut more = fx.scores.clone();
        more.push(ScoreBuilder::mania(&fx.chart_a, "TWulfZ", 1));
        f.write(SCORES_DB, &scores_db(&more));
        let (_, third) = f.sync().await;
        assert_eq!(third.plays_new, 0);
        assert_eq!((counts(&f), blob_count(&f), vault_files(&f)), before);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn conflicting_duplicate_records_item_failure() {
        let md5 = md5_hex(b"x");
        let a = ScoreBuilder::mania(&md5, "TWulfZ", 1);
        let b = a.clone().score(123);
        let f = Fixture::new(
            &wolluf_source_osu::testkit::FakeInstall::new().scores_db(scores_db(&[a, b])),
        )
        .await;
        let (fin, s) = f.sync().await;
        assert_eq!(counts(&f).0, 1);
        assert_eq!(s.conflicts, 1);
        assert_eq!(fin.failed_items, 1);
        let failures = failures(&f, &fin.job_id);
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].code, ErrorCode::Conflict);
        assert_eq!(failures[0].item_ref, plays(&f)[0].id.to_string());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn pre_1601_record_fails_alone() {
        let md5 = md5_hex(b"x");
        let good = ScoreBuilder::mania(&md5, "TWulfZ", 1);
        let bad = ScoreBuilder::mania(&md5, "TWulfZ", 2).ticks(DotNetTicks(5));
        let f = Fixture::new(
            &wolluf_source_osu::testkit::FakeInstall::new().scores_db(scores_db(&[good, bad])),
        )
        .await;
        let (fin, s) = f.sync().await;
        assert_eq!(s.plays_new, 1);
        assert_eq!(fin.failed_items, 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn missing_osu_db_fails_with_osu_dir_not_found() {
        let f = Fixture::new(&FakeInstall::new()).await;
        std::fs::remove_file(f.root.join(OSU_DB)).unwrap();
        let mut rx = f.ctx.subscribe();
        let id = f.ctx.jobs().submit(Box::new(SyncPlaysJob::new(f.install)));
        loop {
            if let crate::events::AppEvent::JobFinished(fin) = rx.recv().await.unwrap()
                && fin.job_id == id
            {
                assert_eq!(fin.status, crate::jobs::JobStatusDto::Failed);
                break;
            }
        }
        let job = f
            .ctx
            .jobs()
            .list(None)
            .await
            .unwrap()
            .into_iter()
            .find(|j| j.id == id)
            .unwrap();
        assert_eq!(
            job.error.map(|e| e.code),
            Some(crate::errors::ErrorCodeDto::OsuDirNotFound)
        );
    }

    fn one_score_install(score: &ScoreBuilder) -> FakeInstall {
        FakeInstall::new().scores_db(scores_db(std::slice::from_ref(score)))
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn replay_added_later_links_on_resync() {
        let score = ScoreBuilder::mania(&md5_hex(b"late"), "TWulfZ", 1);
        let f = Fixture::new(&one_score_install(&score)).await;
        let (_, first) = f.sync().await;
        assert_eq!((first.plays_new, first.replays_linked), (1, 0));

        let osr = OsrBuilder::new(score.clone()).build();
        f.write(Path::new("Data/r").join(osr_name(&score).format()), &osr);
        f.write(
            Path::new("Data/r").join(osg_name(&score).format()),
            &osg_bytes(&score),
        );
        let (_, second) = f.sync().await;
        assert_eq!(second.replays_linked, 1);
        assert_eq!(second.osg_linked, 1);
        assert_eq!(second.plays_replay_only, 0, "a linked replay is no orphan");
        let p = stored(&f, &score, b"TWulfZ").unwrap();
        assert_eq!(p.replay_sha, Some(wolluf_store::vault::sha256(&osr)));
        assert_eq!(
            p.osg_sha,
            Some(wolluf_store::vault::sha256(&osg_bytes(&score)))
        );
        assert_eq!(f.sync().await.1.replays_linked, 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn osr_header_md5_mismatch_not_linked() {
        let score = ScoreBuilder::mania(&md5_hex(b"mine"), "TWulfZ", 1);
        let other = score
            .clone()
            .beatmap_md5(OsuString::present(md5_hex(b"other").as_bytes()));
        let f = Fixture::new(
            &one_score_install(&score).replay(&osr_name(&score), OsrBuilder::new(other).build()),
        )
        .await;
        let (fin, s) = f.sync().await;
        assert_eq!(s.replays_linked, 0);
        assert_eq!(s.plays_replay_only, 0);
        assert_eq!(fin.failed_items, 1);
        let failures = failures(&f, &fin.job_id);
        assert_eq!(failures[0].code, ErrorCode::Conflict);
        assert_eq!(failures[0].item_ref, play_id(&score, b"TWulfZ").to_string());
        assert_eq!(stored(&f, &score, b"TWulfZ").unwrap().replay_sha, None);
        assert_eq!(
            blob_count(&f),
            0,
            "a refused replay is not archived as this play's"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn orphan_replay_imported_as_replay_only() {
        let chart = md5_hex(b"orphan chart");
        let orphan = ScoreBuilder::mania(&chart, "TWulfZ", 4)
            .mods(mods::SCORE_V2)
            .score(812_345)
            .online_id(4_567_890_123);
        let osr = OsrBuilder::new(orphan.clone()).build();
        let f = Fixture::new(
            &FakeInstall::new()
                .replay(&osr_name(&orphan), osr.clone())
                .replay(&osg_name(&orphan), osg_bytes(&orphan)),
        )
        .await;
        let (fin, s) = f.sync().await;
        assert_eq!(
            (s.plays_new, s.plays_replay_only, s.orphan_replays),
            (0, 1, 0)
        );
        assert_eq!(fin.failed_items, 0);
        let p = stored(&f, &orphan, b"TWulfZ").expect("id is PlayId::derive over the header");
        let h = orphan.header();
        assert_eq!(p.origin, PlayOrigin::ReplayOnly);
        assert_eq!(p.snapshot_id, None);
        assert_eq!(p.replay_sha, Some(wolluf_store::vault::sha256(&osr)));
        assert_eq!(
            p.osg_sha,
            Some(wolluf_store::vault::sha256(&osg_bytes(&orphan)))
        );
        assert_eq!(p.score, 812_345);
        assert_eq!(p.score_system, ScoreSystem::V2);
        assert_eq!(p.mods, i32::from_ne_bytes(h.mods.to_ne_bytes()));
        assert_eq!(p.counts.max, h.counts.geki);
        assert_eq!(p.counts.miss, h.counts.miss);
        assert_eq!(p.max_combo, h.max_combo);
        assert_eq!(p.online_score_id, Some(4_567_890_123));
        assert_eq!(p.client_version, h.version);
        assert_eq!(
            wolluf_store::time::format_rfc3339_ms(p.played_at),
            wolluf_store::time::format_rfc3339_ms(DotNetTicks(h.timestamp_ticks).to_unix_us())
        );
        let names: Vec<Vec<u8>> = f
            .ctx
            .user_db()
            .read(alias::list)
            .unwrap()
            .into_iter()
            .map(|a| a.raw_name)
            .collect();
        assert_eq!(names, vec![b"TWulfZ".to_vec()]);
        let (_, again) = f.sync().await;
        assert_eq!((again.plays_replay_only, again.orphan_replays), (0, 0));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn replay_only_upgraded_when_score_row_appears() {
        let score = ScoreBuilder::mania(&md5_hex(b"late row"), "TWulfZ", 2);
        let f = Fixture::new(
            &FakeInstall::new().replay(&osr_name(&score), OsrBuilder::new(score.clone()).build()),
        )
        .await;
        assert_eq!(f.sync().await.1.plays_replay_only, 1);
        let before = stored(&f, &score, b"TWulfZ").unwrap();
        assert_eq!(before.origin, PlayOrigin::ReplayOnly);

        f.write(SCORES_DB, &scores_db(std::slice::from_ref(&score)));
        let (fin, s) = f.sync().await;
        assert_eq!((s.plays_new, s.plays_replay_only, s.conflicts), (0, 0, 0));
        assert_eq!(fin.failed_items, 0);
        let after = stored(&f, &score, b"TWulfZ").unwrap();
        assert_eq!(after.origin, PlayOrigin::ScoresDb);
        assert!(after.snapshot_id.is_some());
        assert_eq!(after.replay_sha, before.replay_sha);
        assert_eq!(counts(&f).0, 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn orphan_with_name_mismatch_archived_only() {
        let fx = ac10();
        let f = Fixture::new(&FakeInstall::new().replay(
            &fx.mismatched_orphan,
            fx.install.files()[&Path::new("Data/r").join(fx.mismatched_orphan.format())].clone(),
        ))
        .await;
        let (fin, s) = f.sync().await;
        assert_eq!(s.orphan_replays, 1);
        assert_eq!(s.plays_replay_only, 0);
        assert_eq!(counts(&f).0, 0, "no play");
        assert_eq!(blob_count(&f), 1, "the bytes are kept");
        let failures = failures(&f, &fin.job_id);
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].code, ErrorCode::Conflict);
        assert_eq!(
            failures[0].item_ref,
            format!("Data/r/{}", fx.mismatched_orphan.format())
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn non_mania_orphan_skipped_not_archived() {
        let std_play = ScoreBuilder::mania(&md5_hex(b"std"), "TWulfZ", 1).mode(0);
        let f = Fixture::new(&FakeInstall::new().replay(
            &osr_name(&std_play),
            OsrBuilder::new(std_play.clone()).build(),
        ))
        .await;
        let (_, s) = f.sync().await;
        assert_eq!(
            (s.skipped_non_mania, s.orphan_replays, s.plays_replay_only),
            (1, 0, 0)
        );
        assert_eq!(blob_count(&f), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn non_mania_score_with_replay_counted_once() {
        let std_play = ScoreBuilder::mania(&md5_hex(b"std"), "TWulfZ", 1).mode(0);
        let f = Fixture::new(&one_score_install(&std_play).replay(
            &osr_name(&std_play),
            OsrBuilder::new(std_play.clone()).build(),
        ))
        .await;
        let (_, first) = f.sync().await;
        assert_eq!(first.skipped_non_mania, 1, "scores.db row and its replay");
        let (_, second) = f.sync().await;
        assert_eq!(
            second.skipped_non_mania, 1,
            "scores.db unchanged, replay seen"
        );
        assert_eq!((counts(&f).0, blob_count(&f)), (0, 0));
    }

    type Manifest = BTreeMap<PathBuf, (u64, SystemTime, BlobSha256)>;

    fn manifest(root: &Path) -> Manifest {
        fn walk(root: &Path, dir: &Path, out: &mut Manifest) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                let meta = std::fs::metadata(&p).unwrap();
                if meta.is_dir() {
                    walk(root, &p, out);
                } else {
                    let sha = wolluf_store::vault::sha256(&std::fs::read(&p).unwrap());
                    let rel = p.strip_prefix(root).unwrap().to_path_buf();
                    out.insert(rel, (meta.len(), meta.modified().unwrap(), sha));
                }
            }
        }
        let mut out = Manifest::new();
        walk(root, root, &mut out);
        out
    }

    /// Unix only: read-only makes any stray write fail loudly instead of only showing up in
    /// the manifest.
    fn set_tree_readonly(root: &Path, readonly: bool) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            const RO_DIR: u32 = 0o555;
            const RO_FILE: u32 = 0o444;
            const RW_DIR: u32 = 0o755;
            const RW_FILE: u32 = 0o644;
            fn walk(dir: &Path, readonly: bool) {
                for e in std::fs::read_dir(dir).unwrap().flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        walk(&p, readonly);
                        let mode = if readonly { RO_DIR } else { RW_DIR };
                        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode))
                            .unwrap();
                    } else {
                        let mode = if readonly { RO_FILE } else { RW_FILE };
                        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode))
                            .unwrap();
                    }
                }
            }
            // Children first when locking, the root first when unlocking, so every
            // directory stays traversable while its entries change.
            if readonly {
                walk(root, true);
                std::fs::set_permissions(root, std::fs::Permissions::from_mode(RO_DIR)).unwrap();
            } else {
                std::fs::set_permissions(root, std::fs::Permissions::from_mode(RW_DIR)).unwrap();
                walk(root, false);
            }
        }
        #[cfg(not(unix))]
        let _ = (root, readonly);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn sync_never_writes_install_root() {
        let fx = ac10();
        let f = Fixture::new(&fx.install).await;
        let before = manifest(&f.root);
        set_tree_readonly(&f.root, true);
        let (_, s) = f.sync().await;
        let (_, again) = f.sync().await;
        set_tree_readonly(&f.root, false);
        assert_eq!(s.plays_new, 6, "the sync did its work");
        assert_eq!(again.plays_new, 0);
        assert_eq!(manifest(&f.root), before);
    }

    /// Every event up to and including `until`'s `JobFinished`.
    async fn events_until(
        rx: &mut tokio::sync::broadcast::Receiver<crate::events::AppEvent>,
        until: &crate::jobs::JobId,
    ) -> Vec<crate::events::AppEvent> {
        use crate::events::AppEvent;
        let mut out = Vec::new();
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let e = rx.recv().await.unwrap();
                let done = matches!(&e, AppEvent::JobFinished(f) if &f.job_id == until);
                out.push(e);
                if done {
                    return;
                }
            }
        })
        .await
        .expect("job finished in time");
        out
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn data_changed_only_when_something_changed() {
        use crate::events::AppEvent;
        let fx = ac10();
        let f = Fixture::new(&fx.install).await;
        let mut rx = f.ctx.subscribe();
        let submit = || f.ctx.jobs().submit(Box::new(SyncPlaysJob::new(f.install)));
        let first = submit();
        events_until(&mut rx, &first).await;
        let first_change = tokio::time::timeout(Duration::from_secs(30), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first_change, AppEvent::data_changed(&["plays"]));
        let second = submit();
        let mut events = events_until(&mut rx, &second).await;
        // The runner is serial and the bus ordered: any DataChanged of the second job is sent
        // before the third starts, so it lands in this window.
        let third = submit();
        events.extend(events_until(&mut rx, &third).await);
        // The chained identity refresh reports `players` on its own; only `plays` is ours.
        assert!(
            !events.iter().any(
                |e| matches!(e, AppEvent::DataChanged(d) if d.domains.iter().any(|d| d == "plays"))
            ),
            "an unchanged install emits no DataChanged{{plays}}: {events:?}"
        );

        let mut more = fx.scores.clone();
        more.push(ScoreBuilder::mania(&fx.chart_a, "TWulfZ", 10));
        f.write(SCORES_DB, &scores_db(&more));
        let fourth = submit();
        events_until(&mut rx, &fourth).await;
        let next = tokio::time::timeout(Duration::from_secs(30), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(next, AppEvent::data_changed(&["plays"]));
    }
}
