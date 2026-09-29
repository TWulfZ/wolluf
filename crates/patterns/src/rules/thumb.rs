//! `regular.tech.thumb`: thumb-heavy sections. The thumb columns are the layout's
//! `Finger::Thumb` columns, whichever hand the layout gives them (ADR 0017: the thumb is not a
//! hand). Over windows of `window_rows` press rows, the share of rows pressing a thumb column
//! must reach `thumb_min_share_permille`. Layouts without a thumb yield nothing.

use wolluf_chart::Finger;
use wolluf_core::{ColMask, Keymode, PatternId};

use super::common::{rows_of, share, union, window_spans};
use crate::params::PatternParams;
use crate::rule::{Candidate, PatternRule};
use crate::view::ChartView;

pub(super) const ID: PatternId = PatternId::from_static("regular.tech.thumb");

pub struct Thumb;

impl PatternRule for Thumb {
    fn id(&self) -> PatternId {
        ID
    }

    fn version(&self) -> u32 {
        1
    }

    fn supports(&self, _keymode: Keymode) -> bool {
        true
    }

    /// Strength: the share of the span's press rows that press a thumb column. `cols` are the
    /// thumb columns pressed.
    fn detect(&self, view: &ChartView<'_>, params: &PatternParams) -> Vec<Candidate> {
        let p = &params.tech;
        let rows = view.rows();
        let k = view.keymode();
        let thumb_bits = (0u8..)
            .zip(view.layout().columns())
            .filter(|(_, (_, finger))| *finger == Finger::Thumb)
            .fold(0u16, |acc, (col, _)| acc | 1 << col);
        if thumb_bits == 0 {
            return Vec::new();
        }
        let on_thumb = |r: &&crate::view::RowFeat| r.press.bits() & thumb_bits != 0;
        let thumb_share = |span: &[usize]| share(&rows_of(rows, span), on_thumb);
        let mut found = Vec::new();
        for span in window_spans(rows, p.window_rows, p.window_max_gap_us, |w| {
            thumb_share(w) >= p.thumb_min_share_permille
        }) {
            let feats = rows_of(rows, &span);
            let (Some(first), Some(last)) = (feats.first(), feats.last()) else {
                continue;
            };
            let cols = union(
                k,
                feats.iter().map(|r| {
                    ColMask::from_bits(k, r.press.bits() & thumb_bits).unwrap_or_default()
                }),
            );
            found.push(Candidate {
                pattern: ID,
                t0: first.t,
                t1: last.t,
                cols,
                strength: thumb_share(&span),
            });
        }
        found.sort();
        found
    }
}

#[cfg(test)]
mod tests {
    use wolluf_chart::Chart;

    use super::*;
    use crate::rules::testkit::{cand, detect, detect_with, layout, taps};

    fn cycle(cols: &[u8], n: usize) -> Chart {
        let notes: Vec<(i32, u8)> = cols
            .iter()
            .copied()
            .cycle()
            .take(n)
            .enumerate()
            .map(|(i, c)| (i as i32 * 100, c))
            .collect();
        taps(&notes, None)
    }

    #[test]
    fn a_thumb_heavy_section_under_both_313_layouts() {
        let chart = cycle(&[3, 0, 4], 18);
        for id in ["k7.313_right_thumb", "k7.313_left_thumb"] {
            assert_eq!(
                detect_with(&Thumb, &chart, &layout(id)),
                [cand(ID, 0, 1700, &[3], 333)],
                "{id}"
            );
        }
    }

    #[test]
    fn layouts_without_a_thumb_and_thumbless_sections_have_none() {
        assert!(detect_with(&Thumb, &cycle(&[3, 0, 4], 18), &layout("k7.43")).is_empty());
        assert!(detect(&Thumb, &cycle(&[0, 1, 4, 5], 20)).is_empty());
    }

    #[test]
    fn share_just_under_the_threshold() {
        assert!(detect(&Thumb, &cycle(&[3, 0, 1, 4], 20)).is_empty());
        assert!(detect(&Thumb, &cycle(&[3, 0, 4], 15)).is_empty());
    }
}
