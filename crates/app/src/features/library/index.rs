//! `IndexLibrary` (F1): parses every catalog chart of a keymode with an engine profile into
//! `chart_parsed`, writes its difficulty-name labels into `chart_label` and its pattern segments
//! into `segment`. Each chart is memoized per md5 and stage in `derivation` (architecture §7),
//! so a rerun only does what is new and a cancel means "run it again".

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::mpsc;

use wolluf_core::{ChartMd5, ErrorCode, Keymode, StageId, VersionKey};
use wolluf_engine::EngineError;
use wolluf_engine::labels::{LabelInput, extract_labels};
use wolluf_engine::profile::Registry;
use wolluf_engine::rows_blob::{decode_rows, encode_rows};
use wolluf_engine::stage::chart_parse::parse_chart;
use wolluf_engine::stage::patterns::{self, Chart, Segment, Segmenter};
use wolluf_engine::stage::{chart_label, chart_parse};
use wolluf_source_osu::songs::{ChartReadError, read_chart_verified};
use wolluf_store::repo::cache::{
    CatalogChart, ChartLabel, ChartParsed, Derivation, DerivationStatus, SegmentRow, catalog_chart,
    chart_label as label_repo, chart_parsed, derivation, segment as segment_repo,
};
use wolluf_store::repo::ledger::{GameInstall, SnapshotKind, game_install, play, source_snapshot};
use wolluf_store::{Conn, DbHandle, StoreError};

use crate::context::{blocking_join_error, songs_dir};
use crate::errors::AppError;
use crate::jobs::dto::{IndexLibrarySummaryDto, JobKindDto, JobStageDto, JobSummaryDto};
use crate::jobs::{ItemError, ItemResult, Job, JobCtx, JobFuture, JobSummary, panic_text};

const DOMAIN_LIBRARY: &str = "library";
/// Architecture §7: writer transactions of ~500–5k rows.
const WRITE_BATCH: usize = 500;
/// Bounds the parsed blobs waiting for the writer, so memory stays flat over 18k charts.
const WRITE_QUEUE: usize = 2 * WRITE_BATCH;

#[derive(Debug, Default)]
pub struct IndexLibraryJob;

impl IndexLibraryJob {
    pub const DEDUPE_KEY: &'static str = "library.index";
}

impl Job for IndexLibraryJob {
    fn kind(&self) -> JobKindDto {
        JobKindDto::IndexLibrary
    }

    fn dedupe_key(&self) -> String {
        Self::DEDUPE_KEY.to_owned()
    }

    fn params(&self) -> serde_json::Value {
        serde_json::json!({})
    }

    fn run(self: Box<Self>, ctx: JobCtx) -> JobFuture {
        Box::pin(async move {
            tokio::task::spawn_blocking(move || index(&ctx))
                .await
                .map_err(blocking_join_error)?
        })
    }
}

/// The stage-level keys every library row is stored and read under (D15).
#[derive(Debug, Clone, Copy)]
pub(super) struct Keys {
    pub(super) parse: VersionKey,
    pub(super) label: VersionKey,
}

impl Keys {
    pub(super) fn current() -> Result<Self, AppError> {
        let key = |r: Result<VersionKey, EngineError>| {
            r.map_err(|e| AppError::internal(format!("library vkey: {e}")))
        };
        Ok(Self {
            parse: key(chart_parse::vkey())?,
            label: key(chart_label::vkey())?,
        })
    }
}

/// The `patterns` stage per keymode profile. The layout is the profile's default; a
/// user-selected layout is future work.
pub(super) struct Segmenters(Vec<(u8, Segmenter)>);

impl Segmenters {
    pub(super) fn current() -> Result<Self, AppError> {
        Registry::builtin()
            .profiles()
            .iter()
            .map(|p| {
                let segmenter = Segmenter::new(p.layout())
                    .map_err(|e| AppError::internal(format!("patterns vkey: {e}")))?;
                Ok((p.keymode.columns(), segmenter))
            })
            .collect::<Result<_, AppError>>()
            .map(Self)
    }

    pub(super) fn get(&self, keymode: u8) -> Option<&Segmenter> {
        self.0.iter().find(|(k, _)| *k == keymode).map(|(_, s)| s)
    }

    fn vkeys(&self) -> Vec<VersionKey> {
        self.0.iter().map(|(_, s)| s.vkey()).collect()
    }
}

pub(super) fn segment_row(s: Segment) -> SegmentRow {
    SegmentRow {
        t0: s.t0,
        t1: s.t1,
        cols: s.cols.bits(),
        axis: s.axis,
        pattern: s.primary,
        secondary: s.secondary,
        purity: s.purity,
        strength: s.strength,
    }
}

