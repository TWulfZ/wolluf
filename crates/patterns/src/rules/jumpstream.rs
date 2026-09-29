//! `regular.stream.jumpstream`: a jackless, stream-fast run of single notes and jumps
//! (2-note chords, ADR 0017) with a jump share in
//! `[jumpstream_min_chord_permille, chordstream_light_min_chord_permille)`; above that the run is
//! a light chordstream.

use wolluf_core::{Keymode, PatternId};

use super::common::{rows_of, share, span_candidate, stream_runs};
use crate::params::PatternParams;
use crate::rule::{Candidate, PatternRule};
use crate::view::ChartView;

pub(super) const ID: PatternId = PatternId::from_static("regular.stream.jumpstream");

/// ADR 0017 / taxonomy: a jump is a 2-note chord.
const JUMP_NOTES: u32 = 2;

pub struct Jumpstream;

impl PatternRule for Jumpstream {
    fn id(&self) -> PatternId {
        ID
    }

    fn version(&self) -> u32 {
        1
    }

    fn supports(&self, _keymode: Keymode) -> bool {
        true
    }

    /// Strength: the share of rows that are jumps.
    fn detect(&self, view: &ChartView<'_>, params: &PatternParams) -> Vec<Candidate> {
        let p = &params.stream;
        let rows = view.rows();
        let mut found: Vec<Candidate> =
            stream_runs(rows, p, |r| (1..=JUMP_NOTES).contains(&r.notes))
                .iter()
                .filter_map(|run| {
                    let span = rows_of(rows, run);
                    if u32::try_from(span.len()).map_or(true, |n| n < p.min_rows) {
                        return None;
                    }
                    let jumps = share(&span, |r| r.notes == JUMP_NOTES);
                    if jumps < p.jumpstream_min_chord_permille
                        || jumps >= p.chordstream_light_min_chord_permille
                    {
                        return None;
                    }
                    let strength = share(&span, |r| r.notes == JUMP_NOTES);
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
    fn singles_with_jumps_mixed_in_are_a_jumpstream() {
        let chart = chart![step = 100;
            "x......",
            "..xx...",
            "x......",
            ".x..x..",
            "..x....",
            "x....x.",
        ];
        assert_eq!(
            detect(&Jumpstream, &chart),
            [cand(ID, 0, 500, &[0, 1, 2, 3, 4, 5], 500)]
        );
    }

    #[test]
    fn plain_streams_and_hands_are_not_jumpstreams() {
        let single = chart![step = 100; "x......", "..x....", ".x.....", "...x...", "..x...."];
        assert!(detect(&Jumpstream, &single).is_empty());
        let hands =
            chart![step = 100; "x......", "..xxx..", "x......", ".x..x..", "..x....", "x....x."];
        assert!(detect(&Jumpstream, &hands).is_empty());
    }

    #[test]
    fn chord_share_bounds() {
        // 1 jump in 6 rows is under 200 permille.
        let sparse =
            chart![step = 100; "x......", "..xx...", "x......", ".x.....", "..x....", "x......"];
        assert!(detect(&Jumpstream, &sparse).is_empty());
        // 4 jumps in 5 rows reaches the light-chordstream share.
        let dense = chart![step = 100; "xx.....", "..xx...", "xx.....", "..x....", "x...x.."];
        assert!(detect(&Jumpstream, &dense).is_empty());
    }

    #[test]
    fn a_jack_breaks_the_run() {
        let chart =
            chart![step = 100; "x......", "..xx...", "..x....", ".x..x..", "..x....", "x....x."];
        assert!(detect(&Jumpstream, &chart).is_empty());
    }
}
