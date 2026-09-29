//! Helpers shared by the rules.

use wolluf_core::{ColMask, Keymode, PatternId};

use crate::params::{JackParams, StreamParams};
use crate::rule::{Candidate, STRENGTH_MAX};
use crate::view::{RowFeat, ticks};

/// A gap is jack-fast when it is within both the absolute cap and, under a red line, the
/// beat-relative one.
pub(super) fn jack_gap_ok(gap_us: i64, beat_us: Option<i64>, params: &JackParams) -> bool {
    gap_us <= params.max_gap_us
        && beat_us.is_none_or(|beat| ticks(gap_us, beat) <= params.max_gap_ticks)
}

/// Indices of the rows that press something; release-only rows never shape a jack.
pub(super) fn press_rows(rows: &[RowFeat]) -> Vec<usize> {
    rows.iter()
        .enumerate()
        .filter(|(_, r)| !r.press.is_empty())
        .map(|(i, _)| i)
        .collect()
}

pub(super) fn notes_between(rows: &[RowFeat], first: usize, last: usize) -> u64 {
    rows.get(first..=last)
        .map_or(0, |span| span.iter().map(|r| u64::from(r.notes)).sum())
}

/// `STRENGTH_MAX × part / whole`, clamped; 0 for an empty whole.
pub(super) fn permille(part: u64, whole: u64) -> u32 {
    if whole == 0 {
        return 0;
    }
    let value = part.saturating_mul(u64::from(STRENGTH_MAX)) / whole;
    u32::try_from(value.min(u64::from(STRENGTH_MAX))).unwrap_or(STRENGTH_MAX)
}

pub(super) fn single(k: Keymode, col: u8) -> ColMask {
    ColMask::single(k, col).unwrap_or_default()
}

/// A maximal run of presses in one column where each press is on the next press row and
/// jack-fast. `len` counts presses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ColumnRun {
    pub col: u8,
    pub first: usize,
    pub last: usize,
    pub len: u32,
}

pub(super) fn column_runs(rows: &[RowFeat], k: Keymode, params: &JackParams) -> Vec<ColumnRun> {
    let mut open: Vec<Option<ColumnRun>> = vec![None; usize::from(k.columns())];
    let mut done = Vec::new();
    for i in press_rows(rows) {
        let Some(row) = rows.get(i) else { continue };
        for (col, slot) in (0u8..).zip(open.iter_mut()) {
            if !row.press.contains(col) {
                done.extend(slot.take());
                continue;
            }
            // An open run means the previous press row pressed this column too.
            let fast = row
                .press_gap_us
                .is_some_and(|gap| jack_gap_ok(gap, row.beat_us, params));
            match slot {
                Some(run) if fast => {
                    run.last = i;
                    run.len += 1;
                }
                _ => {
                    done.extend(slot.replace(ColumnRun {
                        col,
                        first: i,
                        last: i,
                        len: 1,
                    }));
                }
            }
        }
    }
    done.extend(open.into_iter().flatten());
    done
}

/// The link from the previous press row to `row` is stream-fast: within the absolute cap and,
/// under a red line, the beat-relative one.
pub(super) fn stream_gap_ok(row: &RowFeat, params: &StreamParams) -> bool {
    row.press_gap_us.is_some_and(|gap| {
        gap <= params.max_gap_us
            && row
                .beat_us
                .is_none_or(|beat| ticks(gap, beat) <= params.max_gap_ticks)
    })
}

pub(super) fn disjoint(a: ColMask, b: ColMask) -> bool {
    a.bits() & b.bits() == 0
}

/// Length confidence: `STRENGTH_MAX × min(count, 2 × min) / (2 × min)`, so a run at the
/// minimum scores half and one twice as long scores full.
pub(super) fn saturating(count: usize, min: u32) -> u32 {
    let full = u64::from(min).saturating_mul(2).max(1);
    let count = u64::try_from(count).unwrap_or(u64::MAX).min(full);
    permille(count, full)
}

pub(super) fn share<T>(items: &[T], matches: impl Fn(&T) -> bool) -> u32 {
    let hits = items.iter().filter(|item| matches(item)).count();
    permille(
        u64::try_from(hits).unwrap_or(0),
        u64::try_from(items.len()).unwrap_or(0),
    )
}

/// What [`scan`] does with the next press row.
pub(super) enum Step {
    Extend,
    /// Close the run and start a new one at this row.
    Restart,
    /// Close the run and start a new one at its last row plus this row, for shapes where
    /// consecutive windows overlap by one row.
    RestartFromPrev,
    /// Close the run; this row starts nothing.
    Break,
}

