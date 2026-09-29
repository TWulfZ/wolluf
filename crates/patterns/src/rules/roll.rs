//! `regular.stream.roll`: rolls and stairs. At least `roll_min_rows` consecutive moves in the
//! same direction (Interlude prelude `Stream_4K.ROLL` / `CHORD_ROLL`, MIT, see NOTICE), each move
//! stream-fast and an Interlude roll (the new row lies wholly on one side of the previous one).
//! Single-note stairs and chord rolls both qualify; interleaved chord shapes are chordbrackets.

use wolluf_core::{Keymode, PatternId};

use super::common::{Step, rows_of, saturating, scan, span_candidate, stream_gap_ok};
use crate::params::PatternParams;
use crate::rule::{Candidate, PatternRule};
use crate::view::{ChartView, Direction};

pub(super) const ID: PatternId = PatternId::from_static("regular.stream.roll");

pub struct Roll;

impl PatternRule for Roll {
    fn id(&self) -> PatternId {
        ID
    }

    fn version(&self) -> u32 {
        1
    }

    fn supports(&self, _keymode: Keymode) -> bool {
        true
    }

    /// Strength: `saturating(moves, roll_min_rows)`.
    fn detect(&self, view: &ChartView<'_>, params: &PatternParams) -> Vec<Candidate> {
        let p = &params.stream;
        let rows = view.rows();
        let runs = scan(rows, |run, i| {
            let Some(row) = rows.get(i) else {
                return Step::Break;
            };
            let moving = row.is_roll
                && matches!(row.direction, Direction::Left | Direction::Right)
                && stream_gap_ok(row, p);
            if run.is_empty() || !moving {
                return Step::Restart;
            }
            let heading = run.get(1).and_then(|&k| rows.get(k)).map(|r| r.direction);
            match heading {
                Some(d) if d != row.direction => Step::RestartFromPrev,
                _ => Step::Extend,
            }
        });
        let mut found: Vec<Candidate> = runs
            .iter()
            .filter_map(|run| {
                let span = rows_of(rows, run);
                let moves = span.len().saturating_sub(1);
                if u32::try_from(moves).map_or(true, |n| n < p.roll_min_rows) {
                    return None;
                }
                span_candidate(
                    ID,
                    view.keymode(),
                    &span,
                    saturating(moves, p.roll_min_rows),
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
    fn a_single_note_stair_is_a_roll() {
        let chart = chart![step = 100; "x......", ".x.....", "..x....", "...x..."];
        assert_eq!(
            detect(&Roll, &chart),
            [cand(ID, 0, 300, &[0, 1, 2, 3], 500)]
        );
    }

    #[test]
    fn chord_rolls_and_zigzags() {
        let chords = chart![step = 100; "xx.....", "..xx...", "....xx.", "......x"];
        assert_eq!(
            detect(&Roll, &chords),
            [cand(ID, 0, 300, &[0, 1, 2, 3, 4, 5, 6], 500)]
        );
        let zigzag = chart![step = 100;
            "x......", ".x.....", "..x....", "...x...", "..x....", ".x.....", "x......",
        ];
        assert_eq!(
            detect(&Roll, &zigzag),
            [
                cand(ID, 0, 300, &[0, 1, 2, 3], 500),
                cand(ID, 300, 600, &[0, 1, 2, 3], 500)
            ]
        );
    }

    #[test]
    fn trills_and_interleaved_chords_are_not_rolls() {
        let trill = chart![step = 100; "x......", ".x.....", "x......", ".x....."];
        assert!(detect(&Roll, &trill).is_empty());
        let shape = chart![step = 100; "x.x....", ".x.x...", "..x.x..", "...x.x."];
        assert!(detect(&Roll, &shape).is_empty());
    }

    #[test]
    fn near_misses() {
        let short = chart![step = 100; "x......", ".x.....", "..x...."];
        assert!(detect(&Roll, &short).is_empty());
        let slow = chart![step = 251; "x......", ".x.....", "..x....", "...x..."];
        assert!(detect(&Roll, &slow).is_empty());
        let jacked = chart![step = 100; "x......", ".x.....", ".x.....", "..x....", "...x..."];
        assert!(detect(&Roll, &jacked).is_empty());
    }
}
