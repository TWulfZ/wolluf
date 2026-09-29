//! `wolluf chart show|info` over `LibraryService::render` and `get` (F1).

use std::process::ExitCode;

use wolluf_app::context::AppContext;
use wolluf_app::features::library::dto::{ChartDetailDto, ChartLabelDto, SegmentDto};

use crate::cli::ChartCmd;
use crate::cmd::label::clock;
use crate::cmd::library::{duration, label, ln_percent};
use crate::exit;
use crate::render;

pub(crate) async fn run(ctx: &AppContext, cmd: ChartCmd, json: bool) -> anyhow::Result<ExitCode> {
    match cmd {
        ChartCmd::Show(args) => {
            let (from_ms, to_ms) = args.window();
            let text = ctx
                .library()
                .render(
                    &args.md5,
                    from_ms,
                    to_ms,
                    args.layout.as_deref(),
                    args.segments,
                )
                .await?;
            if json {
                render::json(&text)?;
            } else {
                render::text(&text)?;
            }
        }
        ChartCmd::Info { md5 } => {
            let detail = ctx.library().get(&md5).await?;
            if json {
                render::json(&detail)?;
            } else {
                render::text(&detail_text(&detail))?;
            }
        }
    }
    Ok(exit::exit_code(exit::SUCCESS))
}

fn label_line(l: &ChartLabelDto) -> String {
    let mut line = format!("{} {}", l.source, label(l));
    if let Some(tag) = &l.skill_tag {
        line.push_str(&format!(" ({tag})"));
    }
    line
}

fn detail_text(d: &ChartDetailDto) -> String {
    let c = &d.chart;
    let mut pairs = vec![
        ("md5", c.md5.clone()),
        ("title", c.title.clone()),
        ("artist", c.artist.clone()),
        ("version", c.version.clone()),
        ("creator", c.creator.clone()),
        ("keys", c.keymode.to_string()),
        ("notes", c.n_notes.to_string()),
        ("ln", c.n_ln.to_string()),
        ("ln%", ln_percent(c.ln_ratio)),
        ("length", duration(c.length_ms)),
        ("nps", format!("{:.2}", c.nps)),
        ("diagnostics", render::opt(d.diagnostics)),
    ];
    if c.labels.is_empty() {
        pairs.push(("labels", "-".to_owned()));
    }
    pairs.extend(c.labels.iter().map(|l| ("label", label_line(l))));
    if d.segments.is_empty() {
        pairs.push(("segments", "-".to_owned()));
    }
    pairs.extend(d.segments.iter().map(|s| ("segment", segment_line(s))));
    render::key_values(&pairs)
}

/// `t0-t1 key pattern purity strength [+secondary,…]`.
fn segment_line(s: &SegmentDto) -> String {
    let mut line = format!(
        "{}-{} {} {} purity {} strength {}",
        clock(s.t0_ms),
        clock(s.t1_ms),
        s.key,
        s.pattern_id,
        s.purity,
        s.strength
    );
    if !s.secondary.is_empty() {
        line.push_str(&format!(" +{}", s.secondary.join(",")));
    }
    line
}
