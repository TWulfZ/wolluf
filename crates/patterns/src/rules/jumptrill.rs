//! `regular.stream.jumptrill`: two disjoint chords alternating, stream-fast, at least
//! `jumptrill_min_rows` rows (Interlude prelude `Chordstream_4K.JUMPTRILL`, MIT, see NOTICE).
//! Chords of any size count, including the alternating 4+-note chords ADR 0017 files here.

use wolluf_core::{Keymode, PatternId};

use super::common::{alternations, rows_of, saturating, span_candidate};
use crate::params::PatternParams;
use crate::rule::{Candidate, PatternRule};
use crate::view::ChartView;

pub(super) const ID: PatternId = PatternId::from_static("regular.stream.jumptrill");

/// ADR 0017: both sides are chords (2+ notes); a single note against a chord is not a jumptrill.
const CHORD_MIN_NOTES: u32 = 2;

pub struct Jumptrill;

impl PatternRule for Jumptrill {
    fn id(&self) -> PatternId {
        ID
    }

    fn version(&self) -> u32 {
        1
    }

    fn supports(&self, _keymode: Keymode) -> bool {
        true
    }

    /// Strength: `saturating(rows, jumptrill_min_rows)`.
    fn detect(&self, view: &ChartView<'_>, params: &PatternParams) -> Vec<Candidate> {
        let p = &params.stream;
        let rows = view.rows();
        let mut found: Vec<Candidate> = alternations(rows, p, |r| r.notes >= CHORD_MIN_NOTES)
            .iter()
            .filter_map(|run| {
                let span = rows_of(rows, run);
                if u32::try_from(span.len()).map_or(true, |n| n < p.jumptrill_min_rows) {
                    return None;
                }
                span_candidate(
                    ID,
                    view.keymode(),
                    &span,
                    saturating(span.len(), p.jumptrill_min_rows),
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
    fn two_chords_alternating_are_a_jumptrill() {
        let chart = chart![step = 100; "xx.....", "..xx...", "xx.....", "..xx..."];
        assert_eq!(
            detect(&Jumptrill, &chart),
            [cand(ID, 0, 300, &[0, 1, 2, 3], 500)]
        );
    }

    #[test]
    fn chords_of_more_than_four_notes_alternating_are_a_jumptrill() {
        let chart = chart![step = 100; "xxxxx..", ".....xx", "xxxxx..", ".....xx"];
        assert_eq!(
            detect(&Jumptrill, &chart),
            [cand(ID, 0, 300, &[0, 1, 2, 3, 4, 5, 6], 500)]
        );
    }

    #[test]
    fn single_trills_and_jacking_chords_are_not_jumptrills() {
        let trill = chart![step = 100; "x......", ".x.....", "x......", ".x....."];
        assert!(detect(&Jumptrill, &trill).is_empty());
        let jacked = chart![step = 100; "xx.....", ".xx....", "xx.....", ".xx...."];
        assert!(detect(&Jumptrill, &jacked).is_empty());
    }

    #[test]
    fn near_misses() {
        let short = chart![step = 100; "xx.....", "..xx...", "xx....."];
        assert!(detect(&Jumptrill, &short).is_empty());
        let one_side_single = chart![step = 100; "x......", "..xx...", "x......", "..xx..."];
        assert!(detect(&Jumptrill, &one_side_single).is_empty());
    }
}
