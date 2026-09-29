//! `regular.tech.hand_imbalance`: density loaded on one hand. Over windows of `window_rows`
//! press rows, the heavier hand's share of left + right presses must reach
//! `hand_imbalance_min_share_permille`. Hands come from the user's layout; presses on the
//! `Hand::Both` column are left out of the ratio. This share is `(1 + HandBalance) / 2` with
//! MinaCalc's `HandBalance = |L − R| / (L + R)` (research 02 l.44).

use wolluf_chart::Hand;
use wolluf_core::{ColMask, Keymode, PatternId};

use super::common::{permille, rows_of, union, window_spans};
use crate::params::PatternParams;
use crate::rule::{Candidate, PatternRule};
use crate::view::{ChartView, RowFeat};

pub(super) const ID: PatternId = PatternId::from_static("regular.tech.hand_imbalance");

pub struct HandImbalance;

impl PatternRule for HandImbalance {
    fn id(&self) -> PatternId {
        ID
    }

    fn version(&self) -> u32 {
        1
    }

    fn supports(&self, _keymode: Keymode) -> bool {
        true
    }

    /// Strength: the heavy hand's share over the whole span. `cols` are that hand's pressed
    /// columns.
    fn detect(&self, view: &ChartView<'_>, params: &PatternParams) -> Vec<Candidate> {
        let p = &params.tech;
        let rows = view.rows();
        let k = view.keymode();
        let hands = view.hand_cols();
        let mut found = Vec::new();
        for (heavy, light) in [(Hand::Left, Hand::Right), (Hand::Right, Hand::Left)] {
            let (heavy, light) = (hands.get(heavy), hands.get(light));
            let share = |span: &[usize]| hand_share(&rows_of(rows, span), heavy, light);
            for span in window_spans(rows, p.window_rows, p.window_max_gap_us, |w| {
                share(w) >= p.hand_imbalance_min_share_permille
            }) {
                let feats = rows_of(rows, &span);
                let (Some(first), Some(last)) = (feats.first(), feats.last()) else {
                    continue;
                };
                found.push(Candidate {
                    pattern: ID,
                    t0: first.t,
                    t1: last.t,
                    cols: union(k, feats.iter().map(|r| and(k, r.press, heavy))),
                    strength: share(&span),
                });
            }
        }
        found.sort();
        found
    }
}

fn hand_share(span: &[&RowFeat], heavy: ColMask, light: ColMask) -> u32 {
    let count = |m: ColMask| -> u64 {
        span.iter()
            .map(|r| u64::from((r.press.bits() & m.bits()).count_ones()))
            .sum()
    };
    let (h, l) = (count(heavy), count(light));
    permille(h, h + l)
}

fn and(k: Keymode, a: ColMask, b: ColMask) -> ColMask {
    ColMask::from_bits(k, a.bits() & b.bits()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use wolluf_chart::Chart;

    use super::*;
    use crate::rules::testkit::{cand, detect, detect_with, layout, taps};

    fn cycle(cols: &[u8], n: usize) -> Chart {
        let seq: Vec<u8> = cols.iter().copied().cycle().take(n).collect();
        seq_of(&seq)
    }

    fn seq_of(cols: &[u8]) -> Chart {
        let notes: Vec<(i32, u8)> = cols
            .iter()
            .enumerate()
            .map(|(i, &c)| (i as i32 * 100, c))
            .collect();
        taps(&notes, None)
    }

    #[test]
    fn a_section_on_one_hand_is_imbalanced() {
        let chart = cycle(&[0, 1, 2], 16);
        assert_eq!(
            detect(&HandImbalance, &chart),
            [cand(ID, 0, 1500, &[0, 1, 2], 1000)]
        );
    }

    #[test]
    fn the_thumb_side_decides() {
        let chart = cycle(&[0, 3, 1, 3, 2, 3], 18);
        assert!(detect_with(&HandImbalance, &chart, &layout("k7.313_right_thumb")).is_empty());
        assert_eq!(
            detect_with(&HandImbalance, &chart, &layout("k7.313_left_thumb")),
            [cand(ID, 0, 1700, &[0, 1, 2, 3], 1000)]
        );
    }

    #[test]
    fn a_shared_thumb_column_is_left_out_of_the_ratio() {
        let chart = cycle(&[0, 3, 1, 3, 2, 3], 18);
        assert_eq!(
            detect_with(&HandImbalance, &chart, &layout("k7.both_thumbs")),
            [cand(ID, 0, 1700, &[0, 1, 2], 1000)]
        );
    }

    #[test]
    fn balanced_sections_are_not_imbalanced() {
        assert!(detect(&HandImbalance, &cycle(&[0, 4, 1, 5, 2, 6], 18)).is_empty());
    }

    #[test]
    fn share_just_under_and_just_over() {
        let eleven = [0, 4, 1, 5, 2, 6, 0, 4, 1, 5, 2, 0, 1, 2, 0, 1];
        let under: Vec<u8> = eleven.iter().chain(&eleven).copied().collect();
        assert!(detect(&HandImbalance, &seq_of(&under)).is_empty());
        let twelve = [0, 4, 1, 5, 2, 2, 0, 4, 1, 5, 2, 0, 1, 2, 0, 1];
        let over: Vec<u8> = twelve.iter().chain(&twelve).copied().collect();
        assert_eq!(
            detect(&HandImbalance, &seq_of(&over)),
            [cand(ID, 0, 3100, &[0, 1, 2], 750)]
        );
        assert!(detect(&HandImbalance, &cycle(&[0, 1, 2], 15)).is_empty());
    }
}
