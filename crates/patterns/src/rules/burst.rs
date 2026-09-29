//! `regular.speed.burst`: short runs much faster than their surroundings. For each press row,
//! the surrounding pace is the mean press-row gap inside a `burst_context_us` window centred on
//! it and clamped to the chart. A row is bursting when that pace is at least
//! `burst_min_density_ratio_permille` times its own press gap. A burst is a maximal run of a
//! press row plus the bursting rows after it, `burst_min_rows..=burst_max_rows` rows long;
//! longer runs are sustained speed, which the stream rules describe.

use wolluf_core::{Keymode, PatternId};

use super::common::{Step, permille, rows_of, scan, span_candidate};
use crate::params::{PatternParams, SpeedParams};
use crate::rule::{Candidate, PatternRule};
use crate::view::{ChartView, RowFeat};

const PERMILLE: i128 = 1_000;

pub(super) const ID: PatternId = PatternId::from_static("regular.speed.burst");

pub struct Burst;

impl PatternRule for Burst {
    fn id(&self) -> PatternId {
        ID
    }

    fn version(&self) -> u32 {
        1
    }

    fn supports(&self, _keymode: Keymode) -> bool {
        true
    }

    /// Strength: `(ratio − min ratio) / min ratio` for the burst's slowest row, full at twice
    /// the minimum ratio.
    fn detect(&self, view: &ChartView<'_>, params: &PatternParams) -> Vec<Candidate> {
        let p = &params.speed;
        let rows = view.rows();
        let pace = Pace::new(rows, p);
        let runs = scan(rows, |run, i| match pace.ratio(rows, i) {
            Some(r) if !run.is_empty() && r >= i128::from(p.burst_min_density_ratio_permille) => {
                Step::Extend
            }
            _ => Step::Restart,
        });
        let min_ratio = u64::from(p.burst_min_density_ratio_permille);
        let mut found: Vec<Candidate> = runs
            .iter()
            .filter(|run| {
                u32::try_from(run.len())
                    .is_ok_and(|n| (p.burst_min_rows..=p.burst_max_rows).contains(&n))
            })
            .filter_map(|run| {
                let slowest = run
                    .iter()
                    .skip(1)
                    .filter_map(|&i| pace.ratio(rows, i))
                    .min()?;
                let headroom = u64::try_from(slowest)
                    .unwrap_or(0)
                    .saturating_sub(min_ratio);
                span_candidate(
                    ID,
                    view.keymode(),
                    &rows_of(rows, run),
                    permille(headroom, min_ratio),
                )
            })
            .collect();
        found.sort();
        found
    }
}

struct Pace {
    times: Vec<i64>,
    half: i64,
}

impl Pace {
    fn new(rows: &[RowFeat], p: &SpeedParams) -> Self {
        Self {
            times: rows
                .iter()
                .filter(|r| !r.press.is_empty())
                .map(|r| r.t.0)
                .collect(),
            half: p.burst_context_us / 2,
        }
    }

    /// Surrounding mean press gap over this row's press gap, in permille.
    fn ratio(&self, rows: &[RowFeat], i: usize) -> Option<i128> {
        let row = rows.get(i)?;
        let gap = row.press_gap_us.filter(|&g| g > 0)?;
        let (first, last) = (*self.times.first()?, *self.times.last()?);
        let lo = (row.t.0 - self.half).max(first);
        let hi = (row.t.0 + self.half).min(last);
        let inside =
            self.times.partition_point(|&t| t <= hi) - self.times.partition_point(|&t| t < lo);
        let links = i128::try_from(inside)
            .ok()?
            .checked_sub(1)
            .filter(|&n| n > 0)?;
        Some(i128::from(hi - lo) * PERMILLE / (links * i128::from(gap)))
    }
}

#[cfg(test)]
mod tests {
    use wolluf_chart::Chart;

    use super::*;
    use crate::rules::testkit::{cand, detect, seq};

    fn burst_chart(burst_rows: usize) -> Chart {
        let mut gaps = vec![400; 7];
        gaps.extend(std::iter::repeat_n(50, burst_rows));
        gaps.extend([400; 8]);
        seq(&gaps, &[0, 1, 2, 3, 4, 5, 6], None)
    }

    #[test]
    fn a_short_fast_run_inside_a_slow_section_is_a_burst() {
        assert_eq!(
            detect(&Burst, &burst_chart(3)),
            [cand(ID, 2800, 2950, &[0, 1, 2, 3], 1000)]
        );
    }

    #[test]
    fn even_streams_have_no_burst() {
        assert!(detect(&Burst, &seq(&[100; 30], &[0, 2, 4, 6], None)).is_empty());
        assert!(detect(&Burst, &seq(&[400; 20], &[0, 2, 4, 6], None)).is_empty());
    }

    #[test]
    fn near_misses() {
        // One fast row makes a 2-row run; 13 make a 14-row run, which is a stream, not a burst.
        assert!(detect(&Burst, &burst_chart(1)).is_empty());
        assert!(detect(&Burst, &burst_chart(13)).is_empty());
        assert_eq!(detect(&Burst, &burst_chart(11)).len(), 1);
    }
}
