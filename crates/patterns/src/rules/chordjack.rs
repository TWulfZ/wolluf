//! `regular.jack.chordjack`: chords that keep repeating columns row after row.
//!
//! Adapted from Interlude prelude `Patterns.fs` `Jacks.CHORDJACKS` (MIT, see NOTICE), which
//! matches one pair of press rows: `a > 2`, `b > 1`, `jacks ≥ 1`, `(b < a || jacks < b)`. For
//! 7K the pair becomes a sustained run of consecutive press rows, each with at least
//! `chordjack_min_next_notes` notes, each link jack-fast with at least `chordjack_min_jacks`
//! repeated columns, at least `chordjack_min_rows` rows long, and holding at least one chord of
//! `chordjack_min_first_notes`. The `(b < a || jacks < b)` clause is dropped: it rejects a
//! chord repeated unchanged, which 7K players call a chordjack too.

use wolluf_core::{ColMask, Keymode, PatternId};

use super::common::{jack_gap_ok, permille, press_rows};
use crate::params::{JackParams, PatternParams};
use crate::rule::{Candidate, PatternRule};
use crate::view::{ChartView, RowFeat};

pub(super) const ID: PatternId = PatternId::from_static("regular.jack.chordjack");

pub struct Chordjack;

impl PatternRule for Chordjack {
    fn id(&self) -> PatternId {
        ID
    }

    fn version(&self) -> u32 {
        1
    }

    fn supports(&self, _keymode: Keymode) -> bool {
        true
    }

    /// Strength: after the first row, the jacked notes as a share of all notes.
    fn detect(&self, view: &ChartView<'_>, params: &PatternParams) -> Vec<Candidate> {
        let p = &params.jack;
        let rows = view.rows();
        let mut found = Vec::new();
        let mut run: Vec<usize> = Vec::new();
        for i in press_rows(rows) {
            let Some(row) = rows.get(i) else { continue };
            if row.notes < p.chordjack_min_next_notes {
                found.extend(close(&mut run, rows, view.keymode(), p));
                continue;
            }
            let links = run.last().is_some_and(|&last| {
                row.prev_press == Some(last)
                    && row.jacks >= p.chordjack_min_jacks
                    && row
                        .press_gap_us
                        .is_some_and(|gap| jack_gap_ok(gap, row.beat_us, p))
            });
            if !links {
                found.extend(close(&mut run, rows, view.keymode(), p));
            }
            run.push(i);
        }
        found.extend(close(&mut run, rows, view.keymode(), p));
        found.sort();
        found
    }
}

fn close(run: &mut Vec<usize>, rows: &[RowFeat], k: Keymode, p: &JackParams) -> Option<Candidate> {
    let span: Vec<&RowFeat> = run.drain(..).filter_map(|i| rows.get(i)).collect();
    let long_enough = u32::try_from(span.len()).is_ok_and(|n| n >= p.chordjack_min_rows);
    let has_big_chord = span.iter().any(|r| r.notes >= p.chordjack_min_first_notes);
    if !long_enough || !has_big_chord {
        return None;
    }
    let (first, last) = (span.first()?, span.last()?);
    let cols = span.iter().fold(0u16, |acc, r| acc | r.press.bits());
    let rest = span.get(1..)?;
    Some(Candidate {
        pattern: ID,
        t0: first.t,
        t1: last.t,
        cols: ColMask::from_bits(k, cols).unwrap_or_default(),
        strength: permille(
            rest.iter().map(|r| u64::from(r.jacks)).sum(),
            rest.iter().map(|r| u64::from(r.notes)).sum(),
        ),
    })
}

#[cfg(test)]
mod tests {
    use wolluf_chart::chart;

    use super::*;
    use crate::rules::testkit::{cand, detect, detect_both_thumbs};

    #[test]
    fn chords_that_keep_repeating_columns_are_a_chordjack() {
        let chart = chart![step = 100;
            "xx.x...",
            "xx..x..",
            "x.x.x..",
        ];
        assert_eq!(
            detect(&Chordjack, &chart),
            [cand(ID, 0, 200, &[0, 1, 2, 3, 4], 666)]
        );
    }

    #[test]
    fn a_fully_repeated_chord_is_a_chordjack() {
        let chart = chart![step = 100;
            "xxx....",
            "xxx....",
            "xxx....",
        ];
        assert_eq!(
            detect(&Chordjack, &chart),
            [cand(ID, 0, 200, &[0, 1, 2], 1000)]
        );
    }

    #[test]
    fn jackless_chords_and_single_note_jacks_are_not_chordjacks() {
        let jumpstream = chart![step = 100;
            "xx.x...",
            "..x.xx.",
            "xx.x...",
        ];
        assert!(detect(&Chordjack, &jumpstream).is_empty());
        let longjack = chart![step = 100; "x......", "x......", "x......"];
        assert!(detect(&Chordjack, &longjack).is_empty());
    }

    #[test]
    fn too_few_rows_or_only_two_note_chords_miss() {
        let short = chart![step = 100; "xxx....", "xx.x..."];
        assert!(detect(&Chordjack, &short).is_empty());
        let jumps = chart![step = 100;
            "xx.....",
            ".xx....",
            "..xx...",
        ];
        assert!(detect(&Chordjack, &jumps).is_empty());
    }

    #[test]
    fn gap_over_the_threshold_misses() {
        let slow = chart![step = 501; "xxx....", "xxx....", "xxx...."];
        assert!(detect(&Chordjack, &slow).is_empty());
    }

    #[test]
    fn a_single_note_row_breaks_the_run() {
        let broken = chart![step = 100;
            "xxx....",
            "xx.x...",
            "x......",
            "xx.x...",
            "x.xx...",
        ];
        assert!(detect(&Chordjack, &broken).is_empty());
    }

    #[test]
    fn a_release_only_row_does_not_break_the_run() {
        let chart = chart![step = 100;
            "xx.[...",
            "...]...",
            "xx.x...",
            "x.xx...",
        ];
        assert_eq!(
            detect(&Chordjack, &chart),
            [cand(ID, 0, 300, &[0, 1, 2, 3], 833)]
        );
    }

    #[test]
    fn thumb_column_chordjack_under_both_313_layouts() {
        let chart = chart![step = 100;
            "..xxx..",
            "...xx..",
            "..xx...",
        ];
        assert_eq!(
            detect_both_thumbs(&Chordjack, &chart),
            [cand(ID, 0, 200, &[2, 3, 4], 750)]
        );
    }
}
