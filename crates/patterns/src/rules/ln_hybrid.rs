//! `ln.tech.hybrid`: rice tapped while long notes are held. An occurrence is a row with a tap
//! while at least one LN body is held on another column; sections of at least
//! `hybrid_min_taps` such rows (grouped by the LN group gap) are candidates.

use wolluf_core::{ColMask, Keymode, PatternId};

use super::common::{saturating, union};
use super::ln_common::group;
use crate::params::PatternParams;
use crate::rule::{Candidate, PatternRule};
use crate::view::ChartView;

pub(super) const ID: PatternId = PatternId::from_static("ln.tech.hybrid");

pub struct LnHybrid;

impl PatternRule for LnHybrid {
    fn id(&self) -> PatternId {
        ID
    }

    fn version(&self) -> u32 {
        1
    }

    fn supports(&self, _keymode: Keymode) -> bool {
        true
    }

    /// Strength: `saturating(tap rows, hybrid_min_taps)`. `cols` are the taps and the held
    /// columns.
    fn detect(&self, view: &ChartView<'_>, params: &PatternParams) -> Vec<Candidate> {
        let p = &params.ln;
        let rows = view.rows();
        let k = view.keymode();
        let events: Vec<(usize, ColMask)> = rows
            .iter()
            .zip(view.chart().rows())
            .enumerate()
            .filter(|(_, (feat, row))| !row.tap.is_empty() && !feat.held.is_empty())
            .map(|(i, (feat, row))| (i, union(k, [row.tap, feat.held])))
            .collect();
        let mut found: Vec<Candidate> = group(rows, events, p)
            .into_iter()
            .filter(|g| u32::try_from(g.len()).is_ok_and(|n| n >= p.hybrid_min_taps))
            .filter_map(|g| {
                Some(Candidate {
                    pattern: ID,
                    t0: rows.get(g.first()?.0)?.t,
                    t1: rows.get(g.last()?.0)?.t,
                    cols: union(k, g.iter().map(|&(_, m)| m)),
                    strength: saturating(g.len(), p.hybrid_min_taps),
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
    use crate::rules::testkit::{cand, detect, seq};

    #[test]
    fn taps_under_a_held_long_note_are_hybrid() {
        let chart = chart![step = 100; "[......", "|.x....", "|..x...", "|.x....", "]......"];
        assert_eq!(
            detect(&LnHybrid, &chart),
            [cand(ID, 100, 300, &[0, 2, 3], 500)]
        );
    }

    #[test]
    fn rice_alone_and_long_notes_alone_are_not_hybrid() {
        assert!(detect(&LnHybrid, &seq(&[100; 10], &[1, 2, 3], None)).is_empty());
        assert!(
            detect(
                &LnHybrid,
                &chart![step = 100; "[[.....", "||.....", "]]....."]
            )
            .is_empty()
        );
    }

    #[test]
    fn near_misses() {
        let two = chart![step = 100; "[......", "|.x....", "|..x...", "|......", "]......"];
        assert!(detect(&LnHybrid, &two).is_empty());
        let with_head = chart![step = 100; "[.x....", "|..x...", "|.x....", "]......"];
        assert!(detect(&LnHybrid, &with_head).is_empty());
    }
}
