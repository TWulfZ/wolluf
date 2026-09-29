//! `regular.stream.trill`: two columns alternating, single notes `a b a b …`, stream-fast, at
//! least `trill_min_rows` rows (Interlude prelude `Stream_4K.TRILL`, MIT, see NOTICE: 4 rows).

use wolluf_core::{Keymode, PatternId};

use super::common::{alternations, rows_of, saturating, span_candidate};
use crate::params::PatternParams;
use crate::rule::{Candidate, PatternRule};
use crate::view::ChartView;

pub(super) const ID: PatternId = PatternId::from_static("regular.stream.trill");

const SINGLE_NOTES: u32 = 1;

pub struct Trill;

impl PatternRule for Trill {
    fn id(&self) -> PatternId {
        ID
    }

    fn version(&self) -> u32 {
        1
    }

    fn supports(&self, _keymode: Keymode) -> bool {
        true
    }

    /// Strength: `saturating(rows, trill_min_rows)`.
    fn detect(&self, view: &ChartView<'_>, params: &PatternParams) -> Vec<Candidate> {
        let p = &params.stream;
        let rows = view.rows();
        let mut found: Vec<Candidate> = alternations(rows, p, |r| r.notes == SINGLE_NOTES)
            .iter()
            .filter_map(|run| {
                let span = rows_of(rows, run);
                if u32::try_from(span.len()).map_or(true, |n| n < p.trill_min_rows) {
                    return None;
                }
                span_candidate(
                    ID,
                    view.keymode(),
                    &span,
                    saturating(span.len(), p.trill_min_rows),
                )
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
    use crate::rules::testkit::{cand, detect};

    #[test]
    fn two_columns_alternating_are_a_trill() {
        let chart = chart![step = 100; "x......", ".x.....", "x......", ".x....."];
        assert_eq!(detect(&Trill, &chart), [cand(ID, 0, 300, &[0, 1], 500)]);
        let long =
            chart![step = 100; "..x...x", "......x", "..x....", "......x", "..x....", "......x"];
        assert_eq!(detect(&Trill, &long), [cand(ID, 100, 500, &[2, 6], 625)]);
    }

    #[test]
    fn streams_and_chord_alternations_are_not_trills() {
        let stream = chart![step = 100; "x......", ".x.....", "..x....", "...x..."];
        assert!(detect(&Trill, &stream).is_empty());
        let jumptrill = chart![step = 100; "xx.....", "..xx...", "xx.....", "..xx..."];
        assert!(detect(&Trill, &jumptrill).is_empty());
    }

    #[test]
    fn near_misses() {
        let short = chart![step = 100; "x......", ".x.....", "x......"];
        assert!(detect(&Trill, &short).is_empty());
        let broken = chart![step = 100; "x......", ".x.....", "x......", "..x....", "x......"];
        assert!(detect(&Trill, &broken).is_empty());
        let slow = chart![step = 251; "x......", ".x.....", "x......", ".x....."];
        assert!(detect(&Trill, &slow).is_empty());
    }
}
