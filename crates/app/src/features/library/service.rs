//! `LibraryService`: the library feature's only entry point for shells and other features
//! (D12). Every read goes through the current stage keys (D15).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use tokio::sync::broadcast::error::RecvError;
use wolluf_core::{ChartMd5, ErrorCode, Keymode, TimeUs};
use wolluf_engine::profile::Registry;
use wolluf_engine::render::{RenderOpts, RowMark, render_window};
use wolluf_engine::rows_blob::decode_rows;
use wolluf_engine::stage::chart_parse::parse_chart;
use wolluf_engine::taxonomy;
use wolluf_source_osu::songs::read_chart_verified;
use wolluf_store::repo::cache::{
    CatalogChart, ChartLabel, LabelFilter, ParsedSummary, SegmentRow, catalog_chart,
    chart_label as label_repo, chart_parsed, segment as segment_repo,
};
use wolluf_store::{Conn, DbHandle, StoreError};

use super::dto::{
    ChartDetailDto, ChartLabelDto, LibraryChartDto, LibraryFilterDto, PatternCountDto,
    ScaleCountDto, SegmentDto,
};
use super::index::{IndexLibraryJob, Keys, Segmenters, catalog_install};
use crate::context::{AppContext, blocking_join_error, songs_dir};
use crate::errors::AppError;
use crate::events::AppEvent;
use crate::jobs::dto::{JobDto, JobId};

const MS_PER_SECOND: f64 = 1_000.0;
const US_PER_SECOND: f64 = 1_000_000.0;
/// Shown for a pattern id the keymode's taxonomy does not know.
const UNKNOWN_KEY: &str = "?";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChartRows {
    pub keymode: Keymode,
    pub times: Vec<TimeUs>,
}

pub struct LibraryService<'a> {
    ctx: &'a AppContext,
}

#[derive(Clone)]
struct Dbs {
    user: DbHandle,
    cache: DbHandle,
}

impl<'a> LibraryService<'a> {
    pub fn new(ctx: &'a AppContext) -> Self {
        Self { ctx }
    }

