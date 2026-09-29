//! `regular.tech.irregular`: off-snap or mixed-snap timing. Over windows of `window_rows`
//! press rows, a row is judged when its press gap has a beat length; it is irregular when it
//! fits no snap divisor, or when it is on the window's minority snap family (triplet divisors,
//! multiples of 3, against the rest; 1/1 belongs to both). Windows whose irregular share
//! reaches `irregular_min_share_permille` merge into candidates.

use wolluf_core::{Keymode, PatternId};

use super::common::{permille, rows_of, span_candidate, window_spans};
use crate::params::PatternParams;
use crate::rule::{Candidate, PatternRule};
use crate::view::{ChartView, RowFeat};

const TRIPLET_FACTOR: u8 = 3;
const WHOLE_BEAT: u8 = 1;

pub(super) const ID: PatternId = PatternId::from_static("regular.tech.irregular");

pub struct Irregular;

impl PatternRule for Irregular {
    fn id(&self) -> PatternId {
        ID
    }

    fn version(&self) -> u32 {
        1
    }

    fn supports(&self, _keymode: Keymode) -> bool {
        true
    }

    /// Strength: the irregular share over the whole span.
    fn detect(&self, view: &ChartView<'_>, params: &PatternParams) -> Vec<Candidate> {
        let p = &params.tech;
        let rows = view.rows();
        let share = |span: &[usize]| irregular_share(&rows_of(rows, span));
        let mut found: Vec<Candidate> =
            window_spans(rows, p.window_rows, p.window_max_gap_us, |w| {
                share(w) >= p.irregular_min_share_permille
            })
            .iter()
            .filter_map(|span| {
                span_candidate(ID, view.keymode(), &rows_of(rows, span), share(span))
            })
            .collect();
        found.sort();
        found
    }
}

fn irregular_share(span: &[&RowFeat]) -> u32 {
    let judged: Vec<&&RowFeat> = span.iter().filter(|r| r.gap_ticks.is_some()).collect();
    let off = judged.iter().filter(|r| r.snap.is_none()).count();
    let triplet = judged
        .iter()
        .filter(|r| r.snap.is_some_and(|d| d % TRIPLET_FACTOR == 0))
        .count();
    let straight = judged
        .iter()
        .filter(|r| {
            r.snap
                .is_some_and(|d| d != WHOLE_BEAT && d % TRIPLET_FACTOR != 0)
        })
        .count();
    let irregular = off + triplet.min(straight);
    permille(
        u64::try_from(irregular).unwrap_or(0),
        u64::try_from(judged.len()).unwrap_or(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::testkit::{cand, detect, seq};

    const ALL: [u8; 7] = [0, 1, 2, 3, 4, 5, 6];

    fn alternating(a: i32, b: i32, n: usize) -> Vec<i32> {
        (1..n).map(|k| if k % 2 == 1 { a } else { b }).collect()
    }

    #[test]
    fn quarter_and_triplet_gaps_mixed_are_irregular() {
        let chart = seq(&alternating(100, 133, 20), &ALL, Some(400.0));
        // 19 judged gaps: 10 on the 1/4 grid, 9 on the 1/3 grid.
        assert_eq!(detect(&Irregular, &chart), [cand(ID, 0, 2197, &ALL, 473)]);
    }

    #[test]
    fn off_snap_gaps_are_irregular() {
        let chart = seq(&alternating(100, 117, 20), &ALL, Some(400.0));
        assert_eq!(detect(&Irregular, &chart), [cand(ID, 0, 2053, &ALL, 473)]);
    }

    #[test]
    fn on_grid_streams_and_untimed_charts_are_regular() {
        assert!(detect(&Irregular, &seq(&[100; 19], &ALL, Some(400.0))).is_empty());
        assert!(detect(&Irregular, &seq(&alternating(100, 133, 20), &ALL, None)).is_empty());
    }

    #[test]
    fn near_misses() {
        let mut gaps = vec![100; 19];
        for k in [4, 9, 14] {
            gaps[k] = 133;
        }
        // At most 3 triplet gaps per 16-row window: 200 permille.
        assert!(detect(&Irregular, &seq(&gaps, &ALL, Some(400.0))).is_empty());
        let short = seq(&alternating(100, 133, 15), &ALL, Some(400.0));
        assert!(detect(&Irregular, &short).is_empty());
    }
}