/// Walks the press rows in order, letting `step` see the current run (row indices) and the next
/// row. Returns every closed non-empty run in time order.
pub(super) fn scan(
    rows: &[RowFeat],
    mut step: impl FnMut(&[usize], usize) -> Step,
) -> Vec<Vec<usize>> {
    let mut done = Vec::new();
    let mut run: Vec<usize> = Vec::new();
    for i in press_rows(rows) {
        match step(&run, i) {
            Step::Extend => run.push(i),
            Step::Restart => {
                done.push(std::mem::take(&mut run));
                run.push(i);
            }
            Step::RestartFromPrev => {
                let last = run.last().copied();
                done.push(std::mem::take(&mut run));
                run.extend(last);
                run.push(i);
            }
            Step::Break => done.push(std::mem::take(&mut run)),
        }
    }
    done.push(run);
    done.retain(|run| !run.is_empty());
    done
}

/// Maximal jackless, stream-fast runs of rows passing `row_ok`.
pub(super) fn stream_runs(
    rows: &[RowFeat],
    params: &StreamParams,
    row_ok: impl Fn(&RowFeat) -> bool,
) -> Vec<Vec<usize>> {
    scan(rows, |run, i| {
        let Some(row) = rows.get(i) else {
            return Step::Break;
        };
        if !row_ok(row) {
            Step::Break
        } else if !run.is_empty() && row.jacks == 0 && stream_gap_ok(row, params) {
            Step::Extend
        } else {
            Step::Restart
        }
    })
}

/// Maximal runs where rows alternate between two disjoint masks (`a b a b …`), each link
/// stream-fast and each row passing `side_ok`.
pub(super) fn alternations(
    rows: &[RowFeat],
    params: &StreamParams,
    side_ok: impl Fn(&RowFeat) -> bool,
) -> Vec<Vec<usize>> {
    scan(rows, |run, i| {
        let Some(row) = rows.get(i) else {
            return Step::Break;
        };
        if !side_ok(row) {
            return Step::Break;
        }
        let prev = run.last().and_then(|&p| rows.get(p));
        let linked =
            prev.is_some_and(|p| disjoint(p.press, row.press) && stream_gap_ok(row, params));
        if !linked {
            return Step::Restart;
        }
        let two_back = run
            .len()
            .checked_sub(2)
            .and_then(|k| run.get(k))
            .and_then(|&k| rows.get(k));
        match two_back {
            Some(back) if back.press != row.press => Step::RestartFromPrev,
            _ => Step::Extend,
        }
    })
}

pub(super) fn rows_of<'r>(rows: &'r [RowFeat], run: &[usize]) -> Vec<&'r RowFeat> {
    run.iter().filter_map(|&i| rows.get(i)).collect()
}

pub(super) fn union(k: Keymode, masks: impl IntoIterator<Item = ColMask>) -> ColMask {
    let bits = masks.into_iter().fold(0u16, |acc, m| acc | m.bits());
    ColMask::from_bits(k, bits).unwrap_or_default()
}

/// A candidate over `span` (non-empty, in time order) with the union of its presses.
pub(super) fn span_candidate(
    pattern: PatternId,
    k: Keymode,
    span: &[&RowFeat],
    strength: u32,
) -> Option<Candidate> {
    Some(Candidate {
        pattern,
        t0: span.first()?.t,
        t1: span.last()?.t,
        cols: union(k, span.iter().map(|r| r.press)),
        strength,
    })
}

/// `gap_us` is within `max_us` and, under a red line, within `max_ticks`.
pub(super) fn gap_within(gap_us: i64, beat_us: Option<i64>, max_ticks: u32, max_us: i64) -> bool {
    gap_us <= max_us && beat_us.is_none_or(|beat| ticks(gap_us, beat) <= max_ticks)
}

/// Spans of press rows (row indices) covered by windows of `n` consecutive press rows that pass
/// `passes`; overlapping or touching passing windows merge, and no window spans a press gap
/// over `max_gap_us`.
pub(super) fn window_spans(
    rows: &[RowFeat],
    n: u32,
    max_gap_us: i64,
    passes: impl Fn(&[usize]) -> bool,
) -> Vec<Vec<usize>> {
    let mut sections: Vec<Vec<usize>> = vec![Vec::new()];
    for i in press_rows(rows) {
        let broken = rows
            .get(i)
            .and_then(|r| r.press_gap_us)
            .is_some_and(|gap| gap > max_gap_us);
        if broken {
            sections.push(Vec::new());
        }
        if let Some(section) = sections.last_mut() {
            section.push(i);
        }
    }
    let n = usize::try_from(n).unwrap_or(usize::MAX).max(1);
    let mut spans = Vec::new();
    for section in sections.iter().filter(|s| s.len() >= n) {
        let mut open: Option<(usize, usize)> = None;
        for start in 0..=section.len() - n {
            if !section.get(start..start + n).is_some_and(&passes) {
                continue;
            }
            open = match open {
                Some((a, end)) if start <= end => Some((a, start + n)),
                other => {
                    spans.extend(
                        other
                            .and_then(|(a, e)| section.get(a..e))
                            .map(<[usize]>::to_vec),
                    );
                    Some((start, start + n))
                }
            };
        }
        spans.extend(
            open.and_then(|(a, e)| section.get(a..e))
                .map(<[usize]>::to_vec),
        );
    }
    spans
}
