//! `ln.general.density`: dense long notes with short gaps between them. Over windows of the
//! LN `window_rows` press rows, LN heads must be at least `density_min_ln_share_permille` of
//! the presses and held LN bodies must cover at least `density_min_coverage_permille` of the
//! window's time × columns.

use wolluf_core::{Keymode, PatternId};

use super::common::{permille, rows_of, span_candidate, window_spans};
use super::ln_common::{Coverage, masks, real_lns};
use crate::params::PatternParams;
use crate::rule::{Candidate, PatternRule};
use crate::view::ChartView;

pub(super) const ID: PatternId = PatternId::from_static("ln.general.density");

pub struct LnDensity;

impl PatternRule for LnDensity {
    fn id(&self) -> PatternId {
        ID
    }

    fn version(&self) -> u32 {
        1
    }

    fn supports(&self, _keymode: Keymode) -> bool {
        true
    }

    /// Strength: held coverage over the whole span.
    fn detect(&self, view: &ChartView<'_>, params: &PatternParams) -> Vec<Candidate> {
        let p = &params.ln;
        let rows = view.rows();
        let k = view.keymode();
        let heads = masks(k, rows.len(), &real_lns(view, p), true);
        let coverage = Coverage::new(rows, k);
        let cover = |span: &[usize]| match (span.first(), span.last()) {
            (Some(&a), Some(&b)) => coverage.permille(rows, a, b),
            _ => 0,
        };
        let ln_share = |span: &[usize]| {
            let lns: u64 = span
                .iter()
                .filter_map(|&i| heads.get(i))
                .map(|m| u64::from(m.len()))
                .sum();
            let notes: u64 = rows_of(rows, span).iter().map(|r| u64::from(r.notes)).sum();
            permille(lns, notes)
        };
        let mut found: Vec<Candidate> =
            window_spans(rows, p.window_rows, p.window_max_gap_us, |w| {
                ln_share(w) >= p.density_min_ln_share_permille
                    && cover(w) >= p.density_min_coverage_permille
            })
            .iter()
            .filter_map(|span| span_candidate(ID, k, &rows_of(rows, span), cover(span)))
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
    fn overlapping_long_notes_are_dense() {
        let chart = chart![step = 100;
            "[......",
            "|[.....",
            "||[....",
            "]||[...",
            ".]||[..",
            "..]||[.",
            "...]||[",
            "[...]||",
            "|[...]|",
            "||[...]",
            "]||[...",
            ".]||...",
            "..]|...",
            "...]...",
        ];
        // 27 column-steps held over 10 steps × 7 columns.
        assert_eq!(
            detect(&LnDensity, &chart),
            [cand(ID, 0, 1000, &[0, 1, 2, 3, 4, 5, 6], 385)]
        );
    }

    #[test]
    fn rice_is_not_ln_density() {
        let chart = seq(&[100; 20], &[0, 1, 2, 3, 4, 5, 6], None);
        assert!(detect(&LnDensity, &chart).is_empty());
    }

    #[test]
    fn short_back_to_back_long_notes_cover_too_little() {
        let chart = chart![step = 100;
            "[......",
            "][.....",
            ".][....",
            "..][...",
            "...][..",
            "....][.",
            ".....][",
            "[.....]",
            "]......",
        ];
        assert!(detect(&LnDensity, &chart).is_empty());
    }
}
