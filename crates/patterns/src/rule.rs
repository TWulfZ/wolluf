//! The rule contract (architecture D8, §9.2): each rule scans a [`ChartView`] and proposes
//! candidate spans; the segmenter resolves overlaps.

use wolluf_core::{ColMask, Keymode, PatternId, TimeUs};

use crate::params::PatternParams;
use crate::view::ChartView;

/// `strength` is permille, so rules compare on one integer scale and outputs stay exact.
pub const STRENGTH_MAX: u32 = 1_000;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Candidate {
    pub pattern: PatternId,
    pub t0: TimeUs,
    /// Inclusive: the time of the last row in the span.
    pub t1: TimeUs,
    pub cols: ColMask,
    /// `0..=STRENGTH_MAX`.
    pub strength: u32,
}

/// `Send + Sync` because the app runs stages on a worker pool that shares the registry.
pub trait PatternRule: Send + Sync {
    fn id(&self) -> PatternId;
    /// Bump when the rule's output changes for the same input and params: every rule's
    /// `(id, version)` enters the `patterns` stage key, so stored segments are recomputed.
    fn version(&self) -> u32;
    fn supports(&self, keymode: Keymode) -> bool;
    /// Candidates in any order; callers sort them.
    fn detect(&self, view: &ChartView<'_>, params: &PatternParams) -> Vec<Candidate>;
}

#[cfg(test)]
mod tests {
    use wolluf_chart::{Layout, chart};
    use wolluf_core::{ColMask, Keymode, PatternId, TimeUs};

    use super::*;
    use crate::params::PatternParams;
    use crate::view::ChartView;

    struct EveryPress;

    impl PatternRule for EveryPress {
        fn id(&self) -> PatternId {
            PatternId::from_static("test.every_press")
        }

        fn version(&self) -> u32 {
            1
        }

        fn supports(&self, keymode: Keymode) -> bool {
            keymode == Keymode::K7
        }

        fn detect(&self, view: &ChartView<'_>, _params: &PatternParams) -> Vec<Candidate> {
            view.rows()
                .iter()
                .filter(|r| !r.press.is_empty())
                .map(|r| Candidate {
                    pattern: self.id(),
                    t0: r.t,
                    t1: r.t,
                    cols: r.press,
                    strength: STRENGTH_MAX,
                })
                .collect()
        }
    }

    #[test]
    fn rules_are_object_safe_and_candidates_order_by_time() {
        let chart = chart![step = 100; "x......", "[......", "]x....."];
        let layout = Layout::default_for(Keymode::K7);
        let params = PatternParams::default();
        let view = ChartView::new(&chart, &layout, &params).unwrap();
        let rule: &dyn PatternRule = &EveryPress;
        assert!(rule.supports(Keymode::K7));
        assert!(!rule.supports(Keymode::K4));
        let mut found = rule.detect(&view, &params);
        found.reverse();
        found.sort();
        let starts: Vec<TimeUs> = found.iter().map(|c| c.t0).collect();
        assert_eq!(
            starts,
            [
                TimeUs::from_ms(0),
                TimeUs::from_ms(100),
                TimeUs::from_ms(200)
            ]
        );
        assert_eq!(found[2].cols, ColMask::single(Keymode::K7, 1).unwrap());
    }
}
