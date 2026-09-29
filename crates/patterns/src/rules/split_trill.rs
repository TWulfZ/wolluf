//! `regular.stream.split_trill`: an alternation (`a b a b …`, disjoint sides, stream-fast) of at
//! least `split_trill_min_rows` rows whose sides lie on different hands of the user's layout:
//! one side wholly on the left hand, the other wholly on the right. The thumb follows the
//! layout (ADR 0017); a side on the `Hand::Both` column belongs to neither hand, so it never
//! splits. Interlude prelude `Chordstream_4K.SPLITTRILL` (MIT, see NOTICE) gives the 3-row
//! minimum.

use wolluf_core::{Keymode, PatternId};

use super::common::{alternations, rows_of, saturating, span_candidate};
use crate::params::PatternParams;
use crate::rule::{Candidate, PatternRule};
use crate::view::ChartView;

pub(super) const ID: PatternId = PatternId::from_static("regular.stream.split_trill");

pub struct SplitTrill;

impl PatternRule for SplitTrill {
    fn id(&self) -> PatternId {
        ID
    }

    fn version(&self) -> u32 {
        1
    }

    fn supports(&self, _keymode: Keymode) -> bool {
        true
    }

    /// Strength: `saturating(rows, split_trill_min_rows)`.
    fn detect(&self, view: &ChartView<'_>, params: &PatternParams) -> Vec<Candidate> {
        let p = &params.stream;
        let rows = view.rows();
        let hands = view.hand_cols();
        let side = |r: &crate::view::RowFeat| {
            if r.press.is_subset_of(hands.left) {
                Some(Side::Left)
            } else if r.press.is_subset_of(hands.right) {
                Some(Side::Right)
            } else {
                None
            }
        };
        let mut found: Vec<Candidate> = alternations(rows, p, |r| side(r).is_some())
            .iter()
            .filter_map(|run| {
                let span = rows_of(rows, run);
                if u32::try_from(span.len()).map_or(true, |n| n < p.split_trill_min_rows) {
                    return None;
                }
                let (a, b) = (side(span.first()?)?, side(span.get(1)?)?);
                if a == b {
                    return None;
                }
                span_candidate(
                    ID,
                    view.keymode(),
                    &span,
                    saturating(span.len(), p.split_trill_min_rows),
                )
            })
            .collect();
        found.sort();
        found
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

#[cfg(test)]
mod tests {
    use wolluf_chart::{Layout, chart};

    use super::*;
    use crate::rules::testkit::{cand, detect_with};

    fn layout(id: &str) -> Layout {
        Layout::by_id(id).unwrap()
    }

    #[test]
    fn the_thumb_side_decides_a_centre_trill() {
        let right_side = chart![step = 100; "...x...", "....x..", "...x...", "....x.."];
        assert!(detect_with(&SplitTrill, &right_side, &layout("k7.313_right_thumb")).is_empty());
        assert_eq!(
            detect_with(&SplitTrill, &right_side, &layout("k7.313_left_thumb")),
            [cand(ID, 0, 300, &[3, 4], 666)]
        );
        let left_side = chart![step = 100; "..x....", "...x...", "..x....", "...x..."];
        assert_eq!(
            detect_with(&SplitTrill, &left_side, &layout("k7.313_right_thumb")),
            [cand(ID, 0, 300, &[2, 3], 666)]
        );
        assert!(detect_with(&SplitTrill, &left_side, &layout("k7.313_left_thumb")).is_empty());
    }

    #[test]
    fn one_hand_trills_and_the_shared_thumb_are_not_split() {
        let one_hand = chart![step = 100; "x......", ".x.....", "x......", ".x....."];
        for id in ["k7.313_right_thumb", "k7.313_left_thumb", "k7.both_thumbs"] {
            assert!(
                detect_with(&SplitTrill, &one_hand, &layout(id)).is_empty(),
                "{id}"
            );
        }
        let centre = chart![step = 100; "..x....", "...x...", "..x....", "...x..."];
        assert!(detect_with(&SplitTrill, &centre, &layout("k7.both_thumbs")).is_empty());
    }

    #[test]
    fn near_misses() {
        let short = chart![step = 100; "..x....", "....x.."];
        assert!(detect_with(&SplitTrill, &short, &layout("k7.313_right_thumb")).is_empty());
        let cross_hand_chord = chart![step = 100; "..x.x..", ".x.....", "..x.x..", ".x....."];
        assert!(
            detect_with(
                &SplitTrill,
                &cross_hand_chord,
                &layout("k7.313_right_thumb")
            )
            .is_empty()
        );
    }
}
