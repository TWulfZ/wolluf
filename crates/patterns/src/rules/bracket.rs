//! `regular.stream.bracket`: two or more trills at the same time within one hand (ADR 0017).
//!
//! Per hand bucket of the layout (left, right, and the `Hand::Both` column on its own), each
//! press row is restricted to that hand. A run of consecutive, stream-fast press rows with a
//! non-empty restriction qualifies while every three consecutive restrictions `a, b, c` pass
//! MinaCalc's `is_bracket` test: `a ^ b != 0 && b ^ c != 0 && a & c != 0`. It needs at least
//! `bracket_min_rows` rows, and some restricted row must hold 2+ notes: with one note per row
//! the test only sees a single trill. The other hand is free to do anything meanwhile.

use wolluf_chart::Hand;
use wolluf_core::{ColMask, Keymode, PatternId};

use super::common::{Step, saturating, scan, stream_gap_ok, union};
use crate::params::PatternParams;
use crate::rule::{Candidate, PatternRule};
use crate::view::ChartView;

/// ADR 0017: "two or more trills" needs two fingers of the hand down together somewhere.
const TWO_TRILLS_NOTES: u32 = 2;

pub(super) const ID: PatternId = PatternId::from_static("regular.stream.bracket");

pub struct Bracket;

impl PatternRule for Bracket {
    fn id(&self) -> PatternId {
        ID
    }

    fn version(&self) -> u32 {
        1
    }

    fn supports(&self, _keymode: Keymode) -> bool {
        true
    }

    /// Strength: `saturating(rows, bracket_min_rows)`.
    fn detect(&self, view: &ChartView<'_>, params: &PatternParams) -> Vec<Candidate> {
        let p = &params.stream;
        let rows = view.rows();
        let k = view.keymode();
        let mut found = Vec::new();
        for hand in [Hand::Left, Hand::Right, Hand::Both] {
            let cols = view.hand_cols().get(hand);
            let restrict = |i: usize| {
                rows.get(i).map_or(ColMask::EMPTY, |r| {
                    ColMask::from_bits(k, r.press.bits() & cols.bits()).unwrap_or_default()
                })
            };
            let runs = scan(rows, |run, i| {
                if restrict(i).is_empty() {
                    return Step::Break;
                }
                let fast = rows.get(i).is_some_and(|r| stream_gap_ok(r, p));
                if run.is_empty() || !fast {
                    return Step::Restart;
                }
                let tail = run.len().checked_sub(2).and_then(|k| run.get(k..));
                match tail {
                    Some(&[a, b]) if !is_bracket(restrict(a), restrict(b), restrict(i)) => {
                        Step::RestartFromPrev
                    }
                    _ => Step::Extend,
                }
            });
            for run in runs {
                let masks: Vec<ColMask> = run.iter().map(|&i| restrict(i)).collect();
                let long_enough = u32::try_from(masks.len()).is_ok_and(|n| n >= p.bracket_min_rows);
                if !long_enough || masks.iter().all(|m| m.len() < TWO_TRILLS_NOTES) {
                    continue;
                }
                let (Some(first), Some(last)) = (
                    run.first().and_then(|&i| rows.get(i)),
                    run.last().and_then(|&i| rows.get(i)),
                ) else {
                    continue;
                };
                found.push(Candidate {
                    pattern: ID,
                    t0: first.t,
                    t1: last.t,
                    cols: union(k, masks.iter().copied()),
                    strength: saturating(masks.len(), p.bracket_min_rows),
                });
            }
        }
        found.sort();
        found
    }
}

/// MinaCalc `is_bracket`, over one hand's columns.
fn is_bracket(a: ColMask, b: ColMask, c: ColMask) -> bool {
    let (a, b, c) = (a.bits(), b.bits(), c.bits());
    a ^ b != 0 && b ^ c != 0 && a & c != 0
}

#[cfg(test)]
mod tests {
    use wolluf_chart::{Layout, chart};

    use super::*;
    use crate::rules::testkit::{cand, detect_with};

    fn layout(id: &str) -> Layout {
        Layout::by_id(id).unwrap()
    }

    #[test]
    fn two_trills_in_one_hand_are_a_bracket() {
        let chart = chart![step = 100; "x.x....", ".x.....", "x.x....", ".x....."];
        for id in ["k7.313_right_thumb", "k7.313_left_thumb"] {
            assert_eq!(
                detect_with(&Bracket, &chart, &layout(id)),
                [cand(ID, 0, 300, &[0, 1, 2], 500)],
                "{id}"
            );
        }
    }

    #[test]
    fn the_thumb_side_decides_a_centre_bracket() {
        let chart = chart![step = 100; ".x.x...", "..x....", ".x.x...", "..x...."];
        assert!(detect_with(&Bracket, &chart, &layout("k7.313_right_thumb")).is_empty());
        assert_eq!(
            detect_with(&Bracket, &chart, &layout("k7.313_left_thumb")),
            [cand(ID, 0, 300, &[1, 2, 3], 500)]
        );
    }

    #[test]
    fn a_single_trill_is_not_a_bracket() {
        let chart = chart![step = 100; "x......", ".x.....", "x......", ".x....."];
        assert!(detect_with(&Bracket, &chart, &layout("k7.313_right_thumb")).is_empty());
    }

    #[test]
    fn near_misses() {
        let short = chart![step = 100; "x.x....", ".x.....", "x.x...."];
        assert!(detect_with(&Bracket, &short, &layout("k7.313_right_thumb")).is_empty());
        let idle_hand = chart![step = 100; "x.x....", ".x.....", "x.x....", ".....x."];
        assert!(detect_with(&Bracket, &idle_hand, &layout("k7.313_right_thumb")).is_empty());
    }
}