    async fn blocking<R: Send + 'static>(
        &self,
        f: impl FnOnce(Dbs, Keys) -> Result<R, AppError> + Send + 'static,
    ) -> Result<R, AppError> {
        let dbs = Dbs {
            user: self.ctx.user_db().clone(),
            cache: self.ctx.cache_db().clone(),
        };
        let keys = Keys::current()?;
        tokio::task::spawn_blocking(move || f(dbs, keys))
            .await
            .map_err(blocking_join_error)?
    }

    /// Returns at once with the job id; a queued index is reused.
    pub async fn index(&self) -> Result<JobId, AppError> {
        Ok(self.ctx.jobs().submit(Box::new(IndexLibraryJob)))
    }

    /// Runs one index to its end and returns its history entry, whatever its status.
    pub async fn index_and_wait(&self) -> Result<JobDto, AppError> {
        // Subscribed before submitting, so the finish event cannot slip past.
        let mut rx = self.ctx.subscribe();
        let id = self.index().await?;
        loop {
            match rx.recv().await {
                Ok(AppEvent::JobFinished(f)) if f.job_id == id => break,
                // A lagging receiver only lost progress; the history below is authoritative.
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => {
                    return Err(AppError::internal("event bus closed during index"));
                }
            }
        }
        self.ctx
            .jobs()
            .list(None)
            .await?
            .into_iter()
            .find(|j| j.id == id)
            .ok_or_else(|| AppError::internal(format!("job {id} missing from history")))
    }

    /// Indexed charts only: a catalog chart without a parse (no file, a failure, or not yet
    /// indexed) is not listed.
    pub async fn list(&self, filter: LibraryFilterDto) -> Result<Vec<LibraryChartDto>, AppError> {
        let keymode = Keymode::new(filter.keymode).map_err(|_| {
            AppError::invalid_input().with_arg("keymode", filter.keymode.to_string())
        })?;
        self.blocking(move |dbs, keys| Ok(dbs.cache.read(|c| list(c, keys, keymode, &filter))?))
            .await
    }

    pub async fn get(&self, md5: &str) -> Result<ChartDetailDto, AppError> {
        let md5 = parse_md5(md5)?;
        self.blocking(move |dbs, keys| get(&dbs, keys, md5)).await
    }

    /// `from_ms <= t < to_ms`. `layout_id` defaults to the keymode profile's layout. With
    /// `segments`, each row names the primary pattern of the segment covering it (by taxonomy
    /// key, `*` on the segment's first row) and a legend follows, naming the layout the segments
    /// were computed with (the profile's default), or `segments: none`.
    pub async fn render(
        &self,
        md5: &str,
        from_ms: i32,
        to_ms: i32,
        layout_id: Option<&str>,
        segments: bool,
    ) -> Result<String, AppError> {
        let md5 = parse_md5(md5)?;
        if from_ms >= to_ms {
            return Err(AppError::invalid_input()
                .with_arg("fromMs", from_ms.to_string())
                .with_arg("toMs", to_ms.to_string()));
        }
        let layout_id = layout_id.map(str::to_owned);
        self.blocking(move |dbs, keys| {
            let parsed = dbs
                .cache
                .read(|c| chart_parsed::get(c, md5, keys.parse))?
                .ok_or_else(|| not_found(md5))?;
            let chart = decode_rows(&parsed.rows_blob)
                .map_err(|e| AppError::internal(format!("rows blob of {md5}: {e}")))?;
            let profile = Registry::builtin()
                .profile(chart.keymode())
                .ok_or_else(|| not_found(md5))?;
            let layout = match layout_id {
                None => profile.layout(),
                Some(id) => profile
                    .layout_by_id(&id)
                    .ok_or_else(|| AppError::invalid_input().with_arg("layoutId", id))?,
            };
            let (from, to) = (TimeUs::from_ms(from_ms), TimeUs::from_ms(to_ms));
            let segmenters = Segmenters::current()?;
            let keymode = chart.keymode().columns();
            let rows = if segments {
                dbs.cache
                    .read(|c| segment_rows(c, &segmenters, md5, keymode))?
            } else {
                Vec::new()
            };
            let taxonomy = profile.taxonomy;
            let marks = rows
                .iter()
                .map(|r| RowMark {
                    t0: r.t0,
                    t1: r.t1,
                    label: key_of(taxonomy, r.pattern.as_str()).to_owned(),
                })
                .collect();
            let opts = RenderOpts {
                marks,
                ..RenderOpts::default()
            };
            let mut text = render_window(&chart, &layout, from, to, &opts);
            if segments {
                let layout_id = segmenters.get(keymode).map_or("-", |s| s.layout_id());
                text.push_str(&legend(taxonomy, layout_id, &rows, from, to));
            }
            Ok(text)
        })
        .await
    }

    /// The chart's segments under its profile's default layout, in time order; `NOT_FOUND`
    /// unless the chart is parsed.
    pub async fn segments(&self, md5: &str) -> Result<Vec<SegmentDto>, AppError> {
        let md5 = parse_md5(md5)?;
        self.blocking(move |dbs, keys| {
            let (chart, parsed) = dbs.cache.read(|c| {
                Ok((
                    catalog_chart::get(c, md5)?,
                    chart_parsed::exists(c, md5, keys.parse)?,
                ))
            })?;
            let chart = chart.filter(|_| parsed).ok_or_else(|| not_found(md5))?;
            let segmenters = Segmenters::current()?;
            Ok(dbs
                .cache
                .read(|c| segment_dtos(c, &segmenters, md5, chart.keymode))?)
        })
        .await
    }

    /// Primary segments per pattern over the library, per keymode profile, in pattern id order.
    pub async fn pattern_counts(&self) -> Result<Vec<PatternCountDto>, AppError> {
        self.blocking(|dbs, _keys| {
            let segmenters = Segmenters::current()?;
            let mut out = Vec::new();
            for profile in Registry::builtin().profiles() {
                let keymode = profile.keymode.columns();
                let Some(segmenter) = segmenters.get(keymode) else {
                    continue;
                };
                let vkey = segmenter.vkey();
                let counts = dbs
                    .cache
                    .read(|c| segment_repo::counts_by_pattern(c, vkey))?;
                out.extend(counts.into_iter().map(|(pattern, n)| {
                    let def = taxonomy::by_id(profile.taxonomy, pattern.as_str());
                    PatternCountDto {
                        keymode,
                        key: def.map_or(UNKNOWN_KEY, |d| d.key).to_owned(),
                        axis_id: def.map_or_else(String::new, |d| d.axis.to_string()),
                        pattern_id: pattern.to_string(),
                        segments: saturating(n.segments),
                        charts: saturating(n.charts),
                        total_s: n.total_us as f64 / US_PER_SECOND,
                    }
                }));
            }
            Ok(out)
        })
        .await
    }

    /// Every parsed chart of a keymode with its labels, in md5 order: `list` without paging,
    /// read from the stored summaries so no blob is loaded.
    pub async fn overview(&self, keymode: Keymode) -> Result<Vec<LibraryChartDto>, AppError> {
        self.blocking(move |dbs, keys| Ok(dbs.cache.read(|c| overview(c, keys, keymode))?))
            .await
    }

    /// The chart's keymode and the ascending times of its rows (one per distinct event time).
    pub async fn row_times(&self, md5: ChartMd5) -> Result<ChartRows, AppError> {
        self.blocking(move |dbs, keys| {
            let parsed = dbs
                .cache
                .read(|c| chart_parsed::get(c, md5, keys.parse))?
                .ok_or_else(|| not_found(md5))?;
            let chart = decode_rows(&parsed.rows_blob)
                .map_err(|e| AppError::internal(format!("rows blob of {md5}: {e}")))?;
            Ok(ChartRows {
                keymode: chart.keymode(),
                times: chart.rows().iter().map(|r| r.t).collect(),
            })
        })
        .await
    }

    /// Every label row, files or not: labels come from osu!.db names alone.
    pub async fn counts_by_scale(&self) -> Result<Vec<ScaleCountDto>, AppError> {
        self.blocking(|dbs, keys| {
            let counts = dbs
                .cache
                .read(|c| label_repo::counts_by_scale(c, keys.label))?;
            Ok(counts
                .into_iter()
                .map(|(scale, n)| ScaleCountDto {
                    scale,
                    rows: saturating(n.rows),
                    charts: saturating(n.charts),
                })
                .collect())
        })
        .await
    }
}