/// The install whose osu!.db the catalog reflects: catalog paths are relative to its Songs
/// dir. `None` while there is no catalog; `NOT_FOUND` when no install's latest osu!.db built it.
pub(super) fn catalog_install(
    user: &DbHandle,
    cache: &DbHandle,
) -> Result<Option<GameInstall>, AppError> {
    let Some(snapshot) = cache.read(catalog_chart::snapshot_id)? else {
        return Ok(None);
    };
    for install in user.read(game_install::list)? {
        let latest = user.read(|c| source_snapshot::latest(c, install.id, SnapshotKind::OsuDb))?;
        if latest.is_some_and(|s| s.id == snapshot) {
            return Ok(Some(install));
        }
    }
    Err(AppError::not_found().with_arg("snapshotId", snapshot.0.to_string()))
}

/// osu!.db builds the catalog path as `<folder>/<file>`.
/// The set folder that holds the chart, even under nested Songs folders (`Normal/<set>/x.osu`).
pub(super) fn folder_of(path: &str) -> &str {
    let parent = path.rsplit_once('/').map_or("", |(folder, _)| folder);
    parent.rsplit_once('/').map_or(parent, |(_, last)| last)
}

/// Architecture §7: played charts first. Stable, so the catalog order holds within each group.
fn played_first(charts: &mut [CatalogChart], played: &BTreeSet<ChartMd5>) {
    charts.sort_by_key(|c| !played.contains(&c.md5));
}

struct Item {
    chart: CatalogChart,
    played: bool,
    labels: bool,
    parse: bool,
    segments: bool,
}

/// `(items with work, charts whose parse is memoized)`. A skip is no memo hit: the file may
/// come back.
fn plan(
    conn: Conn<'_>,
    charts: Vec<CatalogChart>,
    played: &BTreeSet<ChartMd5>,
    keys: Keys,
    segmenters: &Segmenters,
) -> Result<(Vec<Item>, u32), StoreError> {
    let registry = Registry::builtin();
    let mut items = Vec::new();
    let mut memoized = 0_u32;
    for chart in charts {
        let label_sources = Keymode::new(chart.keymode)
            .ok()
            .and_then(|k| registry.profile(k))
            .is_some_and(|p| p.label_sources);
        let key = chart.md5.to_string();
        let parse_memo = derivation::get(conn, &chart_parse::STAGE, &key, keys.parse)?;
        let parse_done = parse_memo
            .as_ref()
            .is_some_and(|d| d.status != DerivationStatus::Skipped);
        let parse_failed = parse_memo
            .as_ref()
            .is_some_and(|d| d.status == DerivationStatus::Failed);
        let labels_due = label_sources
            && derivation::get(conn, &chart_label::STAGE, &key, keys.label)?.is_none();
        // A skip is no memo hit here either: the chart may be parsed later.
        let segments_due = !parse_failed
            && match segmenters.get(chart.keymode) {
                Some(s) => derivation::get(conn, &patterns::STAGE, &key, s.vkey())?
                    .is_none_or(|d| d.status == DerivationStatus::Skipped),
                None => false,
            };
        memoized += u32::from(parse_done);
        if !parse_done || labels_due || segments_due {
            items.push(Item {
                played: played.contains(&chart.md5),
                chart,
                labels: labels_due,
                parse: !parse_done,
                segments: segments_due,
            });
        }
    }
    Ok((items, memoized))
}

/// One chart's rows, committed together by the writer.
struct ChartWrite {
    md5: ChartMd5,
    parsed: Option<ChartParsed>,
    labels: Option<Vec<ChartLabel>>,
    segments: Option<(VersionKey, Vec<SegmentRow>)>,
    derivations: Vec<Derivation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Parsed,
    Unavailable,
    LabelsOnly,
}

enum Unparsed {
    /// Missing or edited in place: skipped with its reason.
    Unavailable(ChartReadError),
    /// Deterministic for these bytes, so memoized as failed.
    Failed(ItemError),
    /// An IO error that may clear: failed now, retried next run.
    Transient(ItemError),
}

fn engine_error(e: &EngineError) -> ItemError {
    let code = match e {
        // A pattern error is the decoded file disagreeing with the catalog's keymode: a fact of
        // these bytes, memoized like a parse failure, not a bad request or a bug.
        EngineError::Chart(_) | EngineError::Patterns(_) => ErrorCode::ParseFailed,
        _ => ErrorCode::Internal,
    };
    ItemError::new(code, e.to_string())
}

fn memo(stage: StageId, key: &str, vkey: VersionKey, status: DerivationStatus) -> Derivation {
    Derivation {
        stage,
        input_key: key.to_owned(),
        vkey,
        status,
        error_code: None,
        error_msg: None,
        duration_ms: None,
    }
}

