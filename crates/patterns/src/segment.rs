//! Resolves rule candidates into segments: at most one primary pattern per instant, with the
//! overlapping losers and the section-level measures as secondary tags.
//!
//! Per chart row, the covering primary candidates are ranked by the `segment.priority` table
//! (an `ln.*` candidate falls behind every rice pattern unless LNs are a real share of its
//! span), then strength, then id and span. The row's winner decides its pattern. Rows won by one
//! pattern form a run; runs of the same pattern merge across rows nobody covers when the gap is
//! within the merge gap. A run shorter than its class minimum, or than two rows, is withdrawn:
//! its pattern is barred on those rows and they are resolved again, so the next candidate in
//! line can take them. Runs over the maximum split into equal parts at row boundaries, so a
//! chord is never cut.

use std::cmp::Reverse;
use std::collections::BTreeSet;

use wolluf_core::{AxisId, ColMask, PatternId, TimeUs};

use crate::axes::axis_of;
use crate::params::{PatternParams, SegmentParams};
use crate::rule::{Candidate, STRENGTH_MAX};
use crate::rules;
use crate::view::{ChartView, RowFeat, ticks};

const LN_PREFIX: &str = "ln.";
const PERMILLE: u64 = 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub t0: TimeUs,
    pub t1: TimeUs,
    /// Columns of the primary pattern's candidates in the segment.
    pub cols: ColMask,
    pub primary: PatternId,
    pub axis: AxisId,
    /// Sorted, without duplicates, never the primary.
    pub secondary: Vec<PatternId>,
    /// Share of the segment's press rows (for `ln.*`, rows with any press or release) covered
    /// by the primary pattern's candidates.
    pub purity: u16,
    /// The strongest primary-pattern candidate in the segment.
    pub strength: u16,
}

/// A candidate placed on the row index range it covers.
struct Placed {
    c: Candidate,
    first: usize,
    last: usize,
}

struct Run {
    pattern: PatternId,
    /// Rows this pattern won, ascending.
    won: Vec<usize>,
}

pub fn segment(view: &ChartView<'_>, params: &PatternParams) -> Vec<Segment> {
    let sp = &params.segment;
    let rows = view.rows();
    let (mut primary, mut tags) = (Vec::new(), Vec::new());
    for rule in rules::all().iter().filter(|r| r.supports(view.keymode())) {
        let bucket = if sp.tag_only.contains(&rule.id()) {
            &mut tags
        } else {
            &mut primary
        };
        bucket.extend(
            rule.detect(view, params)
                .into_iter()
                .filter_map(|c| place(rows, c)),
        );
    }
    let key = |p: &Placed| {
        (
            rank(view, sp, p),
            Reverse(p.c.strength),
            p.c.pattern.clone(),
            p.c.t0,
            p.c.t1,
            p.c.cols,
        )
    };
    primary.sort_by_cached_key(key);

    let mut covering: Vec<Vec<usize>> = vec![Vec::new(); rows.len()];
    for (idx, p) in primary.iter().enumerate() {
        for slot in covering.get_mut(p.first..=p.last).into_iter().flatten() {
            slot.push(idx);
        }
    }
    let mut barred: Vec<BTreeSet<PatternId>> = vec![BTreeSet::new(); rows.len()];
    let runs = loop {
        let winners: Vec<Option<usize>> = covering
            .iter()
            .zip(&barred)
            .map(|(cands, bar)| {
                cands
                    .iter()
                    .copied()
                    .find(|&i| primary.get(i).is_some_and(|p| !bar.contains(&p.c.pattern)))
            })
            .collect();
        let runs = runs_of(rows, &primary, &winners, sp);
        let short: Vec<&Run> = runs.iter().filter(|r| !long_enough(rows, r, sp)).collect();
        if short.is_empty() {
            break runs;
        }
        for run in short {
            for &row in &run.won {
                if let Some(bar) = barred.get_mut(row) {
                    bar.insert(run.pattern.clone());
                }
            }
        }
    };

    let mut segments: Vec<Segment> = runs
        .iter()
        .flat_map(|run| split(rows, run, sp))
        .filter_map(|run| build(view, &primary, &tags, &run, sp))
        .collect();
    segments.sort_by_key(|s| s.t0);
    segments
}

