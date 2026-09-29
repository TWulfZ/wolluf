//! `regular.jack.anchor`: one column recurring every other press row under a stream.
//!
//! Per column, successive presses link when they are 2 to `anchor_max_period_rows` press rows
//! apart (1 apart is a jack) and jack-fast; a chain of at least `anchor_min_hits` presses is an
//! anchor. A chain whose in-between rows all press the same columns is skipped: that is a
//! trill or jumptrill (ADR 0017), not a column anchored under a stream.

use wolluf_core::{Keymode, PatternId};

use super::common::{jack_gap_ok, permille, press_rows, single};
use crate::params::{JackParams, PatternParams};
use crate::rule::{Candidate, PatternRule};
use crate::view::{ChartView, RowFeat};

pub(super) const ID: PatternId = PatternId::from_static("regular.jack.anchor");

/// The period the taxonomy names ("every other row"); a tighter spacing is a jack.
const MIN_PERIOD_ROWS: usize = 2;

pub struct Anchor;

impl PatternRule for Anchor {
    fn id(&self) -> PatternId {
        ID
    }

    fn version(&self) -> u32 {
        1
    }

    fn supports(&self, _keymode: Keymode) -> bool {
        true
    }

    /// Strength: `(2 × hits − 1) / press rows in the span`, i.e. 1000 for a strict period of 2
    /// and lower as the spacing loosens.
    fn detect(&self, view: &ChartView<'_>, params: &PatternParams) -> Vec<Candidate> {
        let p = &params.jack;
        let rows = view.rows();
        let presses = press_rows(rows);
        let mut found = Vec::new();
        for col in 0..view.keymode().columns() {
            // Ordinals into `presses` of the rows pressing `col`.
            let hits: Vec<usize> = presses
                .iter()
                .enumerate()
                .filter(|&(_, &i)| rows.get(i).is_some_and(|r| r.press.contains(col)))
                .map(|(ord, _)| ord)
                .collect();
            let mut chain: Vec<usize> = Vec::new();
            for &ord in &hits {
                let links = chain.last().is_some_and(|&prev| {
                    let apart = ord - prev;
                    let row = presses.get(ord).and_then(|&i| rows.get(i));
                    (MIN_PERIOD_ROWS..=period_cap(p)).contains(&apart)
                        && row.is_some_and(|r| {
                            r.col_gap_us
                                .get(usize::from(col))
                                .copied()
                                .flatten()
                                .is_some_and(|gap| jack_gap_ok(gap, r.beat_us, p))
                        })
                });
                if !links {
                    found.extend(close(&mut chain, &presses, rows, view.keymode(), col, p));
                }
                chain.push(ord);
            }
            found.extend(close(&mut chain, &presses, rows, view.keymode(), col, p));
        }
        found.sort();
        found
    }
}

fn period_cap(p: &JackParams) -> usize {
    usize::try_from(p.anchor_max_period_rows).unwrap_or(usize::MAX)
}

fn close(
    chain: &mut Vec<usize>,
    presses: &[usize],
    rows: &[RowFeat],
    k: Keymode,
    col: u8,
    p: &JackParams,
) -> Option<Candidate> {
    let hits: Vec<usize> = std::mem::take(chain);
    let (&first, &last) = (hits.first()?, hits.last()?);
    if u32::try_from(hits.len()).map_or(true, |n| n < p.anchor_min_hits) {
        return None;
    }
    let between: Vec<_> = (first..=last)
        .filter(|ord| hits.binary_search(ord).is_err())
        .filter_map(|ord| presses.get(ord).and_then(|&i| rows.get(i)))
        .map(|r| r.press)
        .collect();
    if between.windows(2).all(|w| w[0] == w[1]) {
        return None;
    }
    let span_rows = last - first + 1;
    let hit_count = hits.len();
    Some(Candidate {
        pattern: ID,
        t0: rows.get(*presses.get(first)?)?.t,
        t1: rows.get(*presses.get(last)?)?.t,
        cols: single(k, col),
        strength: permille(
            u64::try_from(2 * hit_count - 1).unwrap_or(0),
            u64::try_from(span_rows).unwrap_or(0),
        ),
    })
}

#[cfg(test)]
mod tests {
    use wolluf_chart::chart;

    use super::*;
    use crate::rules::testkit::{cand, detect, detect_both_thumbs};

    #[test]
    fn a_column_on_every_other_row_of_a_stream_is_an_anchor() {
        let chart = chart![step = 100;
            "x......",
            "..x....",
            "x......",
            "...x...",
            "x......",
            "....x..",
            "x......",
        ];
        assert_eq!(detect(&Anchor, &chart), [cand(ID, 0, 600, &[0], 1000)]);
    }

    #[test]
    fn plain_streams_and_trills_are_not_anchors() {
        let stream = chart![step = 100;
            "x......",
            ".x.....",
            "..x....",
            "...x...",
            "....x..",
            ".....x.",
            "......x",
        ];
        assert!(detect(&Anchor, &stream).is_empty());
        let trill = chart![step = 100;
            "x......",
            ".x.....",
            "x......",
            ".x.....",
            "x......",
            ".x.....",
            "x......",
            ".x.....",
        ];
        assert!(detect(&Anchor, &trill).is_empty());
    }

    #[test]
    fn three_hits_are_too_few() {
        let chart = chart![step = 100;
            "x......",
            "..x....",
            "x......",
            "...x...",
            "x......",
            "....x..",
        ];
        assert!(detect(&Anchor, &chart).is_empty());
    }

    #[test]
    fn every_third_row_is_not_an_anchor() {
        let chart = chart![step = 100;
            "x......",
            "..x....",
            "...x...",
            "x......",
            "....x..",
            "..x....",
            "x......",
            "...x...",
            "....x..",
            "x......",
        ];
        assert!(detect(&Anchor, &chart).is_empty());
    }

    #[test]
    fn hit_gap_just_over_the_threshold_misses() {
        let rows = [
            "x......", "..x....", "x......", "...x...", "x......", "....x..", "x......",
        ];
        let at = wolluf_chart::testkit::chart_from_rows(0, 250, &rows).unwrap();
        assert_eq!(detect(&Anchor, &at).len(), 1);
        let over = wolluf_chart::testkit::chart_from_rows(0, 251, &rows).unwrap();
        assert!(detect(&Anchor, &over).is_empty());
    }

    #[test]
    fn a_jack_in_the_column_breaks_the_anchor() {
        let chart = chart![step = 100;
            "x......",
            "..x....",
            "x......",
            "x..x...",
            "x......",
            "....x..",
            "x......",
        ];
        assert!(detect(&Anchor, &chart).is_empty());
    }

    #[test]
    fn release_only_rows_are_not_counted() {
        let chart = chart![step = 100;
            "x..[...",
            "...]...",
            "..x....",
            "x......",
            ".x.....",
            "x......",
            "....x..",
            "x......",
        ];
        assert_eq!(detect(&Anchor, &chart), [cand(ID, 0, 700, &[0], 1000)]);
    }

    #[test]
    fn thumb_column_anchor_under_both_313_layouts() {
        let chart = chart![step = 100;
            "...x...",
            "x......",
            "...x...",
            "......x",
            "...x...",
            ".x.....",
            "...x...",
        ];
        assert_eq!(
            detect_both_thumbs(&Anchor, &chart),
            [cand(ID, 0, 600, &[3], 1000)]
        );
    }
}