/// The stored row and the decoded chart it was encoded from.
fn parse(
    songs: &Path,
    chart: &CatalogChart,
    vkey: VersionKey,
) -> Result<(ChartParsed, Chart), Unparsed> {
    let bytes =
        read_chart_verified(songs, Path::new(&chart.path), chart.md5).map_err(|e| match e {
            ChartReadError::Missing | ChartReadError::Md5Mismatch { .. } => {
                Unparsed::Unavailable(e)
            }
            e @ ChartReadError::Io { .. } => {
                Unparsed::Transient(ItemError::new(e.code(), e.to_string()))
            }
        })?;
    let parsed = parse_chart(&bytes).map_err(|e| Unparsed::Failed(engine_error(&e)))?;
    let rows_blob = encode_rows(&parsed.chart).map_err(|e| Unparsed::Failed(engine_error(&e)))?;
    let s = parsed.summary;
    let row = ChartParsed {
        md5: chart.md5,
        vkey,
        rows_blob,
        n_notes: s.n_notes,
        n_ln: s.n_ln,
        ln_ratio: s.ln_ratio,
        length_ms: s.length_ms,
    };
    Ok((row, parsed.chart))
}

fn labels_of(chart: &CatalogChart) -> Vec<ChartLabel> {
    let input = LabelInput {
        folder: folder_of(&chart.path),
        version: &chart.version,
        creator: &chart.creator,
        set_id: chart.set_id,
    };
    extract_labels(&input)
        .into_iter()
        .map(|l| ChartLabel {
            source: l.source,
            scale: l.scale,
            level_ord: l.level_ord,
            level_text: l.level_text,
            skill_tag: l.skill_tag,
            is_variant: l.is_variant,
        })
        .collect()
}

/// What the item works on.
struct Env<'a> {
    songs: &'a Path,
    keys: Keys,
    segmenters: &'a Segmenters,
    cache: &'a DbHandle,
}

/// The stored parse of an item whose parse memo is not a skip or failure. A missing row means
/// the cache lost it behind its memo, which the item reports instead of hiding.
fn stored_chart(env: &Env<'_>, md5: ChartMd5) -> Result<Chart, ItemError> {
    let internal = |e: String| ItemError::new(ErrorCode::Internal, e);
    let parsed = env
        .cache
        .read(|c| chart_parsed::get(c, md5, env.keys.parse))
        .map_err(|e| internal(e.to_string()))?
        .ok_or_else(|| internal(format!("{md5} has a parse memo but no chart_parsed row")))?;
    decode_rows(&parsed.rows_blob).map_err(|e| internal(format!("rows blob of {md5}: {e}")))
}

fn index_item(
    env: &Env<'_>,
    item: &Item,
    writes: &mpsc::SyncSender<ChartWrite>,
) -> Result<Outcome, ItemError> {
    let keys = env.keys;
    let md5 = item.chart.md5;
    let key = md5.to_string();
    let mut write = ChartWrite {
        md5,
        parsed: None,
        labels: None,
        segments: None,
        derivations: Vec::new(),
    };
    let mut fresh = None;
    if item.labels {
        write.labels = Some(labels_of(&item.chart));
        let ok = memo(chart_label::STAGE, &key, keys.label, DerivationStatus::Ok);
        write.derivations.push(ok);
    }
    let mut outcome = Ok(Outcome::LabelsOnly);
    if item.parse {
        let stage = chart_parse::STAGE;
        match parse(env.songs, &item.chart, keys.parse) {
            Ok((parsed, chart)) => {
                write.parsed = Some(parsed);
                fresh = Some(chart);
                let ok = memo(stage, &key, keys.parse, DerivationStatus::Ok);
                write.derivations.push(ok);
                outcome = Ok(Outcome::Parsed);
            }
            Err(Unparsed::Unavailable(e)) => {
                write.derivations.push(Derivation {
                    error_code: Some(e.code()),
                    error_msg: Some(e.to_string()),
                    ..memo(stage, &key, keys.parse, DerivationStatus::Skipped)
                });
                outcome = Ok(Outcome::Unavailable);
            }
            Err(Unparsed::Failed(e)) => {
                write.derivations.push(Derivation {
                    error_code: Some(e.code),
                    error_msg: Some(e.message.clone()),
                    ..memo(stage, &key, keys.parse, DerivationStatus::Failed)
                });
                outcome = Err(e);
            }
            Err(Unparsed::Transient(e)) => outcome = Err(e),
        }
    }
    let segmenter = env.segmenters.get(item.chart.keymode);
    if let (true, Some(segmenter), true) = (item.segments, segmenter, outcome.is_ok()) {
        // No early return: the item's other rows and memos must still reach the writer.
        let chart = match fresh {
            Some(chart) => Some(chart),
            None if !item.parse => match stored_chart(env, md5) {
                Ok(chart) => Some(chart),
                Err(e) => {
                    outcome = Err(e);
                    None
                }
            },
            None => None,
        };
        if let Some(chart) = chart {
            let stage = patterns::STAGE;
            let vkey = segmenter.vkey();
            match segmenter.run(&chart) {
                Ok(segments) => {
                    let rows = segments.into_iter().map(segment_row).collect();
                    write.segments = Some((vkey, rows));
                    write
                        .derivations
                        .push(memo(stage, &key, vkey, DerivationStatus::Ok));
                }
                Err(e) => {
                    let e = engine_error(&e);
                    write.derivations.push(Derivation {
                        error_code: Some(e.code),
                        error_msg: Some(e.message.clone()),
                        ..memo(stage, &key, vkey, DerivationStatus::Failed)
                    });
                    outcome = Err(e);
                }
            }
        }
    }
    // A closed queue means the writer failed; the job ends with that error instead of one
    // failure per item.
    let _ = writes.send(write);
    outcome
}