fn place(rows: &[RowFeat], c: Candidate) -> Option<Placed> {
    let first = rows.partition_point(|r| r.t < c.t0);
    let last = rows.partition_point(|r| r.t <= c.t1).checked_sub(1)?;
    (first <= last).then_some(Placed { c, first, last })
}

/// Index in the priority table; `ln.*` candidates without a real LN share, and patterns missing
/// from the table, rank after the whole table.
fn rank(view: &ChartView<'_>, sp: &SegmentParams, p: &Placed) -> usize {
    let table = sp.priority.len();
    let base = sp
        .priority
        .iter()
        .position(|id| *id == p.c.pattern)
        .unwrap_or(table);
    let demoted = p.c.pattern.as_str().starts_with(LN_PREFIX)
        && ln_share(view, p.first, p.last) < u64::from(sp.ln_priority_min_share_permille);
    if demoted { base + table } else { base }
}

/// LN heads and tails over presses plus tails, permille.
fn ln_share(view: &ChartView<'_>, first: usize, last: usize) -> u64 {
    let (Some(chart_rows), Some(feats)) = (
        view.chart().rows().get(first..=last),
        view.rows().get(first..=last),
    ) else {
        return 0;
    };
    let ln: u64 = chart_rows
        .iter()
        .map(|r| u64::from(r.ln_head.len() + r.ln_tail.len()))
        .sum();
    let events: u64 = feats
        .iter()
        .map(|r| u64::from(r.notes + r.release.len()))
        .sum();
    (ln * PERMILLE).checked_div(events).unwrap_or(0)
}

/// Consecutive rows won by one pattern, joined across uncovered rows. Two won rows join when
/// the same candidate won both or their gap is within the merge gap.
fn runs_of(
    rows: &[RowFeat],
    primary: &[Placed],
    winners: &[Option<usize>],
    sp: &SegmentParams,
) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    let mut last: Option<(usize, usize)> = None;
    for (row, winner) in winners.iter().enumerate() {
        let Some(idx) = *winner else { continue };
        let Some(pattern) = primary.get(idx).map(|p| &p.c.pattern) else {
            continue;
        };
        let joins = match (last, runs.last()) {
            (Some((prev_row, prev_idx)), Some(run)) if run.pattern == *pattern => {
                prev_idx == idx || within_merge_gap(rows, prev_row, row, sp)
            }
            _ => false,
        };
        match runs.last_mut() {
            Some(run) if joins => run.won.push(row),
            _ => runs.push(Run {
                pattern: pattern.clone(),
                won: vec![row],
            }),
        }
        last = Some((row, idx));
    }
    runs
}

fn within_merge_gap(rows: &[RowFeat], a: usize, b: usize, sp: &SegmentParams) -> bool {
    let (Some(ra), Some(rb)) = (rows.get(a), rows.get(b)) else {
        return false;
    };
    let gap = rb.t.0 - ra.t.0;
    gap <= sp.merge_gap_us
        && rb
            .beat_us
            .is_none_or(|beat| ticks(gap, beat) <= sp.merge_gap_ticks)
}

/// A segment spans at least two rows: one instant has no pattern.
fn long_enough(rows: &[RowFeat], run: &Run, sp: &SegmentParams) -> bool {
    let (Some(first), Some(last)) = (
        run.won.first().and_then(|&i| rows.get(i)),
        run.won.last().and_then(|&i| rows.get(i)),
    ) else {
        return false;
    };
    let len = last.t.0 - first.t.0;
    if len <= 0 {
        return false;
    }
    if sp.short_class.contains(&run.pattern) {
        return len >= sp.short_min_len_us;
    }
    len >= sp.min_len_us
        || first
            .beat_us
            .is_some_and(|beat| ticks(len, beat) >= sp.min_len_ticks)
}