fn key_of(taxonomy: &[taxonomy::PatternDef], pattern: &str) -> &'static str {
    taxonomy::by_id(taxonomy, pattern).map_or(UNKNOWN_KEY, |d| d.key)
}

/// `keymode`'s segments of the chart; none for a keymode without a patterns stage.
fn segment_rows(
    c: Conn<'_>,
    segmenters: &Segmenters,
    md5: ChartMd5,
    keymode: u8,
) -> Result<Vec<SegmentRow>, StoreError> {
    match segmenters.get(keymode) {
        Some(s) => segment_repo::list_for(c, md5, s.vkey()),
        None => Ok(Vec::new()),
    }
}

fn segment_dtos(
    c: Conn<'_>,
    segmenters: &Segmenters,
    md5: ChartMd5,
    keymode: u8,
) -> Result<Vec<SegmentDto>, StoreError> {
    let taxonomy = Keymode::new(keymode)
        .ok()
        .and_then(|k| Registry::builtin().profile(k))
        .map_or(&[][..], |p| p.taxonomy);
    let rows = segment_rows(c, segmenters, md5, keymode)?;
    Ok(rows
        .into_iter()
        .map(|r| SegmentDto {
            t0_ms: ms_i32(r.t0),
            t1_ms: ms_i32(r.t1),
            cols: r.cols,
            key: key_of(taxonomy, r.pattern.as_str()).to_owned(),
            pattern_id: r.pattern.to_string(),
            axis_id: r.axis.to_string(),
            secondary: r.secondary.iter().map(ToString::to_string).collect(),
            purity: r.purity,
            strength: r.strength,
        })
        .collect())
}

fn ms_i32(t: TimeUs) -> i32 {
    let ms = t.as_ms_floor();
    i32::try_from(ms).unwrap_or(if ms < 0 { i32::MIN } else { i32::MAX })
}