#[derive(Debug, Default)]
struct Written {
    parsed: u32,
    labels: u32,
    segments: u32,
}

fn write_batch(
    cache: &DbHandle,
    keys: Keys,
    batch: Vec<ChartWrite>,
    written: &mut Written,
) -> Result<(), AppError> {
    if batch.is_empty() {
        return Ok(());
    }
    let (parsed, labels, segments) = cache.write(move |tx| {
        let (mut parsed, mut labels, mut segments) = (0_u32, 0_u32, 0_u32);
        for w in &batch {
            if let Some(p) = &w.parsed {
                chart_parsed::put(tx, p)?;
                parsed += 1;
            }
            if let Some(l) = &w.labels {
                label_repo::replace_for(tx, w.md5, keys.label, l)?;
                labels += u32::try_from(l.len()).unwrap_or(u32::MAX);
            }
            if let Some((vkey, rows)) = &w.segments {
                segment_repo::replace_for(tx, w.md5, *vkey, rows)?;
                segments += u32::try_from(rows.len()).unwrap_or(u32::MAX);
            }
            for d in &w.derivations {
                derivation::put(tx, d)?;
            }
        }
        Ok((parsed, labels, segments))
    })?;
    written.parsed += parsed;
    written.labels += labels;
    written.segments += segments;
    Ok(())
}

