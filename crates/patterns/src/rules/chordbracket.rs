//! `regular.stream.chordbracket`: a 2–3-note chord shape moving across columns without jacking
//! (ADR 0017). Adapts Interlude prelude `Chordstream_7K.BRACKETS` (MIT, see NOTICE: 3 rows,
//! no roll, no jack, but chords of 3+): a run of at least `chordbracket_min_rows` stream-fast
//! press rows of 2–3 notes where each row shares no column with the previous one and is not an
//! Interlude roll (it interleaves with the previous row rather than sitting on one side of it),
//! and no row repeats the row two before it, which would be a jumptrill.

use wolluf_core::{Keymode, PatternId};

use super::common::{Step, rows_of, saturating, scan, span_candidate, stream_gap_ok};
use crate::params::PatternParams;
use crate::rule::{Candidate, PatternRule};
use crate::view::ChartView;

/// ADR 0017: the moving shape has 2–3 notes; alternating bigger chords are a jumptrill.
const SHAPE_MIN_NOTES: u32 = 2;
const SHAPE_MAX_NOTES: u32 = 3;

pub(super) const ID: PatternId = PatternId::from_static("regular.stream.chordbracket");

pub struct Chordbracket;

impl PatternRule for Chordbracket {
    fn id(&self) -> PatternId {
        ID
    }

    fn version(&self) -> u32 {
        1
    }

    fn supports(&self, _keymode: Keymode) -> bool {
        true
    }

    /// Strength: `saturating(rows, chordbracket_min_rows)`.
    fn detect(&self, view: &ChartView<'_>, params: &PatternParams) -> Vec<Candidate> {
        let p = &params.stream;
        let rows = view.rows();
        let runs = scan(rows, |run, i| {
            let Some(row) = rows.get(i) else {
                return Step::Break;
            };
            if !(SHAPE_MIN_NOTES..=SHAPE_MAX_NOTES).contains(&row.notes) {
                return Step::Break;
            }
            let linked = row.jacks == 0 && !row.is_roll && stream_gap_ok(row, p);
            if run.is_empty() || !linked {
                return Step::Restart;
            }
            let two_back = run
                .len()
                .checked_sub(2)
                .and_then(|k| run.get(k))
                .and_then(|&k| rows.get(k));
            match two_back {
                Some(back) if back.press == row.press => Step::RestartFromPrev,
                _ => Step::Extend,
            }
        });
        let mut found: Vec<Candidate> = runs
            .iter()
            .filter_map(|run| {
                let span = rows_of(rows, run);
                if u32::try_from(span.len()).map_or(true, |n| n < p.chordbracket_min_rows) {
                    return None;
                }
                span_candidate(
                    ID,
                    view.keymode(),
                    &span,
                    saturating(span.len(), p.chordbracket_min_rows),
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
    fn a_two_note_shape_moving_without_jacks_is_a_chordbracket() {
        let chart = chart![step = 100; "x.x....", ".x.x...", "..x.x..", "...x.x."];
        assert_eq!(
            detect(&Chordbracket, &chart),
            [cand(ID, 0, 300, &[0, 1, 2, 3, 4, 5], 666)]
        );
    }

    #[test]
    fn jumptrills_and_chord_rolls_are_not_chordbrackets() {
        let jumptrill = chart![step = 100; "x.x....", ".x.x...", "x.x....", ".x.x..."];
        assert!(detect(&Chordbracket, &jumptrill).is_empty());
        let roll = chart![step = 100; "xx.....", "..xx...", "....xx."];
        assert!(detect(&Chordbracket, &roll).is_empty());
    }

    #[test]
    fn near_misses() {
        let short = chart![step = 100; "x.x....", ".x.x..."];
        assert!(detect(&Chordbracket, &short).is_empty());
        let big = chart![step = 100; "x.x.x.x", ".x.x.x.", "x.x.x.x"];
        assert!(detect(&Chordbracket, &big).is_empty());
        let jacked = chart![step = 100; "x.x....", "..xx...", ".x..x.."];
        assert!(detect(&Chordbracket, &jacked).is_empty());
    }
}
