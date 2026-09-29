//! `regular.stream.chordstream_dense`: a jackless, stream-fast run where chords of 4+ notes
//! (ADR 0017) make up at least `chordstream_dense_min_chord_permille` of the rows. Adapts
//! Interlude prelude `Chordstream_7K.DENSE_CHORDSTREAM` (MIT, see NOTICE) to sustained runs.

use wolluf_core::{Keymode, PatternId};

use super::common::{rows_of, share, span_candidate, stream_runs};
use crate::params::PatternParams;
use crate::rule::{Candidate, PatternRule};
use crate::view::ChartView;

pub(super) const ID: PatternId = PatternId::from_static("regular.stream.chordstream_dense");

/// ADR 0017 / taxonomy: dense chords have four notes or more.
const DENSE_MIN_NOTES: u32 = 4;

pub struct ChordstreamDense;

impl PatternRule for ChordstreamDense {
    fn id(&self) -> PatternId {
        ID
    }

    fn version(&self) -> u32 {
        1
    }

    fn supports(&self, _keymode: Keymode) -> bool {
        true
    }

    /// Strength: the share of rows that are 4+-note chords.
    fn detect(&self, view: &ChartView<'_>, params: &PatternParams) -> Vec<Candidate> {
        let p = &params.stream;
        let rows = view.rows();
        let mut found: Vec<Candidate> = stream_runs(rows, p, |r| r.notes >= 1)
            .iter()
            .filter_map(|run| {
                let span = rows_of(rows, run);
                if u32::try_from(span.len()).map_or(true, |n| n < p.chordstream_min_rows) {
                    return None;
                }
                let dense = share(&span, |r| r.notes >= DENSE_MIN_NOTES);
                if dense < p.chordstream_dense_min_chord_permille {
                    return None;
                }
                let strength = share(&span, |r| r.notes >= DENSE_MIN_NOTES);
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
    fn four_note_chords_in_a_jackless_stream_are_dense() {
        let chart = chart![step = 100;
            "xxxx...",
            "....xx.",
            "xxxx...",
            "....xxx",
        ];
        assert_eq!(
            detect(&ChordstreamDense, &chart),
            [cand(ID, 0, 300, &[0, 1, 2, 3, 4, 5, 6], 500)]
        );
    }

    #[test]
    fn light_chordstreams_are_not_dense() {
        let chart = chart![step = 100; "xxx....", "...xx..", "xx...x.", "..xx..."];
        assert!(detect(&ChordstreamDense, &chart).is_empty());
    }

    #[test]
    fn near_misses() {
        // One 4-note chord in 4 rows is under 400 permille.
        let sparse = chart![step = 100; "xxxx...", "....xx.", "xx.....", "..xx..."];
        assert!(detect(&ChordstreamDense, &sparse).is_empty());
        let jacked = chart![step = 100; "xxxx...", "...xxx.", "xx.....", "...xxxx"];
        assert!(detect(&ChordstreamDense, &jacked).is_empty());
    }
}