fn write_loop(
    cache: &DbHandle,
    keys: Keys,
    writes: mpsc::Receiver<ChartWrite>,
) -> Result<Written, AppError> {
    let mut written = Written::default();
    let mut batch = Vec::with_capacity(WRITE_BATCH);
    for w in writes {
        batch.push(w);
        if batch.len() >= WRITE_BATCH {
            write_batch(cache, keys, std::mem::take(&mut batch), &mut written)?;
        }
    }
    write_batch(cache, keys, batch, &mut written)?;
    Ok(written)
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

fn index(ctx: &JobCtx) -> Result<JobSummary, AppError> {
    let keys = Keys::current()?;
    let segmenters = Segmenters::current()?;
    let mut summary = IndexLibrarySummaryDto::default();
    let Some(install) = catalog_install(&ctx.user, &ctx.cache)? else {
        ctx.progress.report(JobStageDto::Index, 0, 0);
        return Ok(finish(summary));
    };
    let mut charts = Vec::new();
    for profile in Registry::builtin().profiles() {
        let keymode = profile.keymode.columns();
        charts.extend(
            ctx.cache
                .read(|c| catalog_chart::list_by_keymode(c, keymode))?,
        );
    }
    summary.charts_total = count(charts.len());
    let played: BTreeSet<ChartMd5> = ctx
        .user
        .read(play::keys_by_chart_and_time)?
        .into_iter()
        .map(|(md5, _)| md5)
        .collect();
    played_first(&mut charts, &played);
    let (items, memoized) = ctx
        .cache
        .read(|c| plan(c, charts, &played, keys, &segmenters))?;
    summary.skipped_memoized = memoized;

    let songs = songs_dir(&install.root_path);
    let env = Env {
        songs: &songs,
        keys,
        segmenters: &segmenters,
        cache: &ctx.cache,
    };
    // rayon splits a slice in halves, so one pass would not honour the order: played charts
    // get a pass of their own.
    let split = items.partition_point(|i| i.played);
    let (writes, queue) = mpsc::sync_channel::<ChartWrite>(WRITE_QUEUE);
    let (results, written) = std::thread::scope(move |s| {
        let writer = s.spawn(move || write_loop(&ctx.cache, keys, queue));
        let mut results = Vec::with_capacity(items.len());
        let mut outcome = Ok(());
        for phase in [&items[..split], &items[split..]] {
            match ctx.run_items(
                JobStageDto::Index,
                phase,
                |i| i.chart.md5.to_string(),
                |i| index_item(&env, i, &writes),
            ) {
                Ok(r) => results.extend(r),
                Err(e) => {
                    outcome = Err(e);
                    break;
                }
            }
        }
        drop(writes);
        let written = writer.join().unwrap_or_else(|p| {
            Err(AppError::internal(format!(
                "library writer panicked: {}",
                panic_text(&*p)
            )))
        });
        (outcome.map(|()| results), written)
    });
    let written = written?;
    let results = results?;
    summary.skipped_unavailable = count(
        results
            .iter()
            .filter(|r| matches!(r, ItemResult::Done(Outcome::Unavailable)))
            .count(),
    );
    summary.parsed_new = written.parsed;
    summary.labels_written = written.labels;
    summary.segments_written = written.segments;
    ctx.check_cancelled()?;

    let segment_keys = segmenters.vkeys();
    let pruned = ctx.cache.write(move |tx| {
        Ok(chart_parsed::prune_except(tx, &[keys.parse])?
            + label_repo::prune_except(tx, &[keys.label])?
            + segment_repo::prune_except(tx, &segment_keys)?)
    })?;
    summary.failed_items = ctx.failed_items();
    let changed = written.parsed + written.labels + written.segments > 0 || pruned > 0;
    Ok(JobSummary {
        changed: if changed {
            vec![DOMAIN_LIBRARY]
        } else {
            Vec::new()
        },
        ..finish(summary)
    })
}

fn finish(summary: IndexLibrarySummaryDto) -> JobSummary {
    JobSummary {
        summary: Some(JobSummaryDto::IndexLibrary(summary)),
        changed: Vec::new(),
        follow_ups: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use wolluf_core::{ChartMd5, ErrorCode, TimeUs, VersionKey};
    use wolluf_engine::stage::{chart_label, chart_parse, patterns};
    use wolluf_store::repo::cache::{
        CatalogChart, ChartParsed, DerivationStatus, SegmentRow, catalog_chart,
        chart_label as label_repo, chart_parsed, derivation, item_failure, segment as segment_repo,
    };

    use super::*;
    use crate::features::library::testkit::{Map, osu_text, reindex, synced};
    use crate::features::plays::testkit::Fixture;
    use crate::jobs::JobKindDto;

    const REGULAR_DAN: &str = "1 7K Dan Course - Regular Dan Phase";

    fn md5(m: &Map) -> ChartMd5 {
        m.md5.parse().unwrap()
    }

    fn parse_row(f: &Fixture, m: &Map) -> Option<wolluf_store::repo::cache::Derivation> {
        let vkey = chart_parse::vkey().unwrap();
        f.ctx
            .cache_db()
            .read(|c| derivation::get(c, &chart_parse::STAGE, &m.md5, vkey))
            .unwrap()
    }

    fn parsed_count(f: &Fixture) -> u64 {
        let vkey = chart_parse::vkey().unwrap();
        f.ctx
            .cache_db()
            .read(|c| chart_parsed::count(c, vkey))
            .unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn first_run_parses_every_enabled_chart_and_second_run_none() {
        let maps = [
            Map::k7("alpha"),
            Map::k7("beta"),
            Map::k7("gamma"),
            Map::new("four keys", 4, osu_text(4, "four keys", &[(0, 0)], &[])),
        ];
        let (f, first) = synced(&maps, &[]).await;
        assert_eq!(first.charts_total, 3, "4K has no engine profile");
        assert_eq!(first.parsed_new, 3);
        assert_eq!((first.skipped_memoized, first.failed_items), (0, 0));
        assert_eq!(parsed_count(&f), 3);
        let vkey = chart_parse::vkey().unwrap();
        let row = f
            .ctx
            .cache_db()
            .read(|c| chart_parsed::get(c, md5(&maps[0]), vkey))
            .unwrap()
            .unwrap();
        assert_eq!((row.n_notes, row.n_ln, row.length_ms), (8, 0, 1_750));
        let d = parse_row(&f, &maps[0]).unwrap();
        assert_eq!(d.status, DerivationStatus::Ok);
        assert!(parse_row(&f, &maps[3]).is_none());

        let (_, second) = reindex(&f).await;
        assert_eq!(second.charts_total, 3);
        assert_eq!((second.parsed_new, second.labels_written), (0, 0));
        assert_eq!(second.skipped_memoized, 3);
        assert_eq!(parsed_count(&f), 3);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn edited_and_missing_charts_are_skipped_with_reason_and_retried() {
        let ok = Map::k7("kept");
        let edited = Map::k7("edited").edited();
        let missing = Map::k7("missing").missing();
        let (f, first) = synced(&[ok.clone(), edited.clone(), missing.clone()], &[]).await;
        assert_eq!(first.parsed_new, 1);
        assert_eq!(first.skipped_unavailable, 2);
        assert_eq!(first.failed_items, 0);
        let edited_row = parse_row(&f, &edited).unwrap();
        assert_eq!(edited_row.status, DerivationStatus::Skipped);
        assert_eq!(edited_row.error_code, Some(ErrorCode::Conflict));
        let missing_row = parse_row(&f, &missing).unwrap();
        assert_eq!(missing_row.status, DerivationStatus::Skipped);
        assert_eq!(missing_row.error_code, Some(ErrorCode::NotFound));

        // The file may come back, so a skip is not a memo hit.
        f.write(
            format!("Songs/{}", missing.rel_path()),
            &Map::k7("missing").bytes.unwrap(),
        );
        let (_, second) = reindex(&f).await;
        assert_eq!(second.parsed_new, 1, "the restored chart is parsed");
        assert_eq!(second.skipped_unavailable, 1);
        assert_eq!(second.skipped_memoized, 1);
        assert_eq!(
            parse_row(&f, &missing).unwrap().status,
            DerivationStatus::Ok
        );
    }

    /// Pilot corpus: 432 charts under `Songs/Normal/` were reported missing off Windows.
    #[tokio::test(flavor = "multi_thread")]
    async fn charts_in_nested_songs_folders_are_parsed() {
        let nested = Map::k7("nested").nested_in("Normal");
        let (f, first) = synced(std::slice::from_ref(&nested), &[]).await;
        assert_eq!((first.parsed_new, first.skipped_unavailable), (1, 0));
        assert_eq!(parse_row(&f, &nested).unwrap().status, DerivationStatus::Ok);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn parse_failure_is_recorded_and_memoized() {
        let text = String::from_utf8(osu_text(7, "std", &[(0, 0)], &[]))
            .unwrap()
            .replace("Mode: 3", "Mode: 0");
        let broken = Map::new("std", 7, text.into_bytes());
        let (f, first) = synced(&[broken.clone(), Map::k7("fine")], &[]).await;
        assert_eq!((first.parsed_new, first.failed_items), (1, 1));
        let job = crate::features::library::testkit::last_index_job(&f).await;
        let ulid = ulid::Ulid::from_string(&job.id.0).unwrap();
        let failures = f
            .ctx
            .cache_db()
            .read(|c| item_failure::list(c, ulid))
            .unwrap();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].item_ref, broken.md5);
        assert_eq!(failures[0].code, ErrorCode::ParseFailed);
        let row = parse_row(&f, &broken).unwrap();
        assert_eq!(row.status, DerivationStatus::Failed);
        assert_eq!(row.error_code, Some(ErrorCode::ParseFailed));

        let (_, second) = reindex(&f).await;
        assert_eq!((second.parsed_new, second.failed_items), (0, 0));
        assert_eq!(
            second.skipped_memoized, 2,
            "a failure is memoized like a success"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn labels_come_from_catalog_names_even_without_the_file() {
        let dan = Map::k7("dan four").named(REGULAR_DAN, "4th Dan");
        let lost = Map::k7("dan seven").named(REGULAR_DAN, "7th Dan").missing();
        let plain = Map::k7("plain");
        let (f, first) = synced(&[dan.clone(), lost.clone(), plain.clone()], &[]).await;
        assert_eq!(first.labels_written, 2);
        let vkey = chart_label::vkey().unwrap();
        let labels = |m: &Map| {
            f.ctx
                .cache_db()
                .read(|c| label_repo::list_for(c, md5(m), vkey))
                .unwrap()
        };
        let four = labels(&dan);
        assert_eq!(four.len(), 1);
        assert_eq!(four[0].source, "jinjin_dan_regular");
        assert_eq!(four[0].level_ord, Some(4.0));
        assert_eq!(labels(&lost).len(), 1);
        assert!(labels(&plain).is_empty());
        let (_, second) = reindex(&f).await;
        assert_eq!(second.labels_written, 0, "labels are memoized per chart");
    }

    fn patterns_key() -> VersionKey {
        Segmenters::current().unwrap().get(7).unwrap().vkey()
    }

    fn segments_of(f: &Fixture, m: &Map) -> Vec<SegmentRow> {
        let vkey = patterns_key();
        f.ctx
            .cache_db()
            .read(|c| segment_repo::list_for(c, md5(m), vkey))
            .unwrap()
    }

    fn patterns_row(f: &Fixture, m: &Map) -> Option<wolluf_store::repo::cache::Derivation> {
        let vkey = patterns_key();
        f.ctx
            .cache_db()
            .read(|c| derivation::get(c, &patterns::STAGE, &m.md5, vkey))
            .unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn segments_are_written_per_chart_and_memoized() {
        let jacks = Map::jacks("jacks");
        let plain = Map::k7("plain");
        let (f, first) = synced(&[jacks.clone(), plain.clone()], &[]).await;
        assert_eq!(first.segments_written, 1);
        let segments = segments_of(&f, &jacks);
        assert_eq!(segments.len(), 1, "{segments:?}");
        assert_eq!(segments[0].pattern.as_str(), "regular.jack.longjack");
        assert_eq!(segments[0].axis.as_str(), "7k.regular.jack");
        assert_eq!(
            (segments[0].t0, segments[0].t1),
            (TimeUs::from_ms(1_000), TimeUs::from_ms(1_500))
        );
        assert!(segments_of(&f, &plain).is_empty());
        // A chart without segments is still memoized.
        assert_eq!(
            patterns_row(&f, &plain).unwrap().status,
            DerivationStatus::Ok
        );

        let (_, second) = reindex(&f).await;
        assert_eq!(second.segments_written, 0);
        assert_eq!(segments_of(&f, &jacks).len(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_new_patterns_key_segments_already_parsed_charts() {
        let jacks = Map::jacks("jacks");
        let (f, _) = synced(std::slice::from_ref(&jacks), &[]).await;
        let rows = f.ctx.cache_db().read(derivation::list_all).unwrap();
        let vkey = patterns_key();
        f.ctx
            .cache_db()
            .write(move |tx| {
                segment_repo::prune_except(tx, &[])?;
                for d in rows.iter().filter(|d| d.vkey == vkey) {
                    derivation::put(
                        tx,
                        &wolluf_store::repo::cache::Derivation {
                            status: DerivationStatus::Skipped,
                            ..d.clone()
                        },
                    )?;
                }
                Ok(())
            })
            .unwrap();
        let (_, rerun) = reindex(&f).await;
        assert_eq!((rerun.parsed_new, rerun.segments_written), (0, 1));
        assert_eq!(segments_of(&f, &jacks).len(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn charts_without_a_parse_get_no_segments() {
        let text = String::from_utf8(osu_text(7, "std", &[(0, 0)], &[]))
            .unwrap()
            .replace("Mode: 3", "Mode: 0");
        let broken = Map::new("std", 7, text.into_bytes());
        let missing = Map::jacks("missing").missing();
        let (f, first) = synced(&[broken.clone(), missing.clone()], &[]).await;
        assert_eq!(first.segments_written, 0);
        assert!(patterns_row(&f, &broken).is_none());
        assert!(patterns_row(&f, &missing).is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_parse_memo_without_its_row_is_an_item_failure() {
        let jacks = Map::jacks("jacks");
        let (f, _) = synced(std::slice::from_ref(&jacks), &[]).await;
        let rows = f.ctx.cache_db().read(derivation::list_all).unwrap();
        let vkey = patterns_key();
        f.ctx
            .cache_db()
            .write(move |tx| {
                chart_parsed::prune_except(tx, &[])?;
                for d in rows.iter().filter(|d| d.vkey == vkey) {
                    derivation::put(
                        tx,
                        &wolluf_store::repo::cache::Derivation {
                            status: DerivationStatus::Skipped,
                            ..d.clone()
                        },
                    )?;
                }
                Ok(())
            })
            .unwrap();
        let (job, rerun) = reindex(&f).await;
        assert_eq!(rerun.failed_items, 1);
        let ulid = ulid::Ulid::from_string(&job.id.0).unwrap();
        let failures = f
            .ctx
            .cache_db()
            .read(|c| item_failure::list(c, ulid))
            .unwrap();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].item_ref, jacks.md5);
        assert_eq!(failures[0].code, ErrorCode::Internal);
    }

    /// osu!.db files it as 7K, the file says 4K: the 7K layout cannot segment it.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_file_of_another_keymode_than_the_catalog_fails_segmenting() {
        let liar = Map::new("liar", 7, osu_text(4, "liar", &[(0, 0), (1, 100)], &[]));
        let (f, first) = synced(std::slice::from_ref(&liar), &[]).await;
        assert_eq!((first.parsed_new, first.failed_items), (1, 1));
        let memo = patterns_row(&f, &liar).unwrap();
        assert_eq!(memo.status, DerivationStatus::Failed);
        assert_eq!(memo.error_code, Some(ErrorCode::ParseFailed));
        let (_, rerun) = reindex(&f).await;
        assert_eq!(rerun.failed_items, 0, "memoized like a parse failure");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn segments_under_other_keys_are_pruned() {
        let jacks = Map::jacks("jacks");
        let (f, _) = synced(std::slice::from_ref(&jacks), &[]).await;
        let stale = VersionKey([7; 32]);
        let row = segments_of(&f, &jacks);
        let md5 = md5(&jacks);
        f.ctx
            .cache_db()
            .write(move |tx| segment_repo::replace_for(tx, md5, stale, &row))
            .unwrap();
        reindex(&f).await;
        let (old, current) = f
            .ctx
            .cache_db()
            .read(|c| {
                Ok((
                    segment_repo::count_charts(c, stale)?,
                    segment_repo::count_charts(c, patterns_key())?,
                ))
            })
            .unwrap();
        assert_eq!((old, current), (0, 1));
    }

    #[test]
    fn played_charts_come_first_in_catalog_order() {
        let chart = |hex: &str| CatalogChart {
            md5: hex.repeat(32).parse().unwrap(),
            keymode: 7,
            title: String::new(),
            artist: String::new(),
            version: String::new(),
            creator: String::new(),
            set_id: None,
            beatmap_id: None,
            path: String::new(),
            od: 8.0,
            hp: 8.0,
            length_ms: 0,
        };
        let mut charts = vec![chart("1"), chart("2"), chart("3"), chart("4")];
        let played: BTreeSet<ChartMd5> = [charts[1].md5, charts[3].md5].into();
        played_first(&mut charts, &played);
        let order: Vec<String> = charts
            .iter()
            .map(|c| c.md5.to_string()[..1].to_owned())
            .collect();
        assert_eq!(order, ["2", "4", "1", "3"]);
    }

    #[test]
    fn folder_is_the_parent_of_the_catalog_path() {
        assert_eq!(folder_of("123 a - b/a - b [x].osu"), "123 a - b");
        assert_eq!(folder_of("loose.osu"), "");
        // labels.py keys folder rules ([_BMS_] prefix, KomeijiDove set id) on the set folder alone.
        assert_eq!(folder_of("Normal/[_BMS_] song/x.osu"), "[_BMS_] song");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancelled_run_writes_nothing_and_the_rerun_completes() {
        let maps = [Map::k7("one"), Map::k7("two")];
        let f = Fixture::new(&crate::features::library::testkit::install(&maps, &[])).await;
        f.sync().await;
        f.ctx
            .cache_db()
            .write(|tx| {
                chart_parsed::prune_except(tx, &[])?;
                label_repo::prune_except(tx, &[])
            })
            .unwrap();
        let fresh = |f: &Fixture| {
            let vkey = chart_parse::vkey().unwrap();
            let rows = f.ctx.cache_db().read(derivation::list_all).unwrap();
            // Forget the memo, so the next run has work again.
            f.ctx
                .cache_db()
                .write(move |tx| {
                    for d in rows.iter().filter(|d| d.vkey == vkey) {
                        derivation::put(
                            tx,
                            &wolluf_store::repo::cache::Derivation {
                                status: DerivationStatus::Skipped,
                                ..d.clone()
                            },
                        )?;
                    }
                    Ok(())
                })
                .unwrap();
        };
        fresh(&f);

        let ctx = f.ctx.jobs().test_ctx(JobKindDto::IndexLibrary);
        ctx.cancel.cancel();
        let err = Box::new(IndexLibraryJob).run(ctx).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Cancelled);
        assert_eq!(parsed_count(&f), 0);

        let (_, rerun) = reindex(&f).await;
        assert_eq!(rerun.parsed_new, 2);
        assert_eq!(parsed_count(&f), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn rows_under_other_keys_are_pruned() {
        let map = Map::k7("kept");
        let (f, _) = synced(std::slice::from_ref(&map), &[]).await;
        let stale = VersionKey([7; 32]);
        let row = ChartParsed {
            md5: md5(&map),
            vkey: stale,
            rows_blob: vec![1],
            n_notes: 1,
            n_ln: 0,
            ln_ratio: 0.0,
            length_ms: 0,
        };
        f.ctx
            .cache_db()
            .write(move |tx| chart_parsed::put(tx, &row))
            .unwrap();
        reindex(&f).await;
        let (old, current) = f
            .ctx
            .cache_db()
            .read(|c| {
                Ok((
                    chart_parsed::count(c, stale)?,
                    chart_parsed::count(c, chart_parse::vkey().unwrap())?,
                ))
            })
            .unwrap();
        assert_eq!((old, current), (0, 1));
        assert_eq!(
            f.ctx
                .cache_db()
                .read(catalog_chart::list_all)
                .unwrap()
                .len(),
            1
        );
    }
}
