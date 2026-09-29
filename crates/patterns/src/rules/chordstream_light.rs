//! `regular.stream.chordstream_light`: a jackless, stream-fast run of 1–3-note rows where 2–3-note
//! chords make up at least `chordstream_light_min_chord_permille` of the rows. Adapts Interlude
//! prelude `Chordstream_7K.LIGHT_CHORDSTREAM` / `DOUBLE_STREAMS` (MIT, see NOTICE), which match
//! single chord-to-row pairs, to sustained runs.

use wolluf_core::{Keymode, PatternId};

use super::common::{rows_of, share, span_candidate, stream_runs};
use crate::params::PatternParams;
use crate::rule::{Candidate, PatternRule};
use crate::view::ChartView;

pub(super) const ID: PatternId = PatternId::from_static("regular.stream.chordstream_light");

/// Taxonomy: light chordstreams are mostly 2–3-note chords.
const LIGHT_MIN_NOTES: u32 = 2;
const LIGHT_MAX_NOTES: u32 = 3;

pub struct ChordstreamLight;

impl PatternRule for ChordstreamLight {
    fn id(&self) -> PatternId {
        ID
    }

    fn version(&self) -> u32 {
        1
    }

    fn supports(&self, _keymode: Keymode) -> bool {
        true
    }

    /// Strength: the share of rows that are 2–3-note chords.
    fn detect(&self, view: &ChartView<'_>, params: &PatternParams) -> Vec<Candidate> {
        let p = &params.stream;
        let rows = view.rows();
        let mut found: Vec<Candidate> =
            stream_runs(rows, p, |r| (1..=LIGHT_MAX_NOTES).contains(&r.notes))
                .iter()
                .filter_map(|run| {
                    let span = rows_of(rows, run);
                    if u32::try_from(span.len()).map_or(true, |n| n < p.chordstream_min_rows) {
                        return None;
                    }
                    let chords = share(&span, |r| r.notes >= LIGHT_MIN_NOTES);
                    if chords < p.chordstream_light_min_chord_permille {
                        return None;
                    }
                    let strength = share(&span, |r| r.notes >= LIGHT_MIN_NOTES);
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
    fn mostly_two_and_three_note_chords_are_a_light_chordstream() {
        let chart = chart![step = 100;
            "xxx....",
            "...xx..",
            "xx...x.",
            "..xx...",
            "x......",
        ];
        assert_eq!(
            detect(&ChordstreamLight, &chart),
            [cand(ID, 0, 400, &[0, 1, 2, 3, 4, 5], 800)]
        );
    }

    #[test]
    fn single_streams_and_big_chords_are_not_light() {
        let single = chart![step = 100; "x......", "..x....", ".x.....", "...x..."];
        assert!(detect(&ChordstreamLight, &single).is_empty());
        let big = chart![step = 100; "xxxx...", "....xx.", "xxxx...", "....xxx"];
        assert!(detect(&ChordstreamLight, &big).is_empty());
    }

    #[test]
    fn near_misses() {
        let short = chart![step = 100; "xxx....", "...xx..", "xx...x."];
        assert!(detect(&ChordstreamLight, &short).is_empty());
        let handstream = chart![step = 100; "xxx....", "...x...", "x...xx.", ".x.....", "..xx..x"];
        assert!(detect(&ChordstreamLight, &handstream).is_empty());
    }
}