/// Splits a run longer than `max_len_us` into the fewest equal parts that fit, cutting between
/// rows.
fn split(rows: &[RowFeat], run: &Run, sp: &SegmentParams) -> Vec<Run> {
    let times: Vec<i64> = run
        .won
        .iter()
        .filter_map(|&i| rows.get(i))
        .map(|r| r.t.0)
        .collect();
    let (Some(&start), Some(&end)) = (times.first(), times.last()) else {
        return Vec::new();
    };
    let len = end - start;
    let max = sp.max_len_us.max(1);
    if len <= max {
        return vec![Run {
            pattern: run.pattern.clone(),
            won: run.won.clone(),
        }];
    }
    let parts = (len + max - 1) / max;
    let mut pieces: Vec<Run> = (0..parts)
        .map(|_| Run {
            pattern: run.pattern.clone(),
            won: Vec::new(),
        })
        .collect();
    for (&row, &t) in run.won.iter().zip(&times) {
        let part = usize::try_from(((t - start) * parts / len).min(parts - 1)).unwrap_or(0);
        if let Some(piece) = pieces.get_mut(part) {
            piece.won.push(row);
        }
    }
    pieces.retain(|p| p.won.len() > 1);
    pieces
}

fn build(
    view: &ChartView<'_>,
    primary: &[Placed],
    tags: &[Placed],
    run: &Run,
    sp: &SegmentParams,
) -> Option<Segment> {
    let rows = view.rows();
    let (&first, &last) = (run.won.first()?, run.won.last()?);
    let span = last - first + 1;
    let overlap = |p: &Placed| p.last.min(last).saturating_sub(p.first.max(first)) + 1;
    let overlaps = |p: &Placed| p.first <= last && p.last >= first;
    let own: Vec<&Placed> = primary
        .iter()
        .filter(|p| p.c.pattern == run.pattern && overlaps(p))
        .collect();
    let ln = run.pattern.as_str().starts_with(LN_PREFIX);
    let counted: Vec<usize> = (first..=last)
        .filter(|&i| {
            rows.get(i)
                .is_some_and(|r| !r.press.is_empty() || (ln && !r.release.is_empty()))
        })
        .collect();
    let covered = counted
        .iter()
        .filter(|&&i| own.iter().any(|p| p.first <= i && i <= p.last))
        .count();
    let secondary: BTreeSet<PatternId> = primary
        .iter()
        .chain(tags)
        .filter(|p| p.c.pattern != run.pattern && overlaps(p))
        .filter(|p| permille(overlap(p), span) >= u64::from(sp.secondary_min_share_permille))
        .map(|p| p.c.pattern.clone())
        .collect();
    let cols = own.iter().fold(0u16, |acc, p| acc | p.c.cols.bits());
    Some(Segment {
        t0: rows.get(first)?.t,
        t1: rows.get(last)?.t,
        cols: ColMask::from_bits(view.keymode(), cols).unwrap_or_default(),
        axis: axis_of(view.keymode(), &run.pattern)?,
        primary: run.pattern.clone(),
        secondary: secondary.into_iter().collect(),
        purity: u16::try_from(permille(covered, counted.len())).unwrap_or(0),
        strength: own
            .iter()
            .map(|p| p.c.strength.min(STRENGTH_MAX))
            .max()
            .and_then(|s| u16::try_from(s).ok())
            .unwrap_or(0),
    })
}

