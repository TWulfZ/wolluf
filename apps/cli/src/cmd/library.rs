//! `wolluf library index|list|scales|patterns` over `LibraryService` and `IndexLibrary` (F1).

use std::future::Future;
use std::process::ExitCode;

use wolluf_app::context::AppContext;
use wolluf_app::errors::AppError;
use wolluf_app::features::library::dto::{
    ChartLabelDto, LibraryChartDto, LibraryFilterDto, PatternCountDto, ScaleCountDto,
};
use wolluf_app::jobs::JobStatusDto;
use wolluf_app::jobs::dto::{JobDto, JobStartDto, JobSummaryDto};

use crate::cli::{LibraryCmd, LibraryListArgs};
use crate::exit;
use crate::follow;
use crate::progress::Progress;
use crate::render;

const PERCENT: f64 = 100.0;
const MS_PER_SECOND: u32 = 1_000;
const SECONDS_PER_MINUTE: u32 = 60;

pub(crate) async fn run(ctx: &AppContext, cmd: LibraryCmd, json: bool) -> anyhow::Result<ExitCode> {
    match cmd {
        LibraryCmd::Index => return index(ctx, json).await,
        LibraryCmd::List(args) => {
            let charts = ctx.library().list(filter(args)).await?;
            if json {
                render::json(&charts)?;
            } else {
                render::text(&charts_table(&charts))?;
            }
        }
        LibraryCmd::Scales => {
            let scales = ctx.library().counts_by_scale().await?;
            if json {
                render::json(&scales)?;
            } else {
                render::text(&scales_table(&scales))?;
            }
        }
        LibraryCmd::Patterns => {
            let counts = ctx.library().pattern_counts().await?;
            if json {
                render::json(&counts)?;
            } else {
                render::text(&patterns_table(&counts))?;
            }
        }
    }
    Ok(exit::exit_code(exit::SUCCESS))
}

async fn index(ctx: &AppContext, json: bool) -> anyhow::Result<ExitCode> {
    let (job, interrupted) = index_job(ctx, &mut Progress::stderr(), follow::ctrl_c()).await?;
    if json {
        render::json(&job)?;
    } else {
        render::text(&job_text(&job))?;
    }
    let code = if interrupted {
        exit::CANCELLED
    } else {
        exit::for_job(job.status)
    };
    Ok(exit::exit_code(code))
}

async fn index_job(
    ctx: &AppContext,
    progress: &mut Progress,
    interrupt: impl Future<Output = ()>,
) -> Result<(JobDto, bool), AppError> {
    let mut rx = ctx.subscribe();
    let id = ctx.job_service().start(JobStartDto::IndexLibrary).await?;
    let interrupted =
        follow::until_finished(ctx, &mut rx, &id, progress, std::pin::pin!(interrupt)).await?;
    Ok((follow::history_entry(ctx, &id).await?, interrupted))
}

fn filter(a: LibraryListArgs) -> LibraryFilterDto {
    LibraryFilterDto {
        keymode: a.keys,
        scale: a.scale,
        level_min: a.level_min,
        level_max: a.level_max,
        label_source: a.source,
        text: a.text,
        limit: a.limit,
        offset: a.offset,
    }
}

fn job_text(job: &JobDto) -> String {
    let mut pairs = vec![
        ("job", job.id.to_string()),
        ("status", render::wire(&job.status)),
    ];
    if let Some(JobSummaryDto::IndexLibrary(s)) = &job.summary {
        pairs.extend([
            ("charts total", s.charts_total.to_string()),
            ("parsed new", s.parsed_new.to_string()),
            ("skipped memoized", s.skipped_memoized.to_string()),
            ("skipped unavailable", s.skipped_unavailable.to_string()),
            ("labels written", s.labels_written.to_string()),
            ("segments written", s.segments_written.to_string()),
            ("failed items", s.failed_items.to_string()),
        ]);
    }
    if let Some(e) = &job.error {
        pairs.push((
            "error",
            format!("{} {}", render::wire(&e.code), e.message_key),
        ));
    }
    if job.status == JobStatusDto::Cancelled {
        pairs.push((
            "note",
            "cancelled; rerun `wolluf library index` to resume".to_owned(),
        ));
    }
    render::key_values(&pairs)
}

