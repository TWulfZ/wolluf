//! `ln.tech.shield`: a tap right before an LN head on the same column. An occurrence is a real
//! LN head whose column was last pressed by a tap within the shield gap (beat-relative under a
//! red line, capped in µs); sections of at least `shield_min_occurrences` (grouped by the LN
//! group gap) are candidates, spanning from the first shielding tap.

use wolluf_core::{ColMask, Keymode, PatternId};

use super::common::{gap_within, saturating, union};
use super::ln_common::{group, masks, real_lns};
use crate::params::PatternParams;
use crate::rule::{Candidate, PatternRule};
use crate::view::ChartView;

pub(super) const ID: PatternId = PatternId::from_static("ln.tech.shield");

pub struct LnShield;

impl PatternRule for LnShield {
    fn id(&self) -> PatternId {
        ID
    }

    fn version(&self) -> u32 {
        1
    }

    fn supports(&self, _keymode: Keymode) -> bool {
        true
    }

    /// Strength: `saturating(occurrences, shield_min_occurrences)`.
    fn detect(&self, view: &ChartView<'_>, params: &PatternParams) -> Vec<Candidate> {
        let p = &params.ln;
        let rows = view.rows();
        let k = view.keymode();
        let heads = masks(k, rows.len(), &real_lns(view, p), true);
        // Per column: the row of its last press and whether that press was a tap.
        let mut last: Vec<Option<(usize, bool)>> = vec![None; usize::from(k.columns())];
        let mut events: Vec<(usize, (usize, ColMask))> = Vec::new();
        for (i, (feat, row)) in rows.iter().zip(view.chart().rows()).enumerate() {
            for col in heads.get(i).copied().unwrap_or_default().iter() {
                let shielded =
                    last.get(usize::from(col))
                        .copied()
                        .flatten()
                        .and_then(|(j, tap)| {
                            let gap = feat.t.0 - rows.get(j)?.t.0;
                            (tap && gap_within(
                                gap,
                                feat.beat_us,
                                p.shield_max_gap_ticks,
                                p.shield_max_gap_us,
                            ))
                            .then_some(j)
                        });
                if let Some(j) = shielded {
                    events.push((i, (j, ColMask::single(k, col).unwrap_or_default())));
                }
            }
            for col in feat.press.iter() {
                if let Some(slot) = last.get_mut(usize::from(col)) {
                    *slot = Some((i, row.tap.contains(col)));
                }
            }
        }
        let mut found: Vec<Candidate> = group(rows, events, p)
            .into_iter()
            .filter(|g| u32::try_from(g.len()).is_ok_and(|n| n >= p.shield_min_occurrences))
            .filter_map(|g| {
                let first_tap = g.iter().map(|&(_, (j, _))| j).min()?;
                Some(Candidate {
                    pattern: ID,
                    t0: rows.get(first_tap)?.t,
                    t1: rows.get(g.last()?.0)?.t,
                    cols: union(k, g.iter().map(|&(_, (_, m))| m)),
                    strength: saturating(g.len(), p.shield_min_occurrences),
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
    use crate::rules::testkit::{cand, detect, with_beat};

    fn shields(step: i32) -> Chart {
        wolluf_chart::testkit::chart_from_rows(
            0,
            step,
            &[
                "x......", "[......", "|x.....", "][.....", ".|.....", ".].....",
            ],
        )
        .unwrap()
    }

    #[test]
    fn a_tap_right_before_a_long_note_head_is_a_shield() {
        assert_eq!(
            detect(&LnShield, &shields(100)),
            [cand(ID, 0, 300, &[0, 1], 500)]
        );
        assert_eq!(detect(&LnShield, &with_beat(&shields(100), 400.0)).len(), 1);
    }

    #[test]
    fn a_head_after_another_long_note_or_another_column_is_not_a_shield() {
        let ln_then_ln =
            chart![step = 100; "[......", "]......", "[......", "]......", "[......", "]......"];
        assert!(detect(&LnShield, &ln_then_ln).is_empty());
        let other_column =
            chart![step = 100; "x......", ".[.....", ".]x....", "..[....", "..]...."];
        assert!(detect(&LnShield, &other_column).is_empty());
    }

    #[test]
    fn near_misses() {
        assert!(detect(&LnShield, &shields(251)).is_empty());
        // 150 ms is 3/8 of a 400 ms beat, over the 1/4-beat cap.
        assert!(detect(&LnShield, &with_beat(&shields(150), 400.0)).is_empty());
        let single = chart![step = 100; "x......", "[......", "]......"];
        assert!(detect(&LnShield, &single).is_empty());
    }
}
