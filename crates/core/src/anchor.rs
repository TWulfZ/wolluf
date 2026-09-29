//! Time-based anchors (architecture §5.1): feedback targets a chart section by time and
//! columns, never by derived segment ids, which change whenever the pattern engine changes.

use crate::digest::ChartMd5;
use crate::error::CoreError;
use crate::keymode::{ColMask, Keymode};
use crate::time::TimeUs;

/// `[t0_us, t1_us)` of one chart over `cols`; only [`SegmentAnchor::new`] builds one, so every
/// anchor is a non-empty window over columns its keymode has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SegmentAnchor {
    chart_md5: ChartMd5,
    t0_us: TimeUs,
    t1_us: TimeUs,
    cols: ColMask,
}

impl SegmentAnchor {
    pub fn new(
        chart_md5: ChartMd5,
        t0_us: TimeUs,
        t1_us: TimeUs,
        cols: ColMask,
        keymode: Keymode,
    ) -> Result<Self, CoreError> {
        if t0_us >= t1_us {
            return Err(CoreError::InvalidAnchorWindow {
                t0_us: t0_us.0,
                t1_us: t1_us.0,
            });
        }
        ColMask::from_bits(keymode, cols.bits())?;
        if cols.is_empty() {
            return Err(CoreError::EmptyAnchorColumns);
        }
        Ok(Self {
            chart_md5,
            t0_us,
            t1_us,
            cols,
        })
    }

    pub const fn chart_md5(&self) -> ChartMd5 {
        self.chart_md5
    }

    pub const fn t0_us(&self) -> TimeUs {
        self.t0_us
    }

    pub const fn t1_us(&self) -> TimeUs {
        self.t1_us
    }

    pub const fn cols(&self) -> ColMask {
        self.cols
    }

    pub const fn duration(&self) -> TimeUs {
        TimeUs(self.t1_us.0 - self.t0_us.0)
    }

    /// Same chart and intersecting time windows; columns are ignored, so a column-restricted
    /// label still claims its time span.
    pub fn overlaps(&self, other: &Self) -> bool {
        self.chart_md5 == other.chart_md5 && self.t0_us < other.t1_us && other.t0_us < self.t1_us
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChartMd5, ColMask, CoreError, Keymode, TimeUs};

    fn md5() -> ChartMd5 {
        "e956977ccc1d74a50ae48b43a868cc20".parse().unwrap()
    }

    #[test]
    fn anchor_accepts_a_forward_window_within_the_keymode() {
        let cols = ColMask::full(Keymode::K7);
        let a = SegmentAnchor::new(md5(), TimeUs(1_000), TimeUs(5_000), cols, Keymode::K7).unwrap();
        assert_eq!(
            (a.chart_md5(), a.t0_us(), a.t1_us()),
            (md5(), TimeUs(1_000), TimeUs(5_000))
        );
        assert_eq!(a.cols(), cols);
        assert_eq!(a.duration(), TimeUs(4_000));
    }

    #[test]
    fn anchor_rejects_empty_or_backward_windows() {
        let cols = ColMask::full(Keymode::K7);
        for (t0, t1) in [(5, 5), (6, 5)] {
            assert_eq!(
                SegmentAnchor::new(md5(), TimeUs(t0), TimeUs(t1), cols, Keymode::K7),
                Err(CoreError::InvalidAnchorWindow {
                    t0_us: t0,
                    t1_us: t1
                })
            );
        }
    }

    #[test]
    fn anchor_rejects_columns_outside_the_keymode_or_none() {
        let seven = ColMask::full(Keymode::K7);
        assert_eq!(
            SegmentAnchor::new(md5(), TimeUs(0), TimeUs(1), seven, Keymode::K4),
            Err(CoreError::ColumnOutOfRange { col: 4, keymode: 4 })
        );
        assert_eq!(
            SegmentAnchor::new(md5(), TimeUs(0), TimeUs(1), ColMask::EMPTY, Keymode::K7),
            Err(CoreError::EmptyAnchorColumns)
        );
    }

    #[test]
    fn anchor_overlap_is_half_open_and_per_chart() {
        let cols = ColMask::full(Keymode::K7);
        let at =
            |t0, t1| SegmentAnchor::new(md5(), TimeUs(t0), TimeUs(t1), cols, Keymode::K7).unwrap();
        assert!(at(0, 10).overlaps(&at(9, 20)));
        assert!(
            !at(0, 10).overlaps(&at(10, 20)),
            "touching windows do not overlap"
        );
        let other: ChartMd5 = "00000000000000000000000000000000".parse().unwrap();
        let elsewhere =
            SegmentAnchor::new(other, TimeUs(0), TimeUs(10), cols, Keymode::K7).unwrap();
        assert!(!at(0, 10).overlaps(&elsewhere));
    }
}
