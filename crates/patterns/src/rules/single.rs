//! `regular.stream.single`: a jackless, stream-fast run of single-note rows (Interlude prelude
//! `Core.STREAM`, MIT, see NOTICE: 5 rows). A run on only two columns is a trill and a run whose
//! every move goes one way is a stair; both are left to their own rules (ADR 0017).

use wolluf_core::{Keymode, PatternId};

use super::common::{permille, rows_of, span_candidate, stream_runs};
use crate::params::PatternParams;
use crate::rule::{Candidate, PatternRule};
use crate::view::{ChartView, Direction};

pub(super) const ID: PatternId = PatternId::from_static("regular.stream.single");

const SINGLE_NOTES: u32 = 1;
/// ADR 0017: two columns alternating is a trill.
const TRILL_COLUMNS: u32 = 2;

pub struct Single;

impl PatternRule for Single {
    fn id(&self) -> PatternId {
        ID
    }

    fn version(&self) -> u32 {
        1
    }

    fn supports(&self, _keymode: Keymode) -> bool {
        true
    }

    /// Strength: direction changes between successive moves over the number of move pairs, so
    /// zigzags score high and roll-like runs low.
    fn detect(&self, view: &ChartView<'_>, params: &PatternParams) -> Vec<Candidate> {
        let p = &params.stream;
        let rows = view.rows();
        let mut found: Vec<Candidate> = stream_runs(rows, p, |r| r.notes == SINGLE_NOTES)
            .iter()
            .filter_map(|run| {
                let span = rows_of(rows, run);
                if u32::try_from(span.len()).map_or(true, |n| n < p.min_rows) {
                    return None;
                }
                let moves: Vec<Direction> = span.iter().skip(1).map(|r| r.direction).collect();
                let turns = moves.windows(2).filter(|w| w[0] != w[1]).count();
                let columns = span
                    .iter()
                    .fold(0u16, |acc, r| acc | r.press.bits())
                    .count_ones();
                if columns <= TRILL_COLUMNS || turns == 0 {
                    return None;
                }
                let pairs = moves.len().saturating_sub(1);
                let strength = permille(
                    u64::try_from(turns).unwrap_or(0),
                    u64::try_from(pairs).unwrap_or(0),
                );
                span_candidate(ID, view.keymode(), &span, strength)
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
    fn a_jackless_zigzag_of_single_notes_is_a_stream() {
        let chart = chart![step = 100;
            "x......",
            "..x....",
            ".x.....",
            "...x...",
            "..x....",
            "....x..",
        ];
        assert_eq!(
            detect(&Single, &chart),
            [cand(ID, 0, 500, &[0, 1, 2, 3, 4], 1000)]
        );
    }

    #[test]
    fn strength_drops_as_the_run_rolls() {
        let chart = chart![step = 100;
            "x......",
            ".x.....",
            "..x....",
            ".x.....",
            "..x....",
            "...x...",
        ];
        // Directions R R L R R: two changes over four link pairs.
        assert_eq!(
            detect(&Single, &chart),
            [cand(ID, 0, 500, &[0, 1, 2, 3], 500)]
        );
    }

    #[test]
    fn trills_stairs_and_chords_are_not_single_streams() {
        let trill = chart![step = 100; "x......", ".x.....", "x......", ".x.....", "x......"];
        assert!(detect(&Single, &trill).is_empty());
        let stair = chart![step = 100; "x......", ".x.....", "..x....", "...x...", "....x.."];
        assert!(detect(&Single, &stair).is_empty());
        let jumps =
            chart![step = 100; "x......", "..xx...", ".x.....", "...x...", "..x....", "....x.."];
        assert!(detect(&Single, &jumps).is_empty());
    }

    #[test]
    fn near_misses() {
        let short = chart![step = 100; "x......", "..x....", ".x.....", "...x..."];
        assert!(detect(&Single, &short).is_empty());
        let rows = ["x......", "..x....", ".x.....", "...x...", "..x...."];
        let at = wolluf_chart::testkit::chart_from_rows(0, 250, &rows).unwrap();
        assert_eq!(detect(&Single, &at).len(), 1);
        let slow = wolluf_chart::testkit::chart_from_rows(0, 251, &rows).unwrap();
        assert!(detect(&Single, &slow).is_empty());
        let jacked =
            chart![step = 100; "x......", "..x....", ".x.....", ".x.....", "...x...", "..x...."];
        assert!(detect(&Single, &jacked).is_empty());
    }
}
