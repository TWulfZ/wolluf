//! `regular.jack.minijack`: exactly two consecutive presses in one column (ADR 0017).
//! Consecutive means on successive press rows, each link jack-fast; see
//! [`column_runs`](super::common::column_runs).

use wolluf_core::{Keymode, PatternId};

use super::common::{column_runs, notes_between, permille, single};
use crate::params::PatternParams;
use crate::rule::{Candidate, PatternRule};
use crate::view::ChartView;

pub(super) const ID: PatternId = PatternId::from_static("regular.jack.minijack");

/// ADR 0017: a minijack is exactly 2 presses; 3+ is a longjack.
const MINIJACK_PRESSES: u32 = 2;

pub struct Minijack;

impl PatternRule for Minijack {
    fn id(&self) -> PatternId {
        ID
    }

    fn version(&self) -> u32 {
        1
    }

    fn supports(&self, _keymode: Keymode) -> bool {
        true
    }

    /// Strength: the jack's presses as a share of all presses on the span's rows.
    fn detect(&self, view: &ChartView<'_>, params: &PatternParams) -> Vec<Candidate> {
        let rows = view.rows();
        let mut found: Vec<Candidate> = column_runs(rows, view.keymode(), &params.jack)
            .into_iter()
            .filter(|run| run.len == MINIJACK_PRESSES)
            .filter_map(|run| {
                Some(Candidate {
                    pattern: ID,
                    t0: rows.get(run.first)?.t,
                    t1: rows.get(run.last)?.t,
                    cols: single(view.keymode(), run.col),
                    strength: permille(
                        u64::from(run.len),
                        notes_between(rows, run.first, run.last),
                    ),
                })
            })
            .collect();
        found.sort();
        found
    }
}

#[cfg(test)]
mod tests {
    use wolluf_chart::chart;

    use super::*;
    use crate::rules::testkit::{cand, detect, detect_both_thumbs, taps, with_beat};

    #[test]
    fn two_presses_in_one_column_are_a_minijack() {
        let chart = chart![step = 100;
            "x......",
            "x......",
            ".x.....",
        ];
        assert_eq!(detect(&Minijack, &chart), [cand(ID, 0, 100, &[0], 1000)]);
    }

    #[test]
    fn strength_is_the_jack_share_of_the_span_notes() {
        let chart = chart![step = 100;
            "x.x....",
            "x..x...",
        ];
        assert_eq!(detect(&Minijack, &chart), [cand(ID, 0, 100, &[0], 500)]);
    }

    #[test]
    fn streams_and_longer_runs_are_not_minijacks() {
        let stream = chart![step = 100;
            "x......",
            ".x.....",
            "..x....",
            "x......",
        ];
        assert!(detect(&Minijack, &stream).is_empty());
        let run = chart![step = 100;
            "x......",
            "x......",
            "x......",
        ];
        assert!(detect(&Minijack, &run).is_empty());
    }

    #[test]
    fn gap_just_over_the_threshold_is_not_a_jack() {
        let at = chart![step = 500; "x......", "x......"];
        assert_eq!(detect(&Minijack, &at).len(), 1);
        let over = chart![step = 501; "x......", "x......"];
        assert!(detect(&Minijack, &over).is_empty());
    }

    #[test]
    fn gap_over_one_beat_is_not_a_jack() {
        let one_beat = with_beat(&chart![step = 400; "x......", "x......"], 400.0);
        assert_eq!(detect(&Minijack, &one_beat).len(), 1);
        let longer = with_beat(&chart![step = 450; "x......", "x......"], 400.0);
        assert!(detect(&Minijack, &longer).is_empty());
    }

    #[test]
    fn a_row_without_the_column_splits_a_run_into_two_minijacks() {
        let chart = chart![step = 100;
            "x......",
            "x......",
            ".x.....",
            "x......",
            "x......",
        ];
        assert_eq!(
            detect(&Minijack, &chart),
            [cand(ID, 0, 100, &[0], 1000), cand(ID, 300, 400, &[0], 1000)]
        );
    }

    #[test]
    fn a_release_only_row_does_not_break_a_jack() {
        let chart = chart![step = 100;
            "x..[...",
            "...]...",
            "x......",
        ];
        assert_eq!(detect(&Minijack, &chart), [cand(ID, 0, 200, &[0], 666)]);
    }

    #[test]
    fn a_minijack_whose_third_gap_is_too_slow_stays_a_minijack() {
        let chart = taps(&[(0, 0), (400, 0), (901, 0)], None);
        assert_eq!(detect(&Minijack, &chart), [cand(ID, 0, 400, &[0], 1000)]);
    }

    #[test]
    fn thumb_column_jack_under_both_313_layouts() {
        let chart = chart![step = 100; "...x...", "...x..."];
        assert_eq!(
            detect_both_thumbs(&Minijack, &chart),
            [cand(ID, 0, 100, &[3], 1000)]
        );
    }
}