fn permille(part: usize, whole: usize) -> u64 {
    let (part, whole) = (
        u64::try_from(part).unwrap_or(0),
        u64::try_from(whole).unwrap_or(0),
    );
    (part * PERMILLE).checked_div(whole).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use wolluf_chart::testkit::{chart_from_rows, render_rows};
    use wolluf_chart::{Chart, Layout, chart};
    use wolluf_core::{PatternId, TimeUs};

    use super::*;
    use crate::params::PatternParams;
    use crate::rules::testkit::{layout, seq, taps, with_beat};
    use crate::view::ChartView;

    fn pid(id: &'static str) -> PatternId {
        PatternId::from_static(id)
    }

    fn ms(t: i32) -> TimeUs {
        TimeUs::from_ms(t)
    }

    /// Defaults without the minimum segment length, to look at priority alone.
    fn loose() -> PatternParams {
        let mut params = PatternParams::default();
        params.segment.min_len_us = 0;
        params
    }

    fn run(chart: &Chart, layout: &Layout, params: &PatternParams) -> Vec<Segment> {
        segment(&ChartView::new(chart, layout, params).unwrap(), params)
    }

    fn run_loose(chart: &Chart) -> Vec<Segment> {
        run(chart, &Layout::default_for(chart.keymode()), &loose())
    }

    fn primaries(segments: &[Segment]) -> Vec<&str> {
        segments.iter().map(|s| s.primary.as_str()).collect()
    }

    fn only(segments: &[Segment], primary: &str) -> Segment {
        assert_eq!(primaries(segments), [primary], "{segments:#?}");
        segments[0].clone()
    }

    fn tagged(segment: &Segment, pattern: &'static str) -> bool {
        segment.secondary.contains(&pid(pattern))
    }

    #[test]
    fn split_trill_beats_trill() {
        let chart = chart![step = 100; "...x...", "....x..", "...x...", "....x.."];
        let s = only(
            &run(&chart, &layout("k7.313_left_thumb"), &loose()),
            "regular.stream.split_trill",
        );
        assert!(tagged(&s, "regular.stream.trill"), "{s:?}");
        assert_eq!(s.axis.as_str(), "7k.regular.stream");
        assert_eq!((s.t0, s.t1), (ms(0), ms(300)));
    }

    #[test]
    fn split_trill_beats_jumptrill_and_light_chordstream() {
        let chart = chart![step = 100; "xxx....", "....xxx", "xxx....", "....xxx"];
        let s = only(&run_loose(&chart), "regular.stream.split_trill");
        assert!(tagged(&s, "regular.stream.jumptrill"), "{s:?}");
        assert!(tagged(&s, "regular.stream.chordstream_light"), "{s:?}");
    }

    #[test]
    fn bracket_beats_trill() {
        let chart = chart![step = 100;
            "x......", ".x.....", "x......", ".x.....",
            "x.x....", ".x.....", "x.x....", ".x.....",
        ];
        let s = only(&run_loose(&chart), "regular.stream.bracket");
        assert!(tagged(&s, "regular.stream.trill"), "{s:?}");
        assert_eq!((s.t0, s.t1), (ms(0), ms(700)));
    }

    #[test]
    fn chordbracket_beats_light_chordstream() {
        let chart = chart![step = 100; "x.x....", ".x.x...", "..x.x..", "...x.x."];
        let s = only(&run_loose(&chart), "regular.stream.chordbracket");
        assert!(tagged(&s, "regular.stream.chordstream_light"), "{s:?}");
    }

    #[test]
    fn roll_beats_single() {
        let chart = chart![step = 100;
            "x......", ".x.....", "..x....", "...x...", "....x..", ".....x.", "......x",
            ".....x.", "....x..", "...x...", "..x....", ".x.....", "x......",
        ];
        let s = only(&run_loose(&chart), "regular.stream.roll");
        assert!(tagged(&s, "regular.stream.single"), "{s:?}");
        assert_eq!((s.t0, s.t1), (ms(0), ms(1200)));
    }

    #[test]
    fn jumptrill_beats_chordstreams() {
        let light = chart![step = 100; "xx.....", "..xx...", "xx.....", "..xx..."];
        let s = only(&run_loose(&light), "regular.stream.jumptrill");
        assert!(tagged(&s, "regular.stream.chordstream_light"), "{s:?}");
        let dense = chart![step = 100; "xxxx...", "....xxx", "xxxx...", "....xxx"];
        let s = only(&run_loose(&dense), "regular.stream.jumptrill");
        assert!(tagged(&s, "regular.stream.chordstream_dense"), "{s:?}");
    }

    #[test]
    fn chordjack_beats_longjack_and_minijack() {
        let chart = chart![step = 100; "xxx....", "xx.x...", "x.x.x.."];
        let s = only(&run_loose(&chart), "regular.jack.chordjack");
        assert!(tagged(&s, "regular.jack.longjack"), "{s:?}");
        assert!(tagged(&s, "regular.jack.minijack"), "{s:?}");
    }

    #[test]
    fn shield_beats_minijack() {
        let chart = chart![step = 100;
            "x......", "[......", "|x.....", "][.....", ".|.....", ".].....",
        ];
        let s = only(&run_loose(&chart), "ln.tech.shield");
        assert!(tagged(&s, "regular.jack.minijack"), "{s:?}");
        assert_eq!((s.t0, s.t1), (ms(0), ms(300)));
        assert_eq!(s.axis.as_str(), "7k.ln.tech");
    }

    #[test]
    fn shield_beats_longjack_and_a_one_row_remainder_is_dropped() {
        let chart = chart![step = 100; "x.x....", "x.x....", "[.[....", "|.|....", "].]...."];
        let s = only(&run_loose(&chart), "ln.tech.shield");
        assert!(tagged(&s, "regular.jack.longjack"), "{s:?}");
        assert_eq!((s.t0, s.t1), (ms(100), ms(200)));
    }

    #[test]
    fn inverse_beats_ln_chord() {
        let (head, body, tail, gap) = ("[[[[[[[", "|||||||", "]]]]]]]", ".......");
        let mut rows = Vec::new();
        for block in 0..3 {
            rows.push(head);
            rows.extend([body; 4]);
            rows.push(tail);
            if block < 2 {
                rows.push(gap);
            }
        }
        let chart = chart_from_rows(0, 50, &rows).unwrap();
        let s = only(&run_loose(&chart), "ln.inverse.gap");
        assert!(tagged(&s, "ln.general.chord"), "{s:?}");
        assert!(tagged(&s, "regular.jack.chordjack"), "{s:?}");
        assert_eq!((s.t0, s.t1), (ms(0), ms(950)));
    }

    #[test]
    fn hybrid_beats_rice_only_when_long_notes_are_a_real_share() {
        let frequent = chart![step = 100;
            "x.....[", "..x...|", ".x....|", "...x..]",
            "..x...[", "....x.|", "...x..|", ".x....]",
            "x.....[", "..x...|", ".x....|", "...x..]",
        ];
        let s = only(&run_loose(&frequent), "ln.tech.hybrid");
        assert!(tagged(&s, "regular.stream.jumpstream"), "{s:?}");
        assert_eq!((s.t0, s.t1), (ms(100), ms(1000)));

        let one_long_note = chart![step = 100;
            "x.....[", "..x...|", ".x....|", "...x..|",
            "..x...|", "....x.|", "...x..|", ".x....|",
            "....x.|", "..x...|", ".x....|", "...x..]",
        ];
        let s = only(&run_loose(&one_long_note), "regular.stream.single");
        assert!(tagged(&s, "ln.tech.hybrid"), "{s:?}");
    }

    #[test]
    fn release_timing_beats_rice_under_staggered_tails() {
        let chart = chart![step = 50;
            "...[[[[", "x..||||", ".x.||||", "..x]|||", ".x..]||",
            "x....]|", ".x....]", "..x....", ".x.....",
        ];
        let segments = run_loose(&chart);
        assert_eq!(
            primaries(&segments),
            ["ln.tech.hybrid", "ln.release.timing", "regular.jack.anchor"],
            "{segments:#?}"
        );
        let release = &segments[1];
        assert_eq!((release.t0, release.t1), (ms(150), ms(300)));
        assert!(tagged(release, "regular.stream.single"), "{release:?}");
        assert!(tagged(release, "ln.tech.hybrid"), "{release:?}");
    }

    #[test]
    fn burst_beats_the_stream_it_sits_in() {
        let mut gaps = vec![240; 7];
        gaps.extend([30; 3]);
        gaps.extend([240; 8]);
        let segments = run_loose(&seq(&gaps, &[0, 2, 1, 3], None));
        assert_eq!(
            primaries(&segments),
            [
                "regular.stream.single",
                "regular.speed.burst",
                "regular.stream.single"
            ],
            "{segments:#?}"
        );
        assert_eq!((segments[1].t0, segments[1].t1), (ms(1680), ms(1770)));
        assert!(tagged(&segments[1], "regular.stream.single"));
    }

    #[test]
    fn window_rules_only_tag() {
        let cols: Vec<u8> = [3, 1, 4, 3, 0, 5]
            .iter()
            .copied()
            .cycle()
            .take(18)
            .collect();
        let notes: Vec<(i32, u8)> = cols
            .iter()
            .enumerate()
            .map(|(i, &c)| (i as i32 * 100, c))
            .collect();
        let s = only(&run_loose(&taps(&notes, None)), "regular.stream.single");
        assert!(tagged(&s, "regular.tech.thumb"), "{s:?}");

        // Only hand_imbalance fires on a slow one-hand section: no segment at all.
        assert!(run_loose(&seq(&[300; 19], &[0, 1, 2], None)).is_empty());
    }

    #[test]
    fn same_pattern_merges_across_a_small_gap_only() {
        let near = taps(&[(0, 0), (100, 0), (400, 3), (500, 3)], None);
        let params = PatternParams::default();
        let s = only(
            &run(&near, &layout("k7.313_right_thumb"), &params),
            "regular.jack.minijack",
        );
        assert_eq!((s.t0, s.t1), (ms(0), ms(500)));
        assert_eq!(s.purity, 1000);

        let far = taps(&[(0, 0), (100, 0), (700, 3), (800, 3)], None);
        let segments = run(&far, &layout("k7.313_right_thumb"), &params);
        assert_eq!(
            primaries(&segments),
            ["regular.jack.minijack", "regular.jack.minijack"]
        );
    }

    #[test]
    fn long_runs_split_at_row_boundaries() {
        let rows: Vec<&str> = (0..200)
            .map(|i| if i % 2 == 0 { "xx....." } else { "..xx..." })
            .collect();
        let chart = chart_from_rows(0, 100, &rows).unwrap();
        let params = PatternParams::default();
        let segments = run(&chart, &Layout::default_for(chart.keymode()), &params);
        let spans: Vec<(TimeUs, TimeUs)> = segments.iter().map(|s| (s.t0, s.t1)).collect();
        assert_eq!(
            spans,
            [
                (ms(0), ms(6600)),
                (ms(6700), ms(13200)),
                (ms(13300), ms(19900))
            ]
        );
        assert!(
            segments
                .iter()
                .all(|s| s.primary.as_str() == "regular.stream.jumptrill")
        );
        assert!(
            segments
                .iter()
                .all(|s| s.t1.0 - s.t0.0 <= params.segment.max_len_us)
        );
    }

    #[test]
    fn short_segments_drop_unless_short_by_nature_or_long_in_beats() {
        let params = PatternParams::default();
        let k7 = Layout::default_for(wolluf_core::Keymode::K7);
        let rows = [
            "x......", ".x.....", "x......", ".x.....", "x......", ".x.....", "x......", ".x.....",
            "x......",
        ];
        let trill = chart_from_rows(0, 100, &rows).unwrap();
        assert!(run(&trill, &k7, &params).is_empty());
        // 800 ms is four 200 ms beats.
        let s = only(
            &run(&with_beat(&trill, 200.0), &k7, &params),
            "regular.stream.trill",
        );
        assert_eq!((s.t0, s.t1), (ms(0), ms(800)));
    }

    #[test]
    fn mixed_7k_fixture() {
        let mut rows: Vec<&str> = Vec::new();
        let jumpstream = [
            "x......", "..xx...", "x......", ".x..x..", "..x....", "x....x.", "...x...", ".x..x..",
        ];
        for _ in 0..4 {
            rows.extend(jumpstream);
        }
        // Gaps between sections keep the jack and shield rules from bridging them.
        rows.extend([
            ".......", ".......", "xxx....", "xxx....", "xx.x...", "xx.x...", "x.xx...",
        ]);
        rows.extend([".......", ".......", "......."]);
        for block in 0..4 {
            rows.push("[[[[[[[");
            rows.extend(["|||||||"; 5]);
            rows.push("]]]]]]]");
            if block < 3 {
                rows.push(".......");
            }
        }
        rows.push(".......");
        let hybrid = [
            "x.....[", "..x...|", ".x....|", "...x..]", "..x...[", "....x.|", "...x..|", ".x....]",
        ];
        for _ in 0..3 {
            rows.extend(hybrid);
        }
        let chart = chart_from_rows(0, 100, &rows).unwrap();
        let params = PatternParams::default();
        let segments = run(&chart, &Layout::default_for(chart.keymode()), &params);
        let mut text = render_rows(&chart);
        text.push_str("\nsegments:\n");
        for s in &segments {
            let secondary: Vec<&str> = s.secondary.iter().map(|p| p.as_str()).collect();
            text.push_str(&format!(
                "{:>8}..{:<8} {:<30} {:<18} cols={:07b} purity={} strength={} secondary=[{}]\n",
                s.t0.0 / 1000,
                s.t1.0 / 1000,
                s.primary.as_str(),
                s.axis.as_str(),
                s.cols.bits().reverse_bits() >> 9,
                s.purity,
                s.strength,
                secondary.join(", ")
            ));
        }
        insta::assert_snapshot!("mixed_7k", text);
    }
}

