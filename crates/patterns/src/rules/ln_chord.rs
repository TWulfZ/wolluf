//! `ln.general.chord`: long notes pressed or released together. An occurrence is a row with at
//! least `chord_min_notes` real LN heads, or as many real LN tails; sections of at least
//! `chord_min_occurrences` occurrences (grouped by the LN group gap) are candidates.

use wolluf_core::{ColMask, Keymode, PatternId};

use super::common::{saturating, union};
use super::ln_common::{group, masks, real_lns};
use crate::params::PatternParams;
use crate::rule::{Candidate, PatternRule};
use crate::view::ChartView;

pub(super) const ID: PatternId = PatternId::from_static("ln.general.chord");

pub struct LnChord;

impl PatternRule for LnChord {
    fn id(&self) -> PatternId {
        ID
    }

    fn version(&self) -> u32 {
        1
    }

    fn supports(&self, _keymode: Keymode) -> bool {
        true
    }

    /// Strength: `saturating(occurrences, chord_min_occurrences)`. `cols` are the chorded LN
    /// columns.
    fn detect(&self, view: &ChartView<'_>, params: &PatternParams) -> Vec<Candidate> {
        let p = &params.ln;
        let rows = view.rows();
        let k = view.keymode();
        let lns = real_lns(view, p);
        let (heads, tails) = (
            masks(k, rows.len(), &lns, true),
            masks(k, rows.len(), &lns, false),
        );
        let chorded = |m: ColMask| {
            if m.len() >= p.chord_min_notes {
                m
            } else {
                ColMask::EMPTY
            }
        };
        let events: Vec<(usize, ColMask)> = heads
            .iter()
            .zip(&tails)
            .enumerate()
            .map(|(i, (&h, &t))| (i, union(k, [chorded(h), chorded(t)])))
            .filter(|(_, m)| !m.is_empty())
            .collect();
        let mut found: Vec<Candidate> = group(rows, events, p)
            .into_iter()
            .filter(|g| u32::try_from(g.len()).is_ok_and(|n| n >= p.chord_min_occurrences))
            .filter_map(|g| {
                Some(Candidate {
                    pattern: ID,
                    t0: rows.get(g.first()?.0)?.t,
                    t1: rows.get(g.last()?.0)?.t,
                    cols: union(k, g.iter().map(|&(_, m)| m)),
                    strength: saturating(g.len(), p.chord_min_occurrences),
                })
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
    use crate::rules::testkit::{cand, detect, with_beat};

    #[test]
    fn long_notes_pressed_and_released_together_are_ln_chords() {
        let chart = chart![step = 200; "[[.....", "]].....", "..[[...", "..]]..."];
        assert_eq!(
            detect(&LnChord, &chart),
            [cand(ID, 0, 600, &[0, 1, 2, 3], 666)]
        );
    }

    #[test]
    fn staggered_long_notes_are_not_chords() {
        let chart = chart![step = 100; "[......", "|[.....", "]|.....", ".]....."];
        assert!(detect(&LnChord, &chart).is_empty());
    }

    #[test]
    fn near_misses() {
        assert!(detect(&LnChord, &chart![step = 200; "[[.....", "]]....."]).is_empty());
        let far = chart![step = 1001; "[[.....", "]].....", "..[[...", "..]]..."];
        assert!(detect(&LnChord, &far).is_empty());
        // 40 ms long notes are under 1/8 of a 400 ms beat, so they count as taps.
        let short = with_beat(
            &chart![step = 40; "[[.....", "]].....", "[[.....", "]]....."],
            400.0,
        );
        assert!(detect(&LnChord, &short).is_empty());
    }
}