/// The patterns drawn in `[from, to)`, one `key  pattern id` line each, in key order, under
/// the layout the segments were computed with (it may differ from the one drawn).
fn legend(
    taxonomy: &[taxonomy::PatternDef],
    layout_id: &str,
    rows: &[SegmentRow],
    from: TimeUs,
    to: TimeUs,
) -> String {
    let shown: BTreeSet<(&str, &str)> = rows
        .iter()
        .filter(|r| r.t0 < to && r.t1 >= from)
        .map(|r| (key_of(taxonomy, r.pattern.as_str()), r.pattern.as_str()))
        .collect();
    if shown.is_empty() {
        return "segments: none\n".to_owned();
    }
    let mut out = format!("segments ({layout_id}, * first row):\n");
    for (key, pattern) in shown {
        out.push_str(&format!("  {key:<3} {pattern}\n"));
    }
    out
}

fn saturating(n: u64) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

fn parse_md5(md5: &str) -> Result<ChartMd5, AppError> {
    md5.parse()
        .map_err(|_| AppError::invalid_input().with_arg("md5", md5))
}

fn not_found(md5: ChartMd5) -> AppError {
    AppError::not_found().with_arg("md5", md5.to_string())
}

fn label_dto(l: ChartLabel) -> ChartLabelDto {
    ChartLabelDto {
        source: l.source,
        scale: l.scale,
        level_ord: l.level_ord,
        level_text: l.level_text,
        skill_tag: l.skill_tag,
        is_variant: l.is_variant,
    }
}

/// From the stored columns, so a listing never decodes a blob. `length_ms` is floored, so the
/// last digits may differ from the engine's summary.
fn nps(p: &ParsedSummary) -> f64 {
    if p.length_ms == 0 {
        0.0
    } else {
        f64::from(p.n_notes) * MS_PER_SECOND / f64::from(p.length_ms)
    }
}

fn chart_dto(
    chart: &CatalogChart,
    parsed: &ParsedSummary,
    labels: Vec<ChartLabel>,
) -> LibraryChartDto {
    LibraryChartDto {
        md5: chart.md5.to_string(),
        title: chart.title.clone(),
        artist: chart.artist.clone(),
        version: chart.version.clone(),
        creator: chart.creator.clone(),
        keymode: chart.keymode,
        n_notes: parsed.n_notes,
        n_ln: parsed.n_ln,
        ln_ratio: parsed.ln_ratio,
        length_ms: parsed.length_ms,
        nps: nps(parsed),
        labels: labels.into_iter().map(label_dto).collect(),
    }
}

fn matches_text(chart: &CatalogChart, needle: &str) -> bool {
    [&chart.title, &chart.artist, &chart.version, &chart.creator]
        .iter()
        .any(|field| field.to_lowercase().contains(needle))
}

fn list(
    conn: Conn<'_>,
    keys: Keys,
    keymode: Keymode,
    filter: &LibraryFilterDto,
) -> Result<Vec<LibraryChartDto>, StoreError> {
    let catalog: BTreeMap<ChartMd5, CatalogChart> =
        catalog_chart::list_by_keymode(conn, keymode.columns())?
            .into_iter()
            .map(|c| (c.md5, c))
            .collect();
    let by_label = filter.scale.is_some()
        || filter.level_min.is_some()
        || filter.level_max.is_some()
        || filter.label_source.is_some();
    let candidates: Vec<ChartMd5> = if by_label {
        let label_filter = LabelFilter {
            scale: filter.scale.clone(),
            level_min: filter.level_min,
            level_max: filter.level_max,
        };
        let mut seen = BTreeSet::new();
        label_repo::list_filtered(conn, keys.label, &label_filter, u32::MAX)?
            .into_iter()
            .filter(|(_, l)| filter.label_source.as_ref().is_none_or(|s| &l.source == s))
            .map(|(md5, _)| md5)
            .filter(|md5| seen.insert(*md5))
            .collect()
    } else {
        catalog.keys().copied().collect()
    };
    let needle = filter
        .text
        .as_deref()
        .map(str::to_lowercase)
        .filter(|t| !t.is_empty());
    let limit = usize::try_from(filter.limit).unwrap_or(usize::MAX);
    let mut skipped = 0_u32;
    let mut page = Vec::new();
    for md5 in candidates {
        if page.len() >= limit {
            break;
        }
        let Some(chart) = catalog.get(&md5) else {
            continue;
        };
        if needle.as_deref().is_some_and(|n| !matches_text(chart, n)) {
            continue;
        }
        if skipped < filter.offset {
            skipped += u32::from(chart_parsed::exists(conn, md5, keys.parse)?);
            continue;
        }
        let Some(parsed) = chart_parsed::get(conn, md5, keys.parse)? else {
            continue;
        };
        let labels = label_repo::list_for(conn, md5, keys.label)?;
        page.push(chart_dto(chart, &parsed.summary(), labels));
    }
    Ok(page)
}