/// `m:ss`.
pub(crate) fn duration(ms: u32) -> String {
    let s = ms / MS_PER_SECOND;
    format!("{}:{:02}", s / SECONDS_PER_MINUTE, s % SECONDS_PER_MINUTE)
}

pub(crate) fn ln_percent(ratio: f64) -> String {
    format!("{:.1}", ratio * PERCENT)
}

/// `scale:level`, with `*` for a variant difficulty.
pub(crate) fn label(l: &ChartLabelDto) -> String {
    let variant = if l.is_variant { "*" } else { "" };
    format!("{}:{}{variant}", l.scale, l.level_text)
}

fn labels(labels: &[ChartLabelDto]) -> String {
    if labels.is_empty() {
        return "-".to_owned();
    }
    labels.iter().map(label).collect::<Vec<_>>().join(" ")
}

/// The full md5, because `chart show` takes nothing shorter.
fn charts_table(charts: &[LibraryChartDto]) -> String {
    let rows: Vec<Vec<String>> = charts
        .iter()
        .map(|c| {
            vec![
                c.md5.clone(),
                c.title.clone(),
                c.version.clone(),
                c.n_notes.to_string(),
                ln_percent(c.ln_ratio),
                duration(c.length_ms),
                format!("{:.2}", c.nps),
                labels(&c.labels),
            ]
        })
        .collect();
    render::table(
        &[
            "MD5", "TITLE", "VERSION", "NOTES", "LN%", "LENGTH", "NPS", "LABELS",
        ],
        &rows,
    )
}

fn patterns_table(counts: &[PatternCountDto]) -> String {
    let rows: Vec<Vec<String>> = counts
        .iter()
        .map(|c| {
            vec![
                c.keymode.to_string(),
                c.key.clone(),
                c.pattern_id.clone(),
                c.segments.to_string(),
                c.charts.to_string(),
                format!("{:.1}", c.total_s),
            ]
        })
        .collect();
    render::table(
        &["KEYS", "KEY", "PATTERN", "SEGMENTS", "CHARTS", "SECONDS"],
        &rows,
    )
}

fn scales_table(scales: &[ScaleCountDto]) -> String {
    let rows: Vec<Vec<String>> = scales
        .iter()
        .map(|s| vec![s.scale.clone(), s.rows.to_string(), s.charts.to_string()])
        .collect();
    render::table(&["SCALE", "ROWS", "CHARTS"], &rows)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use wolluf_app::context::AppPaths;
    use wolluf_core::{FixedClock, UnixUs};

    use super::*;

    const T0: UnixUs = UnixUs(1_790_637_236_636_000);

    fn context(dir: &std::path::Path) -> AppContext {
        AppContext::open(
            AppPaths::from_data_dir(dir.join("data")),
            Arc::new(FixedClock::new(T0)),
        )
        .unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn interrupt_cancels_the_index() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let (job, interrupted) = index_job(&ctx, &mut Progress::stderr(), std::future::ready(()))
            .await
            .unwrap();
        assert!(interrupted);
        // An empty catalog can finish before the cancel lands; either way the wait ends and
        // `interrupted` alone decides exit 130.
        assert!(
            matches!(job.status, JobStatusDto::Cancelled | JobStatusDto::Ok),
            "{job:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn no_interrupt_waits_for_ok() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let (job, interrupted) = index_job(&ctx, &mut Progress::stderr(), std::future::pending())
            .await
            .unwrap();
        assert!(!interrupted);
        assert_eq!(job.status, JobStatusDto::Ok, "{job:?}");
        assert!(matches!(job.summary, Some(JobSummaryDto::IndexLibrary(_))));
    }

    fn lab(scale: &str, level: &str, is_variant: bool) -> ChartLabelDto {
        ChartLabelDto {
            source: "bms_5ynt3ck".to_owned(),
            scale: scale.to_owned(),
            level_ord: None,
            level_text: level.to_owned(),
            skill_tag: None,
            is_variant,
        }
    }

    #[test]
    fn cells() {
        assert_eq!(duration(0), "0:00");
        assert_eq!(duration(65_999), "1:05");
        assert_eq!(duration(3_600_000), "60:00");
        assert_eq!(ln_percent(1.0 / 3.0), "33.3");
        assert_eq!(labels(&[]), "-");
        assert_eq!(
            labels(&[lab("insane", "4", false), lab("satellite", "sl2", true)]),
            "insane:4 satellite:sl2*"
        );
    }
}