#[cfg(test)]
mod props {
    use proptest::prelude::*;
    use wolluf_chart::Note;
    use wolluf_core::Keymode;

    use super::*;
    use crate::axes::axis_of;
    use crate::params::PatternParams;
    use crate::rules::generate::{arb_input, build, preset_layout};
    use crate::view::ChartView;

    const K: Keymode = Keymode::K7;

    proptest! {
        #[test]
        fn segments_are_disjoint_ordered_and_well_formed((notes, timing, preset) in arb_input()) {
            let chart = build(notes, timing);
            let layout = preset_layout(preset);
            let params = PatternParams::default();
            let view = ChartView::new(&chart, &layout, &params).unwrap();
            let segments = segment(&view, &params);
            prop_assert_eq!(&segments, &segment(&view, &params));
            prop_assert!(segments.windows(2).all(|w| w[0].t1 < w[1].t0), "{:#?}", segments);
            let times: Vec<_> = view.rows().iter().map(|r| r.t).collect();
            let sp = &params.segment;
            for s in &segments {
                prop_assert!(s.t0 < s.t1);
                prop_assert!(times.binary_search(&s.t0).is_ok() && times.binary_search(&s.t1).is_ok());
                prop_assert!(!sp.tag_only.contains(&s.primary));
                prop_assert_eq!(Some(s.axis.clone()), axis_of(K, &s.primary));
                prop_assert!(s.secondary.windows(2).all(|w| w[0] < w[1]));
                prop_assert!(!s.secondary.contains(&s.primary));
                prop_assert!(s.purity <= 1000 && s.strength <= 1000);
                prop_assert!(!s.cols.is_empty());
                prop_assert!(s.t1.0 - s.t0.0 <= sp.max_len_us);
            }
        }

        #[test]
        fn mirroring_chart_and_layout_mirrors_segments((notes, timing, preset) in arb_input()) {
            let flipped: Vec<Note> = notes.iter().map(|n| Note { col: 6 - n.col, ..*n }).collect();
            let chart = build(notes, timing.clone());
            let mirror_chart = build(flipped, timing);
            let layout = preset_layout(preset);
            let mirror_layout = layout.mirror();
            let params = PatternParams::default();
            let a = segment(&ChartView::new(&chart, &layout, &params).unwrap(), &params);
            let b = segment(&ChartView::new(&mirror_chart, &mirror_layout, &params).unwrap(), &params);
            let expected: Vec<Segment> = a
                .into_iter()
                .map(|s| Segment { cols: s.cols.mirror(K), ..s })
                .collect();
            prop_assert_eq!(b, expected);
        }
    }
}
