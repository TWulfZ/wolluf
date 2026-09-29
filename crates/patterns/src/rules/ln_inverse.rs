//! `ln.inverse.gap`: long notes filling the gaps between notes. An event is a tail followed on
//! the same column by the next real LN's head within the inverse gap (beat-relative under a red
//! line, capped in µs), and only counts when held coverage from the earlier head to the later
//! tail reaches `inverse_min_coverage_permille`: a thin LN run nearby must not dilute a
//! full-width section, nor join it. Sections of at least `inverse_min_gaps` counted events
//! (grouped by the LN group gap) span from the first earlier head to the last later tail.

use wolluf_core::{ColMask, Keymode, PatternId};

use super::common::{gap_within, union};
use super::ln_common::{Coverage, RealLn, group, real_lns};
use crate::params::PatternParams;
use crate::rule::{Candidate, PatternRule};
use crate::view::ChartView;

pub(super) const ID: PatternId = PatternId::from_static("ln.inverse.gap");

pub struct LnInverse;

impl PatternRule for LnInverse {
    fn id(&self) -> PatternId {
        ID
    }

    fn version(&self) -> u32 {
        1
    }

    fn supports(&self, _keymode: Keymode) -> bool {
        true
    }

    /// Strength: held coverage over the span.
    fn detect(&self, view: &ChartView<'_>, params: &PatternParams) -> Vec<Candidate> {
        let p = &params.ln;
        let rows = view.rows();
        let k = view.keymode();
        let lns = real_lns(view, p);
        let coverage = Coverage::new(rows, k);
        let mut events: Vec<(usize, (RealLn, RealLn))> = lns
            .windows(2)
            .filter_map(|w| {
                let (prev, next) = (w.first()?, w.get(1)?);
                let (tail, head) = (rows.get(prev.tail)?, rows.get(next.head)?);
                let short = prev.col == next.col
                    && gap_within(
                        head.t.0 - tail.t.0,
                        head.beat_us,
                        p.inverse_max_gap_ticks,
                        p.inverse_max_gap_us,
                    )
                    && coverage.permille(rows, prev.head, next.tail)
                        >= p.inverse_min_coverage_permille;
                short.then_some((next.head, (*prev, *next)))
            })
            .collect();
        events.sort_by_key(|&(row, (prev, _))| (row, prev.col));
        let mut found: Vec<Candidate> = group(rows, events, p)
            .into_iter()
            .filter(|g| u32::try_from(g.len()).is_ok_and(|n| n >= p.inverse_min_gaps))
            .filter_map(|g| {
                let first = g.iter().map(|&(_, (prev, _))| prev.head).min()?;
                let last = g.iter().map(|&(_, (_, next))| next.tail).max()?;
                let cover = coverage.permille(rows, first, last);
                Some(Candidate {
                    pattern: ID,
                    t0: rows.get(first)?.t,
                    t1: rows.get(last)?.t,
                    cols: union(
                        k,
                        g.iter().map(|&(_, (prev, _))| {
                            ColMask::single(k, prev.col).unwrap_or_default()
                        }),
                    ),
                    strength: cover,
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

    fn inverse(step: i32, cols: usize) -> Chart {
        let row = |c: char| -> String { (0..7).map(|i| if i < cols { c } else { '.' }).collect() };
        let (head, body, tail, gap) = (row('['), row('|'), row(']'), row('.'));
        let mut rows: Vec<&str> = Vec::new();
        for block in 0..3 {
            rows.push(&head);
            rows.extend([body.as_str(); 4]);
            rows.push(&tail);
            if block < 2 {
                rows.push(&gap);
            }
        }
        wolluf_chart::testkit::chart_from_rows(0, step, &rows).unwrap()
    }

    #[test]
    fn long_notes_filling_the_gaps_are_inverse() {
        // Held 750 of 950 ms on all 7 columns.
        assert_eq!(
            detect(&LnInverse, &inverse(50, 7)),
            [cand(ID, 0, 950, &[0, 1, 2, 3, 4, 5, 6], 789)]
        );
    }

    #[test]
    fn a_thin_ln_run_after_the_section_does_not_dilute_it() {
        let block: [&str; 8] = [
            "[[[[[[[", "|||||||", "|||||||", "|||||||", "|||||||", "]]]]]]]", ".......", ".......",
        ];
        let thin: [&str; 4] = ["......[", "......|", "......]", "......."];
        let mut rows: Vec<&str> = Vec::new();
        for _ in 0..3 {
            rows.extend(&block[..7]);
        }
        rows.pop();
        for _ in 0..6 {
            rows.extend(thin);
        }
        let chart = wolluf_chart::testkit::chart_from_rows(0, 100, &rows).unwrap();
        let found = detect(&LnInverse, &chart);
        assert_eq!(found.len(), 1, "{found:?}");
        // The first thin LN still fills a gap next to the full-width section, so it joins it.
        assert_eq!((found[0].t0.0, found[0].t1.0), (0, 2_200_000));
    }

    #[test]
    fn long_notes_with_rice_between_are_not_inverse() {
        let chart = chart![step = 100; "[......", "]......", ".x.....", "..x....", "...x...", "[......", "]......"];
        assert!(detect(&LnInverse, &chart).is_empty());
    }

    #[test]
    fn near_misses() {
        // Gaps of 260 ms are over the 250 ms cap.
        assert!(detect(&LnInverse, &inverse(130, 7)).is_empty());
        // Two columns cover at most 2/7 of the playfield.
        assert!(detect(&LnInverse, &inverse(50, 2)).is_empty());
    }
}
