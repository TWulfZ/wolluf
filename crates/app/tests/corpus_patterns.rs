#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! F1 acceptance for the pattern engine: every parsed 7K chart of the pilot is segmented with no
//! failure, the per-pattern coverage table is printed, and a second index segments nothing.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use wolluf_app::clock::SystemClock;
use wolluf_app::context::{AppContext, AppPaths};
use wolluf_app::jobs::JobStatusDto;
use wolluf_app::jobs::dto::{IndexLibrarySummaryDto, JobDto, JobKindDto, JobSummaryDto};
use wolluf_core::{Clock, Keymode};
use wolluf_engine::profile::Registry;
use wolluf_engine::stage::chart_parse;
use wolluf_engine::stage::patterns::{self, Segmenter};
use wolluf_store::open_cache_db;
use wolluf_store::repo::cache::{DerivationStatus, chart_parsed, derivation, segment};
use wolluf_store::time::parse_rfc3339_ms;

const MS_PER_SECOND: f64 = 1_000.0;

fn index_summary(job: &JobDto) -> IndexLibrarySummaryDto {
    assert_eq!(job.status, JobStatusDto::Ok, "{job:?}");
    let Some(JobSummaryDto::IndexLibrary(summary)) = &job.summary else {
        panic!("index finished without a summary: {job:?}");
    };
    summary.clone()
}

fn job_duration(job: &JobDto) -> Duration {
    let at = |t: &Option<String>| parse_rfc3339_ms(t.as_deref().unwrap()).unwrap().0;
    Duration::from_micros(u64::try_from(at(&job.ended) - at(&job.started)).unwrap())
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "corpus: needs WOLLUF_CORPUS"]
async fn corpus_patterns_segment_every_parsed_chart() {
    let root = common::corpus();
    let tree_before = common::tree_state(&root);

    let data = tempfile::tempdir().unwrap();
    let paths = AppPaths::from_data_dir(data.path().join("data"));
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let ctx = AppContext::open(paths.clone(), clock).unwrap();
    let install = ctx.register_install(root.clone(), None).await.unwrap();
    let sync = ctx.plays().sync_and_wait(install).await.unwrap();
    assert_eq!(sync.status, JobStatusDto::Ok, "{sync:?}");
    let wait = Instant::now();
    ctx.jobs().wait_idle().await;
    let follow_ups = wait.elapsed();
    let jobs = ctx.jobs().list(None).await.unwrap();
    let first_job = jobs
        .iter()
        .find(|j| j.kind == JobKindDto::IndexLibrary)
        .expect("SyncPlays chains an index");
    let first = index_summary(first_job);
    let first_time = job_duration(first_job);

    let second_job = ctx.library().index_and_wait().await.unwrap();
    let second = index_summary(&second_job);
    let second_time = job_duration(&second_job);
    let counts = ctx.library().pattern_counts().await.unwrap();
    drop(ctx);

    let profile = Registry::builtin().profile(Keymode::K7).unwrap();
    let key = Segmenter::new(profile.layout()).unwrap().vkey();
    let parse_key = chart_parse::vkey().unwrap();
    let cache = open_cache_db(&paths.cache_db()).unwrap();
    let parsed = cache
        .read(|c| chart_parsed::summaries(c, parse_key))
        .unwrap();
    let memo: Vec<_> = cache
        .read(derivation::list_all)
        .unwrap()
        .into_iter()
        .filter(|d| d.stage == patterns::STAGE && d.vkey == key)
        .collect();
    let segmented_charts = cache.read(|c| segment::count_charts(c, key)).unwrap();
    drop(cache);

    let ok = memo
        .iter()
        .filter(|d| d.status == DerivationStatus::Ok)
        .count();
    let failed: Vec<_> = memo
        .iter()
        .filter(|d| d.status == DerivationStatus::Failed)
        .collect();
    let chart_ms: f64 = parsed.iter().map(|p| f64::from(p.length_ms)).sum();
    let segment_ms: f64 = counts.iter().map(|c| c.total_s * MS_PER_SECOND).sum();
    let segments: u64 = counts.iter().map(|c| u64::from(c.segments)).sum();

    println!(
        "corpus_patterns: {} parsed 7K charts, {ok} segmented ok, {} failed, {segmented_charts} \
         with at least one segment, {segments} segments",
        parsed.len(),
        failed.len()
    );
    println!(
        "first index {first_time:?} (sync follow-ups {follow_ups:?}): {first:?}\n\
         second index {second_time:?}: {second:?}"
    );
    println!(
        "segmented time: {:.1} of {:.1} chart hours ({:.1}%)",
        segment_ms / MS_PER_SECOND / 3_600.0,
        chart_ms / MS_PER_SECOND / 3_600.0,
        100.0 * segment_ms / chart_ms.max(1.0)
    );
    println!("per pattern (segments / charts / seconds / share of segmented time):");
    for c in &counts {
        println!(
            "  {:<3} {:<34} {:>7} {:>6} {:>10.1} {:>6.2}%",
            c.key,
            c.pattern_id,
            c.segments,
            c.charts,
            c.total_s,
            100.0 * c.total_s * MS_PER_SECOND / segment_ms.max(1.0)
        );
    }

    assert!(failed.is_empty(), "segmentation failures: {failed:?}");
    assert_eq!(ok, parsed.len(), "every parsed chart has a patterns memo");
    assert_eq!(second.segments_written, 0, "the second index is memoized");
    assert_eq!(second.parsed_new, 0);
    assert_eq!(
        u64::from(first.segments_written),
        segments,
        "the first index wrote every stored segment"
    );
    common::assert_unchanged("two indexes", &tree_before, &common::tree_state(&root));
}