fn overview(
    conn: Conn<'_>,
    keys: Keys,
    keymode: Keymode,
) -> Result<Vec<LibraryChartDto>, StoreError> {
    let catalog: BTreeMap<ChartMd5, CatalogChart> =
        catalog_chart::list_by_keymode(conn, keymode.columns())?
            .into_iter()
            .map(|c| (c.md5, c))
            .collect();
    let mut labels: BTreeMap<ChartMd5, Vec<ChartLabel>> = BTreeMap::new();
    for (md5, label) in
        label_repo::list_filtered(conn, keys.label, &LabelFilter::default(), u32::MAX)?
    {
        labels.entry(md5).or_default().push(label);
    }
    Ok(chart_parsed::summaries(conn, keys.parse)?
        .into_iter()
        .filter_map(|p| {
            let chart = catalog.get(&p.md5)?;
            Some(chart_dto(
                chart,
                &p,
                labels.remove(&p.md5).unwrap_or_default(),
            ))
        })
        .collect())
}

fn get(dbs: &Dbs, keys: Keys, md5: ChartMd5) -> Result<ChartDetailDto, AppError> {
    let (chart, parsed, labels) = dbs.cache.read(|c| {
        Ok((
            catalog_chart::get(c, md5)?,
            chart_parsed::get(c, md5, keys.parse)?,
            label_repo::list_for(c, md5, keys.label)?,
        ))
    })?;
    let (Some(chart), Some(parsed)) = (chart, parsed) else {
        return Err(not_found(md5));
    };
    let segmenters = Segmenters::current()?;
    let segments = dbs
        .cache
        .read(|c| segment_dtos(c, &segmenters, md5, chart.keymode))?;
    Ok(ChartDetailDto {
        segments,
        diagnostics: diagnostics(dbs, &chart)?,
        chart: chart_dto(&chart, &parsed.summary(), labels),
    })
}

