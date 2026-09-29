//! `regular.stream.handstream`: a jackless, stream-fast run of 1–3-note rows with a hand
//! (3-note chord, ADR 0017) share of at least `handstream_min_chord_permille`, and a 2–3-note
//! share under `chordstream_light_min_chord_permille`; above that the run is a light
//! chordstream.

use wolluf_core::{Keymode, PatternId};

use super::common::{rows_of, share, span_candidate, stream_runs};
use crate::params::PatternParams;
use crate::rule::{Candidate, PatternRule};
use crate::view::ChartView;

pub(super) const ID: PatternId = PatternId::from_static("regular.stream.handstream");

/// ADR 0017 / taxonomy: a hand is a 3-note chord.
const HAND_NOTES: u32 = 3;
const CHORD_MIN_NOTES: u32 = 2;

pub struct Handstream;

impl PatternRule for Handstream {
    fn id(&self) -> PatternId {
        ID
    }

    fn version(&self) -> u32 {
        1
    }

    fn supports(&self, _keymode: Keymode) -> bool {
        true
    }

    /// Strength: the share of rows that are hands.
    fn detect(&self, view: &ChartView<'_>, params: &PatternParams) -> Vec<Candidate> {
        let p = &params.stream;
        let rows = view.rows();
        let mut found: Vec<Candidate> =
            stream_runs(rows, p, |r| (1..=HAND_NOTES).contains(&r.notes))
                .iter()
                .filter_map(|run| {
                    let span = rows_of(rows, run);
                    if u32::try_from(span.len()).map_or(true, |n| n < p.min_rows) {
                        return None;
                    }
                    let hands = share(&span, |r| r.notes == HAND_NOTES);
                    let chords = share(&span, |r| r.notes >= CHORD_MIN_NOTES);
                    if hands < p.handstream_min_chord_permille
                        || chords >= p.chordstream_light_min_chord_permille
                    {
                        return None;
                    }
                    let strength = share(&span, |r| r.notes == HAND_NOTES);
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
    fn a_stream_with_three_note_chords_is_a_handstream() {
        let chart = chart![step = 100;
            "xxx....",
            "...x...",
            "x...xx.",
            ".x.....",
            "..xx..x",
        ];
        assert_eq!(
            detect(&Handstream, &chart),
            [cand(ID, 0, 400, &[0, 1, 2, 3, 4, 5, 6], 600)]
        );
    }

    #[test]
    fn jumpstreams_are_not_handstreams() {
        let chart =
            chart![step = 100; "x......", "..xx...", "x......", ".x..x..", "..x....", "x....x."];
        assert!(detect(&Handstream, &chart).is_empty());
    }

    #[test]
    fn chord_share_bounds() {
        // Chords on 4 of 5 rows are a light chordstream instead.
        let chordy = chart![step = 100; "xxx....", "...xx..", "xx...x.", "..xx...", "x......"];
        assert!(detect(&Handstream, &chordy).is_empty());
        // One hand in 7 rows is under 150 permille.
        let sparse = chart![step = 100;
            "xxx....", "...x...", "x......", ".x.....", "..x....", "x......", "...x...",
        ];
        assert!(detect(&Handstream, &sparse).is_empty());
    }
}
