//! Helpers shared by the LN rules: which LNs count, grouping of LN events into sections, and
//! held coverage.

use wolluf_core::{ColMask, Keymode, TimeUs};

use super::common::gap_within;
use crate::params::LnParams;
use crate::view::{ChartView, RowFeat, ticks};

/// An LN long enough to play as one, with the row indices of its head and tail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RealLn {
    pub col: u8,
    pub head: usize,
    pub tail: usize,
}

/// Sorted by `(col, head)`.
pub(super) fn real_lns(view: &ChartView<'_>, params: &LnParams) -> Vec<RealLn> {
    let rows = view.rows();
    let mut lns: Vec<RealLn> = view
        .chart()
        .ln_pairs()
        .iter()
        .filter_map(|ln| {
            let head = row_at(rows, ln.head)?;
            let tail = row_at(rows, ln.tail)?;
            let len = ln.tail.0 - ln.head.0;
            let beat = rows.get(head).and_then(|r| r.beat_us);
            let short = len < params.min_len_us
                || beat.is_some_and(|b| ticks(len, b) < params.min_len_ticks);
            (!short).then_some(RealLn {
                col: ln.col,
                head,
                tail,
            })
        })
        .collect();
    lns.sort_by_key(|ln| (ln.col, ln.head));
    lns
}

/// Per row, the columns where a real LN starts (`heads`) or ends (`!heads`).
pub(super) fn masks(k: Keymode, rows: usize, lns: &[RealLn], heads: bool) -> Vec<ColMask> {
    let mut bits = vec![0u16; rows];
    for ln in lns {
        let at = if heads { ln.head } else { ln.tail };
        if let Some(slot) = bits.get_mut(at) {
            *slot |= 1 << ln.col;
        }
    }
    bits.into_iter()
        .map(|b| ColMask::from_bits(k, b).unwrap_or_default())
        .collect()
}

fn row_at(rows: &[RowFeat], t: TimeUs) -> Option<usize> {
    rows.binary_search_by_key(&t, |r| r.t).ok()
}

/// Splits events, each tagged with the row it happens on and sorted by that row, into sections
/// where consecutive events are within the LN group gap.
pub(super) fn group<T>(
    rows: &[RowFeat],
    events: Vec<(usize, T)>,
    params: &LnParams,
) -> Vec<Vec<(usize, T)>> {
    let mut groups: Vec<Vec<(usize, T)>> = Vec::new();
    for (row, event) in events {
        let joins = groups
            .last()
            .and_then(|g| g.last())
            .and_then(|&(prev, _)| Some((rows.get(prev)?, rows.get(row)?)))
            .is_some_and(|(a, b)| {
                gap_within(
                    b.t.0 - a.t.0,
                    b.beat_us,
                    params.group_max_gap_ticks,
                    params.group_max_gap_us,
                )
            });
        match groups.last_mut() {
            Some(g) if joins => g.push((row, event)),
            _ => groups.push(vec![(row, event)]),
        }
    }
    groups
}

/// Held column-time between rows, as prefix sums, so any row span's coverage is O(1).
pub(super) struct Coverage {
    prefix: Vec<i128>,
    columns: i128,
}

impl Coverage {
    /// Between rows `j-1` and `j` the covered columns are exactly those held at `j` or
    /// released at `j`: heads and tails only happen on rows.
    pub(super) fn new(rows: &[RowFeat], k: Keymode) -> Self {
        let mut prefix = Vec::with_capacity(rows.len());
        let mut sum = 0i128;
        let mut prev: Option<TimeUs> = None;
        for r in rows {
            if let Some(p) = prev {
                let cols = (r.held.bits() | r.release.bits()).count_ones();
                sum += i128::from(r.t.0 - p.0) * i128::from(cols);
            }
            prefix.push(sum);
            prev = Some(r.t);
        }
        Self {
            prefix,
            columns: i128::from(k.columns()),
        }
    }

    /// Covered column-time over rows `first..=last` as permille of time × columns.
    pub(super) fn permille(&self, rows: &[RowFeat], first: usize, last: usize) -> u32 {
        let (Some(a), Some(b), Some(ta), Some(tb)) = (
            self.prefix.get(first),
            self.prefix.get(last),
            rows.get(first),
            rows.get(last),
        ) else {
            return 0;
        };
        let whole = i128::from(tb.t.0 - ta.t.0) * self.columns;
        if whole <= 0 {
            return 0;
        }
        u32::try_from(((b - a) * 1000 / whole).clamp(0, 1000)).unwrap_or(0)
    }
}