/// Re-decoded from the file on demand: the store keeps no diagnostics, and one chart decodes in
/// milliseconds.
fn diagnostics(dbs: &Dbs, chart: &CatalogChart) -> Result<Option<u32>, AppError> {
    let install = match catalog_install(&dbs.user, &dbs.cache) {
        Ok(Some(install)) => install,
        Ok(None) => return Ok(None),
        Err(e) if e.code == ErrorCode::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let songs = songs_dir(&install.root_path);
    let Ok(bytes) = read_chart_verified(&songs, Path::new(&chart.path), chart.md5) else {
        return Ok(None);
    };
    Ok(parse_chart(&bytes)
        .ok()
        .map(|p| saturating(p.diagnostics.iter().count() as u64)))
}

#[cfg(test)]
mod tests {
    use wolluf_core::ErrorCode;

    use super::*;
    use crate::events::AppEvent;
    use crate::features::library::dto::{
        LibraryFilterDto, PatternCountDto, ScaleCountDto, SegmentDto,
    };
    use crate::features::library::testkit::{Map, osu_text, synced};
    use crate::jobs::dto::{JobKindDto, JobStartDto, JobStatusDto};

    const REGULAR_DAN: &str = "1 7K Dan Course - Regular Dan Phase";
    const WILD_DAN: &str = "2 Wild 7K Dan Course";

    fn filter() -> LibraryFilterDto {
        LibraryFilterDto {
            keymode: 7,
            scale: None,
            level_min: None,
            level_max: None,
            label_source: None,
            text: None,
            limit: 50,
            offset: 0,
        }
    }

    struct Library {
        four: Map,
        seven: Map,
        wild: Map,
        plain: Map,
        lost: Map,
    }

    fn library() -> Library {
        Library {
            four: Map::k7("four").named(REGULAR_DAN, "4th Dan"),
            seven: Map::k7("seven").named(REGULAR_DAN, "7th Dan"),
            wild: Map::k7("wild").named(WILD_DAN, "3rd Dan"),
            plain: Map::k7("Plain Song"),
            lost: Map::k7("lost").named(REGULAR_DAN, "9th Dan").missing(),
        }
    }

    impl Library {
        fn maps(&self) -> Vec<Map> {
            vec![
                self.four.clone(),
                self.seven.clone(),
                self.wild.clone(),
                self.plain.clone(),
                self.lost.clone(),
                Map::new("four keys", 4, osu_text(4, "four keys", &[(0, 0)], &[])),
            ]
        }
    }

    fn md5s(list: &[LibraryChartDto]) -> Vec<&str> {
        list.iter().map(|c| c.md5.as_str()).collect()
    }

    fn sorted(maps: &[&Map]) -> Vec<String> {
        let mut out: Vec<String> = maps.iter().map(|m| m.md5.clone()).collect();
        out.sort();
        out
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn list_filters_by_label_text_and_pages() {
        let lib = library();
        let (f, _) = synced(&lib.maps(), &[]).await;
        let svc = f.ctx.library();

        let all = svc.list(filter()).await.unwrap();
        assert_eq!(
            md5s(&all),
            sorted(&[&lib.four, &lib.seven, &lib.wild, &lib.plain]),
            "parsed charts only, in md5 order"
        );
        let four = all.iter().find(|c| c.md5 == lib.four.md5).unwrap();
        assert_eq!((four.keymode, four.n_notes, four.n_ln), (7, 8, 0));
        assert_eq!((four.length_ms, four.ln_ratio), (1_750, 0.0));
        assert!((four.nps - 8.0 / 1.75).abs() < 1e-9, "{}", four.nps);
        assert_eq!(four.version, "4th Dan");
        assert_eq!(four.labels.len(), 1);
        assert_eq!(four.labels[0].scale, "jinjin_dan_regular");

        let regular = svc
            .list(LibraryFilterDto {
                scale: Some("jinjin_dan_regular".into()),
                ..filter()
            })
            .await
            .unwrap();
        assert_eq!(md5s(&regular), [&lib.four.md5, &lib.seven.md5], "by level");
        let hard = svc
            .list(LibraryFilterDto {
                scale: Some("jinjin_dan_regular".into()),
                level_min: Some(5.0),
                ..filter()
            })
            .await
            .unwrap();
        assert_eq!(md5s(&hard), [&lib.seven.md5]);
        let wild = svc
            .list(LibraryFilterDto {
                label_source: Some("wild_dan".into()),
                ..filter()
            })
            .await
            .unwrap();
        assert_eq!(md5s(&wild), [&lib.wild.md5]);
        let text = svc
            .list(LibraryFilterDto {
                text: Some("plain SONG".into()),
                ..filter()
            })
            .await
            .unwrap();
        assert_eq!(md5s(&text), [&lib.plain.md5]);
        let page = svc
            .list(LibraryFilterDto {
                limit: 2,
                offset: 1,
                ..filter()
            })
            .await
            .unwrap();
        assert_eq!(md5s(&page), md5s(&all)[1..3]);

        let four_keys = svc
            .list(LibraryFilterDto {
                keymode: 4,
                ..filter()
            })
            .await
            .unwrap();
        assert!(four_keys.is_empty(), "4K is not indexed");
        let err = svc
            .list(LibraryFilterDto {
                keymode: 0,
                ..filter()
            })
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn counts_by_scale_include_labelled_charts_without_a_file() {
        let lib = library();
        let (f, _) = synced(&lib.maps(), &[]).await;
        let counts = f.ctx.library().counts_by_scale().await.unwrap();
        assert_eq!(
            counts,
            [
                ScaleCountDto {
                    scale: "jinjin_dan_regular".into(),
                    rows: 3,
                    charts: 3,
                },
                ScaleCountDto {
                    scale: "wild_dan".into(),
                    rows: 1,
                    charts: 1,
                },
            ]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn get_returns_summary_labels_and_diagnostics() {
        let twice = osu_text(7, "dup", &[(1, 100), (1, 100), (2, 400)], &[(3, 100, 900)]);
        let dup = Map::new("dup", 7, twice);
        let lib = library();
        let (f, _) = synced(&[dup.clone(), lib.four.clone()], &[]).await;
        let svc = f.ctx.library();

        let detail = svc.get(&dup.md5).await.unwrap();
        assert_eq!(detail.chart.md5, dup.md5);
        assert_eq!((detail.chart.n_notes, detail.chart.n_ln), (3, 1));
        assert_eq!(detail.diagnostics, Some(1), "the duplicate note");
        let four = svc.get(&lib.four.md5).await.unwrap();
        assert_eq!(four.diagnostics, Some(0));
        assert_eq!(four.chart.labels.len(), 1);

        let unknown = "0".repeat(32);
        assert_eq!(
            svc.get(&unknown).await.unwrap_err().code,
            ErrorCode::NotFound
        );
        assert_eq!(
            svc.get("not an md5").await.unwrap_err().code,
            ErrorCode::InvalidInput
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn render_draws_the_window_in_the_default_layout() {
        let chart = osu_text(7, "render", &[(0, 1_000), (6, 1_250)], &[(3, 1_000, 1_500)]);
        let map = Map::new("render", 7, chart);
        let (f, _) = synced(std::slice::from_ref(&map), &[]).await;
        let svc = f.ctx.library();

        let text = svc.render(&map.md5, 0, 2_000, None, false).await.unwrap();
        assert!(text.contains("00:01.000"), "{text}");
        assert!(text.contains("00:01.250"), "{text}");
        assert!(text.contains('H') && text.contains('T'), "{text}");
        let named = svc
            .render(&map.md5, 0, 2_000, Some("k7.313_right_thumb"), false)
            .await
            .unwrap();
        assert_eq!(named, text);
        let left_thumb = svc
            .render(&map.md5, 0, 2_000, Some("k7.313_left_thumb"), false)
            .await
            .unwrap();
        assert!(left_thumb.contains("k7.313_left_thumb"), "{left_thumb}");
        let wrong_keymode = svc
            .render(&map.md5, 0, 2_000, Some("k4.generic"), false)
            .await;
        assert_eq!(wrong_keymode.unwrap_err().code, ErrorCode::InvalidInput);
        let window = svc
            .render(&map.md5, 1_100, 2_000, None, false)
            .await
            .unwrap();
        assert!(!window.contains("00:01.000"), "{window}");

        let bad_layout = svc.render(&map.md5, 0, 2_000, Some("k7.nope"), false).await;
        assert_eq!(bad_layout.unwrap_err().code, ErrorCode::InvalidInput);
        let empty_window = svc.render(&map.md5, 2_000, 2_000, None, false).await;
        assert_eq!(empty_window.unwrap_err().code, ErrorCode::InvalidInput);
        let unknown = svc.render(&"0".repeat(32), 0, 1, None, false).await;
        assert_eq!(unknown.unwrap_err().code, ErrorCode::NotFound);
    }

    fn longjack() -> SegmentDto {
        SegmentDto {
            t0_ms: 1_000,
            t1_ms: 1_500,
            cols: 1,
            pattern_id: "regular.jack.longjack".into(),
            key: "lj".into(),
            axis_id: "7k.regular.jack".into(),
            secondary: Vec::new(),
            purity: 1_000,
            strength: 1_000,
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn segments_and_pattern_counts() {
        let jacks = Map::jacks("jacks");
        let plain = Map::k7("plain");
        let (f, _) = synced(&[jacks.clone(), plain.clone()], &[]).await;
        let svc = f.ctx.library();
        assert_eq!(svc.segments(&jacks.md5).await.unwrap(), [longjack()]);
        assert_eq!(svc.segments(&plain.md5).await.unwrap(), []);
        assert_eq!(
            svc.segments(&"0".repeat(32)).await.unwrap_err().code,
            ErrorCode::NotFound
        );
        assert_eq!(svc.get(&jacks.md5).await.unwrap().segments, [longjack()]);
        assert_eq!(
            svc.pattern_counts().await.unwrap(),
            [PatternCountDto {
                keymode: 7,
                pattern_id: "regular.jack.longjack".into(),
                key: "lj".into(),
                axis_id: "7k.regular.jack".into(),
                segments: 1,
                charts: 1,
                total_s: 0.5,
            }]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn render_marks_segment_rows_and_adds_a_legend() {
        let jacks = Map::jacks("jacks");
        let (f, _) = synced(std::slice::from_ref(&jacks), &[]).await;
        let svc = f.ctx.library();
        let marked = svc.render(&jacks.md5, 0, 3_000, None, true).await.unwrap();
        let lines: Vec<&str> = marked.lines().collect();
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("00:01.000") && l.ends_with("* lj")),
            "{marked}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("00:01.500") && l.ends_with("| lj")),
            "{marked}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.trim() == "lj  regular.jack.longjack"),
            "{marked}"
        );
        assert!(
            marked.contains(
                "segments (k7.313_right_thumb, * first row):\n  lj  regular.jack.longjack\n"
            ),
            "{marked}"
        );
        // Another display layout still names the layout the segments were computed under.
        let left = svc
            .render(&jacks.md5, 0, 3_000, Some("k7.313_left_thumb"), true)
            .await
            .unwrap();
        assert!(
            left.contains("segments (k7.313_right_thumb, * first row):"),
            "{left}"
        );
        let empty = svc
            .render(&jacks.md5, 2_000, 3_000, None, true)
            .await
            .unwrap();
        assert!(empty.ends_with("segments: none\n"), "{empty}");
        let plain = svc.render(&jacks.md5, 0, 3_000, None, false).await.unwrap();
        assert!(!plain.contains("lj"), "{plain}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn overview_lists_every_parsed_chart_with_labels() {
        let lib = library();
        let (f, _) = synced(&lib.maps(), &[]).await;
        let svc = f.ctx.library();
        let overview = svc.overview(Keymode::K7).await.unwrap();
        let listed = svc
            .list(LibraryFilterDto {
                limit: u32::MAX,
                ..filter()
            })
            .await
            .unwrap();
        assert_eq!(overview, listed, "same rows as an unbounded listing");
        assert!(svc.overview(Keymode::K4).await.unwrap().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn row_times_are_the_chart_rows() {
        let chart = osu_text(7, "rows", &[(0, 1_000), (6, 1_250)], &[(3, 1_000, 1_500)]);
        let map = Map::new("rows", 7, chart);
        let (f, _) = synced(std::slice::from_ref(&map), &[]).await;
        let md5: ChartMd5 = map.md5.parse().unwrap();
        let rows = f.ctx.library().row_times(md5).await.unwrap();
        assert_eq!(rows.keymode, Keymode::K7);
        assert_eq!(
            rows.times,
            [
                TimeUs::from_ms(1_000),
                TimeUs::from_ms(1_250),
                TimeUs::from_ms(1_500)
            ]
        );
        let unknown = f.ctx.library().row_times(ChartMd5([0; 16])).await;
        assert_eq!(unknown.unwrap_err().code, ErrorCode::NotFound);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn job_service_starts_index_library() {
        let (f, _) = synced(&[Map::k7("solo")], &[]).await;
        let mut rx = f.ctx.subscribe();
        let id = f
            .ctx
            .job_service()
            .start(JobStartDto::IndexLibrary)
            .await
            .unwrap();
        loop {
            if let AppEvent::JobFinished(fin) = rx.recv().await.unwrap()
                && fin.job_id == id
            {
                assert_eq!(fin.status, JobStatusDto::Ok);
                break;
            }
        }
        let job = f.ctx.job_service().list(None).await.unwrap();
        let job = job.iter().find(|j| j.id == id).unwrap();
        assert_eq!(job.kind, JobKindDto::IndexLibrary);
    }
}
