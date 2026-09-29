//! `ln.release.timing`: staggered tails that need separate, precise releases. An occurrence is a
//! row of real LN tails whose previous tail row is at least `release_min_stagger_us` earlier
//! (closer tails release as one, the lazer `release_threshold`, research 02 l.44) and within the
//! release stagger cap (beat-relative under a red line, capped in µs). Sections of at least
//! `release_min_occurrences` (grouped by the LN group gap) span from the tail row before the
//! first occurrence.

use wolluf_core::{Keymode, PatternId};

use super::common::{gap_within, saturating, union};
use super::ln_common::{group, masks, real_lns};
use crate::params::PatternParams;
use crate::rule::{Candidate, PatternRule};
use crate::view::ChartView;

pub(super) const ID: PatternId = PatternId::from_static("ln.release.timing");

pub struct LnRelease;

impl PatternRule for LnRelease {
    fn id(&self) -> PatternId {
        ID
    }

    fn version(&self) -> u32 {
        1
    }

    fn supports(&self, _keymode: Keymode) -> bool {
        true
    }

    /// Strength: `saturating(occurrences, release_min_occurrences)`. `cols` are the tails'
    /// columns.
    fn detect(&self, view: &ChartView<'_>, params: &PatternParams) -> Vec<Candidate> {
        let p = &params.ln;
        let rows = view.rows();
        let k = view.keymode();
        let tails = masks(k, rows.len(), &real_lns(view, p), false);
        let tail_rows: Vec<usize> = (0..rows.len())
            .filter(|&i| tails.get(i).is_some_and(|m| !m.is_empty()))
            .collect();
        let events: Vec<(usize, usize)> = tail_rows
            .windows(2)
            .filter_map(|w| {
                let (&prev, &cur) = (w.first()?, w.get(1)?);
                let (a, b) = (rows.get(prev)?, rows.get(cur)?);
                let gap = b.t.0 - a.t.0;
                let staggered = gap >= p.release_min_stagger_us
                    && gap_within(
                        gap,
                        b.beat_us,
                        p.release_max_stagger_ticks,
                        p.release_max_stagger_us,
                    );
                staggered.then_some((cur, prev))
            })
            .collect();
        let tail_cols = |i: usize| tails.get(i).copied().unwrap_or_default();
        let mut found: Vec<Candidate> = group(rows, events, p)
            .into_iter()
            .filter(|g| u32::try_from(g.len()).is_ok_and(|n| n >= p.release_min_occurrences))
            .filter_map(|g| {
                let (first, last) = (g.first()?.1, g.last()?.0);
                Some(Candidate {
                    pattern: ID,
                    t0: rows.get(first)?.t,
                    t1: rows.get(last)?.t,
                    cols: union(
                        k,
                        g.iter()
                            .flat_map(|&(cur, prev)| [tail_cols(cur), tail_cols(prev)]),
                    ),
                    strength: saturating(g.len(), p.release_min_occurrences),
                })
            })
            .collect();
        found.sort();
        found
    }
}

#[cfg(test)]
mod tests {
    use wolluf_chart::{Chart, chart};

    use super::*;
    use crate::rules::testkit::{cand, detect};

    fn staggered(step: i32) -> Chart {
        wolluf_chart::testkit::chart_from_rows(
            0,
            step,
            &[
                "[[[[...", "||||...", "]|||...", ".]||...", "..]|...", "...]...",
            ],
        )
        .unwrap()
    }

    #[test]
    fn staggered_tails_need_timed_releases() {
        assert_eq!(
            detect(&LnRelease, &staggered(50)),
            [cand(ID, 100, 250, &[0, 1, 2, 3], 500)]
        );
    }

    #[test]
    fn chord_releases_are_not_release_timing() {
        let chart = chart![step = 50; "[[[[...", "||||...", "]]]]..."];
        assert!(detect(&LnRelease, &chart).is_empty());
    }

    #[test]
    fn near_misses() {
        // 20 ms apart is inside the 30 ms release window: effectively one release.
        assert!(detect(&LnRelease, &staggered(20)).is_empty());
        assert!(detect(&LnRelease, &staggered(251)).is_empty());
        let two = chart![step = 50; "[[[....", "|||....", "]||....", ".]|....", "..]...."];
        assert!(detect(&LnRelease, &two).is_empty());
    }
}
