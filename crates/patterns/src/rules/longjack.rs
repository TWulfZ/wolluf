//! `regular.jack.longjack`: three or more consecutive presses in one column (ADR 0017), one
//! candidate per maximal run; see [`column_runs`](super::common::column_runs).

use wolluf_core::{Keymode, PatternId};

use super::common::{column_runs, notes_between, permille, single};
use crate::params::PatternParams;
use crate::rule::{Candidate, PatternRule};
use crate::view::ChartView;

pub(super) const ID: PatternId = PatternId::from_static("regular.jack.longjack");

/// ADR 0017: a longjack is 3+ presses.
const LONGJACK_MIN_PRESSES: u32 = 3;

pub struct Longjack;

impl PatternRule for Longjack {
    fn id(&self) -> PatternId {
        ID
    }

    fn version(&self) -> u32 {
        1
    }

    fn supports(&self, _keymode: Keymode) -> bool {
        true
    }

    /// Strength: the run's presses as a share of all presses on the span's rows.
    fn detect(&self, view: &ChartView<'_>, params: &PatternParams) -> Vec<Candidate> {
        let rows = view.rows();
        let mut found: Vec<Candidate> = column_runs(rows, view.keymode(), &params.jack)
            .into_iter()
            .filter(|run| run.len >= LONGJACK_MIN_PRESSES)
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
    use crate::rules::testkit::{cand, detect, detect_both_thumbs, taps};

    #[test]
    fn three_or_more_presses_in_one_column_are_one_longjack() {
        let chart = chart![step = 100;
            "x......",
            "x......",
            "x......",
            "x......",
        ];
        assert_eq!(detect(&Longjack, &chart), [cand(ID, 0, 300, &[0], 1000)]);
    }

    #[test]
    fn parallel_runs_are_separate_candidates_in_time_order() {
        let chart = chart![step = 100;
            "x......",
            "x.....x",
            "x.....x",
            "......x",
        ];
        assert_eq!(
            detect(&Longjack, &chart),
            [cand(ID, 0, 200, &[0], 600), cand(ID, 100, 300, &[6], 600)]
        );
    }

    #[test]
    fn two_presses_are_not_a_longjack() {
        let chart = chart![step = 100; "x......", "x......", ".x....."];
        assert!(detect(&Longjack, &chart).is_empty());
    }

    #[test]
    fn a_slow_link_ends_the_run() {
        let chart = taps(&[(0, 0), (400, 0), (901, 0)], None);
        assert!(detect(&Longjack, &chart).is_empty());
        let fast = taps(&[(0, 0), (400, 0), (900, 0)], None);
        assert_eq!(detect(&Longjack, &fast), [cand(ID, 0, 900, &[0], 1000)]);
    }

    #[test]
    fn a_row_without_the_column_breaks_the_run() {
        let chart = chart![step = 100;
            "x......",
            "x......",
            ".x.....",
            "x......",
            "x......",
            "x......",
        ];
        assert_eq!(detect(&Longjack, &chart), [cand(ID, 300, 500, &[0], 1000)]);
    }

    #[test]
    fn a_release_only_row_does_not_break_the_run() {
        let chart = chart![step = 100;
            "x..[...",
            "x..]...",
            "x......",
        ];
        assert_eq!(detect(&Longjack, &chart), [cand(ID, 0, 200, &[0], 750)]);
    }

    #[test]
    fn thumb_column_longjack_under_both_313_layouts() {
        let chart = chart![step = 100; "...x...", "...x...", "...x..."];
        assert_eq!(
            detect_both_thumbs(&Longjack, &chart),
            [cand(ID, 0, 200, &[3], 1000)]
        );
    }
}
