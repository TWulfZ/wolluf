//! Per-row primitives computed once per chart, shared by every rule. Direction, roll, jack
//! count and the press-row chain follow Interlude prelude `Calculator/Patterns/Primitives.fs`
//! (MIT, YAVSRG d41fc21, see NOTICE); hands come from the user's [`Layout`] instead of column
//! numbers, and gaps are also expressed in beats of the active red line.

use wolluf_chart::{Chart, Hand, Layout, TimingKind};
use wolluf_core::keymode::MAX_COLUMNS;
use wolluf_core::{ColMask, Keymode, TimeUs};

use crate::error::PatternsError;
use crate::params::{PatternParams, TICKS_PER_BEAT, ViewParams};

const US_PER_MS: f64 = 1_000.0;
const MILLI_NPS_PER_NOTE_US: i64 = 1_000_000_000;

/// How the outermost pressed columns moved from the previous press row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Direction {
    /// Interlude's `Direction.None`: both extremes unchanged, or no previous press row.
    Neutral,
    Left,
    Right,
    Outwards,
    Inwards,
}

/// Columns split by the layout's hand. `both` is only non-empty for layouts whose thumb column
/// either hand may take (`k7.both_thumbs`); hand-split features treat it as its own bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HandMasks {
    pub left: ColMask,
    pub right: ColMask,
    pub both: ColMask,
}

impl HandMasks {
    pub const fn get(&self, hand: Hand) -> ColMask {
        match hand {
            Hand::Left => self.left,
            Hand::Right => self.right,
            Hand::Both => self.both,
        }
    }

    fn of_layout(layout: &Layout) -> Self {
        let mut bits = [0u16; 3];
        for (col, (hand, _)) in layout.columns().iter().enumerate() {
            let slot = match hand {
                Hand::Left => 0,
                Hand::Right => 1,
                Hand::Both => 2,
            };
            bits[slot] |= 1 << col;
        }
        let k = layout.keymode();
        Self {
            left: mask(k, bits[0]),
            right: mask(k, bits[1]),
            both: mask(k, bits[2]),
        }
    }

    fn split(&self, k: Keymode, press: ColMask) -> Self {
        Self {
            left: and(k, press, self.left),
            right: and(k, press, self.right),
            both: and(k, press, self.both),
        }
    }
}

/// One chart row. "Press" is a tap or an LN head; "previous press row" skips rows that only
/// release, as Interlude does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowFeat {
    pub t: TimeUs,
    pub press: ColMask,
    pub release: ColMask,
    /// LN bodies strictly between head and tail; disjoint from `press` and `release`.
    pub held: ColMask,
    /// Presses in this row.
    pub notes: u32,
    /// To the previous row of any kind.
    pub gap_us: Option<i64>,
    /// Index of the previous row with a press.
    pub prev_press: Option<usize>,
    pub press_gap_us: Option<i64>,
    /// `press` ∩ the previous press row's `press`.
    pub jack: ColMask,
    pub jacks: u32,
    pub hands: HandMasks,
    pub direction: Direction,
    /// Interlude's `Roll`: this row lies entirely on one side of the previous press row.
    pub is_roll: bool,
    /// Per column, the time since its last press strictly before this row.
    pub col_gap_us: [Option<i64>; MAX_COLUMNS as usize],
    /// Beat length of the active red line, rounded to µs.
    pub beat_us: Option<i64>,
    /// `press_gap_us` in [`TICKS_PER_BEAT`] ticks of `beat_us`, rounded.
    pub gap_ticks: Option<u32>,
    /// The coarsest `ViewParams::snap_divisors` entry `d` for which `press_gap_us` is a whole
    /// number of `1/d` beats; `None` is off-snap.
    pub snap: Option<u8>,
    /// Presses per second ×1000 in the window centred on `t`.
    pub density_milli: u32,
}

#[derive(Debug, Clone)]
pub struct ChartView<'a> {
    chart: &'a Chart,
    layout: &'a Layout,
    hands: HandMasks,
    rows: Vec<RowFeat>,
}

impl<'a> ChartView<'a> {
    pub fn new(
        chart: &'a Chart,
        layout: &'a Layout,
        params: &PatternParams,
    ) -> Result<Self, PatternsError> {
        let k = chart.keymode();
        if layout.keymode() != k {
            return Err(PatternsError::KeymodeMismatch {
                chart: k.columns(),
                layout: layout.keymode().columns(),
            });
        }
        let hands = HandMasks::of_layout(layout);
        let reds = red_lines(chart);
        let presses: Vec<ColMask> = chart
            .rows()
            .iter()
            .map(|r| or(k, r.tap, r.ln_head))
            .collect();
        let density = densities(chart, &presses, &params.view);

        let mut rows = Vec::with_capacity(presses.len());
        let mut prev_t: Option<TimeUs> = None;
        let mut prev_press: Option<(usize, TimeUs, ColMask)> = None;
        let mut last_press = [None::<TimeUs>; MAX_COLUMNS as usize];
        for (i, (row, &press)) in chart.rows().iter().zip(&presses).enumerate() {
            let t = row.t;
            let press_gap_us = prev_press.map(|(_, pt, _)| t.0 - pt.0);
            let (jack, direction, is_roll) = match prev_press {
                Some((_, _, prev)) if !press.is_empty() => {
                    let (direction, is_roll) = direction_of(prev, press);
                    (and(k, press, prev), direction, is_roll)
                }
                _ => (ColMask::EMPTY, Direction::Neutral, false),
            };
            let beat_us = beat_at(&reds, t);
            let gap_ticks = beat_us.zip(press_gap_us).map(|(b, g)| ticks(g, b));
            let snap = beat_us
                .zip(press_gap_us)
                .and_then(|(b, g)| snap_of(g, b, &params.view));
            let col_gap_us = last_press.map(|last| last.map(|lt| t.0 - lt.0));

            rows.push(RowFeat {
                t,
                press,
                release: row.ln_tail,
                held: chart.hold_mask(t),
                notes: press.len(),
                gap_us: prev_t.map(|pt| t.0 - pt.0),
                prev_press: prev_press.map(|(p, _, _)| p),
                press_gap_us,
                jack,
                jacks: jack.len(),
                hands: hands.split(k, press),
                direction,
                is_roll,
                col_gap_us,
                beat_us,
                gap_ticks,
                snap,
                density_milli: density.get(i).copied().unwrap_or(0),
            });

            prev_t = Some(t);
            if !press.is_empty() {
                prev_press = Some((i, t, press));
                for col in press.iter() {
                    if let Some(slot) = last_press.get_mut(usize::from(col)) {
                        *slot = Some(t);
                    }
                }
            }
        }

        Ok(Self {
            chart,
            layout,
            hands,
            rows,
        })
    }

    pub fn chart(&self) -> &'a Chart {
        self.chart
    }

    pub fn layout(&self) -> &'a Layout {
        self.layout
    }

    pub fn keymode(&self) -> Keymode {
        self.chart.keymode()
    }

    pub fn hand_cols(&self) -> HandMasks {
        self.hands
    }

    /// One entry per `Chart::rows()` entry, same order.
    pub fn rows(&self) -> &[RowFeat] {
        &self.rows
    }
}

/// Interlude `Primitives.detect_direction`, over column extremes.
fn direction_of(prev: ColMask, cur: ColMask) -> (Direction, bool) {
    let (p_left, p_right) = extremes(prev);
    let (c_left, c_right) = extremes(cur);
    let (dl, dr) = (c_left - p_left, c_right - p_right);
    let direction = match (dl.signum(), dr.signum()) {
        (1, 1) => Direction::Right,
        (1, _) | (0, -1) => Direction::Inwards,
        (-1, -1) => Direction::Left,
        (-1, _) | (0, 1) => Direction::Outwards,
        _ => Direction::Neutral,
    };
    (direction, p_left > c_right || p_right < c_left)
}

fn extremes(m: ColMask) -> (i32, i32) {
    let bits = m.bits();
    (
        bits.trailing_zeros() as i32,
        (u16::BITS - 1 - bits.leading_zeros()) as i32,
    )
}

/// Valid red lines as `(t, beat_us)`, in time order. Non-finite or sub-µs beat lengths (seen in
/// "stop" gimmicks) are skipped so they never divide a gap.
fn red_lines(chart: &Chart) -> Vec<(TimeUs, i64)> {
    chart
        .timing()
        .iter()
        .filter_map(|p| match p.kind {
            TimingKind::Uninherited { beat_len_ms, .. } => {
                let beat_us = (beat_len_ms * US_PER_MS).round();
                (beat_us.is_finite() && beat_us >= 1.0).then_some((p.t, beat_us as i64))
            }
            TimingKind::Inherited { .. } => None,
        })
        .collect()
}

/// The last red line at or before `t`; before the first one, the first one (osu! behaviour).
fn beat_at(reds: &[(TimeUs, i64)], t: TimeUs) -> Option<i64> {
    let after = reds.partition_point(|&(rt, _)| rt <= t);
    after
        .checked_sub(1)
        .and_then(|i| reds.get(i))
        .or(reds.first())
        .map(|&(_, beat)| beat)
}

pub(crate) fn ticks(gap_us: i64, beat_us: i64) -> u32 {
    let (gap, beat) = (i128::from(gap_us), i128::from(beat_us));
    let ticks = (gap * i128::from(TICKS_PER_BEAT) + beat / 2) / beat;
    u32::try_from(ticks).unwrap_or(u32::MAX)
}

fn snap_of(gap_us: i64, beat_us: i64, params: &ViewParams) -> Option<u8> {
    let (gap, beat) = (i128::from(gap_us), i128::from(beat_us));
    params.snap_divisors.iter().copied().find(|&d| {
        let d = i128::from(d);
        if d == 0 {
            return false;
        }
        let steps = (gap * d + beat / 2) / beat;
        let ideal = (steps * beat + d / 2) / d;
        steps > 0 && (gap - ideal).abs() <= i128::from(params.snap_tolerance_us)
    })
}

/// Presses in `[t - w/2, t - w/2 + w)` per row, via prefix sums over the sorted rows.
fn densities(chart: &Chart, presses: &[ColMask], params: &ViewParams) -> Vec<u32> {
    let window = params.density_window_us.max(1);
    let rows = chart.rows();
    let mut prefix = Vec::with_capacity(presses.len() + 1);
    prefix.push(0i64);
    for press in presses {
        let last = prefix.last().copied().unwrap_or(0);
        prefix.push(last + i64::from(press.len()));
    }
    let count_before = |t: i64| {
        let idx = rows.partition_point(|r| r.t.0 < t);
        prefix.get(idx).copied().unwrap_or(0)
    };
    rows.iter()
        .map(|r| {
            let lo = r.t.0.saturating_sub(window / 2);
            let hi = lo.saturating_add(window);
            let notes = count_before(hi) - count_before(lo);
            let milli = i128::from(notes) * i128::from(MILLI_NPS_PER_NOTE_US) / i128::from(window);
            u32::try_from(milli).unwrap_or(u32::MAX)
        })
        .collect()
}

/// Both inputs are valid for `k`, so the fallbacks are unreachable; core has no infallible
/// mask operators.
fn and(k: Keymode, a: ColMask, b: ColMask) -> ColMask {
    mask(k, a.bits() & b.bits())
}

fn or(k: Keymode, a: ColMask, b: ColMask) -> ColMask {
    mask(k, a.bits() | b.bits())
}

fn mask(k: Keymode, bits: u16) -> ColMask {
    ColMask::from_bits(k, bits).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use wolluf_chart::{
        Chart, ChartMeta, Diagnostics, Hand, Layout, Note, NoteKind, TimingKind, TimingPoint, chart,
    };
    use wolluf_core::{ColMask, Keymode, TimeUs};

    use super::*;
    use crate::error::PatternsError;
    use crate::params::PatternParams;

    fn mask(cols: &[u8]) -> ColMask {
        ColMask::from_cols(Keymode::K7, cols.iter().copied()).unwrap()
    }

    fn us(ms: i64) -> i64 {
        ms * 1000
    }

    fn feats(chart: &Chart) -> Vec<RowFeat> {
        feats_with(chart, &Layout::default_for(chart.keymode()))
    }

    fn feats_with(chart: &Chart, layout: &Layout) -> Vec<RowFeat> {
        ChartView::new(chart, layout, &PatternParams::default())
            .unwrap()
            .rows()
            .to_vec()
    }

    fn red(t_ms: i32, beat_len_ms: f64) -> TimingPoint {
        TimingPoint {
            t: TimeUs::from_ms(t_ms),
            kind: TimingKind::Uninherited {
                beat_len_ms,
                meter: 4,
            },
        }
    }

    fn rebuild(chart: &Chart, timing: Vec<TimingPoint>) -> Chart {
        let taps = chart.rows().iter().flat_map(|r| {
            r.tap.iter().map(move |col| Note {
                t: r.t,
                col,
                kind: NoteKind::Tap,
            })
        });
        let holds = chart.ln_pairs().iter().map(|ln| Note {
            t: ln.head,
            col: ln.col,
            kind: NoteKind::Hold { end: ln.tail },
        });
        Chart::from_notes(
            chart.keymode(),
            ChartMeta::default(),
            timing,
            taps.chain(holds).collect(),
            &mut Diagnostics::new(),
        )
    }

    fn taps_at(times_ms: &[i32], timing: Vec<TimingPoint>) -> Chart {
        let notes = times_ms
            .iter()
            .enumerate()
            .map(|(i, &t)| Note {
                t: TimeUs::from_ms(t),
                col: (i % 2) as u8,
                kind: NoteKind::Tap,
            })
            .collect();
        Chart::from_notes(
            Keymode::K7,
            ChartMeta::default(),
            timing,
            notes,
            &mut Diagnostics::new(),
        )
    }

    #[test]
    fn press_release_and_gaps_per_row() {
        let chart = chart![step = 100;
            "x..[...",
            ".x.|..x",
            "...]...",
            "x.x....",
        ];
        let rows = feats(&chart);
        assert_eq!(rows.len(), 4);

        assert_eq!(rows[0].t, TimeUs::from_ms(0));
        assert_eq!(rows[0].press, mask(&[0, 3]));
        assert_eq!(rows[0].release, ColMask::EMPTY);
        assert_eq!(rows[0].notes, 2);
        assert_eq!(rows[0].gap_us, None);
        assert_eq!(rows[0].prev_press, None);
        assert_eq!(rows[0].press_gap_us, None);

        assert_eq!(rows[1].press, mask(&[1, 6]));
        assert_eq!(rows[1].notes, 2);
        assert_eq!(rows[1].gap_us, Some(us(100)));
        assert_eq!(rows[1].prev_press, Some(0));
        assert_eq!(rows[1].press_gap_us, Some(us(100)));

        assert_eq!(rows[2].press, ColMask::EMPTY);
        assert_eq!(rows[2].release, mask(&[3]));
        assert_eq!(rows[2].notes, 0);
        assert_eq!(rows[2].prev_press, Some(1));
        assert_eq!(rows[2].press_gap_us, Some(us(100)));
        assert_eq!(rows[2].direction, Direction::Neutral);
        assert!(!rows[2].is_roll);

        assert_eq!(rows[3].press, mask(&[0, 2]));
        assert_eq!(rows[3].gap_us, Some(us(100)));
        assert_eq!(rows[3].prev_press, Some(1));
        assert_eq!(rows[3].press_gap_us, Some(us(200)));

        // chart! draws no timing lines.
        assert!(
            rows.iter()
                .all(|r| r.beat_us.is_none() && r.gap_ticks.is_none() && r.snap.is_none())
        );
    }

    #[test]
    fn held_mask_is_the_ln_bodies_strictly_inside_head_and_tail() {
        let chart = chart![step = 100;
            "[[.....",
            "||x....",
            "|]x....",
            "]......",
        ];
        let rows = feats(&chart);
        assert_eq!(rows[0].held, ColMask::EMPTY);
        assert_eq!(rows[0].press, mask(&[0, 1]));
        assert_eq!(rows[1].held, mask(&[0, 1]));
        assert_eq!(rows[2].held, mask(&[0]));
        assert_eq!(rows[2].release, mask(&[1]));
        assert_eq!(rows[3].held, ColMask::EMPTY);
        assert_eq!(rows[3].release, mask(&[0]));
    }

    #[test]
    fn jacks_compare_with_the_previous_press_row() {
        let chart = chart![step = 100;
            "x.x....",
            "x..x...",
            "[..x...",
            "]......",
            "x......",
        ];
        let rows = feats(&chart);
        let jacks: Vec<(ColMask, u32)> = rows.iter().map(|r| (r.jack, r.jacks)).collect();
        assert_eq!(
            jacks,
            vec![
                (ColMask::EMPTY, 0),
                (mask(&[0]), 1),
                (mask(&[0, 3]), 2),
                (ColMask::EMPTY, 0),
                // The tail-only row has no press, so the jack reaches back to the LN head.
                (mask(&[0]), 1),
            ]
        );
    }

    #[test]
    fn hands_split_by_the_layout() {
        let chart = chart![step = 100; "xxxxxxx"];
        let cases = [
            (
                Layout::by_id("k7.313_right_thumb").unwrap(),
                mask(&[0, 1, 2]),
                mask(&[3, 4, 5, 6]),
                ColMask::EMPTY,
            ),
            (
                Layout::by_id("k7.313_left_thumb").unwrap(),
                mask(&[0, 1, 2, 3]),
                mask(&[4, 5, 6]),
                ColMask::EMPTY,
            ),
            (
                Layout::by_id("k7.313_right_thumb").unwrap().mirror(),
                mask(&[4, 5, 6]),
                mask(&[0, 1, 2, 3]),
                ColMask::EMPTY,
            ),
            (
                Layout::by_id("k7.both_thumbs").unwrap(),
                mask(&[0, 1, 2]),
                mask(&[4, 5, 6]),
                mask(&[3]),
            ),
        ];
        for (layout, left, right, both) in cases {
            let view = ChartView::new(&chart, &layout, &PatternParams::default()).unwrap();
            let expected = HandMasks { left, right, both };
            assert_eq!(view.hand_cols(), expected, "{}", layout.id());
            assert_eq!(view.rows()[0].hands, expected, "{}", layout.id());
            assert_eq!(expected.get(Hand::Left), left);
            assert_eq!(expected.get(Hand::Right), right);
            assert_eq!(expected.get(Hand::Both), both);
        }

        let chord = chart![step = 100; "x..x..x"];
        let rows = feats_with(&chord, &Layout::by_id("k7.313_left_thumb").unwrap());
        assert_eq!(
            rows[0].hands,
            HandMasks {
                left: mask(&[0, 3]),
                right: mask(&[6]),
                both: ColMask::EMPTY,
            }
        );
    }

    #[test]
    fn direction_and_roll_follow_interlude() {
        let chart = chart![step = 100;
            "x......",
            ".x.....",
            "x......",
            "x.....x",
            "..x.x..",
            "..x.x..",
            "x.x....",
            "...x..x",
            "...xx..",
            "...x.x.",
        ];
        let got: Vec<(Direction, bool)> = feats(&chart)
            .iter()
            .map(|r| (r.direction, r.is_roll))
            .collect();
        assert_eq!(
            got,
            vec![
                (Direction::Neutral, false),
                (Direction::Right, true),
                (Direction::Left, true),
                (Direction::Outwards, false),
                (Direction::Inwards, false),
                (Direction::Neutral, false),
                (Direction::Left, false),
                (Direction::Right, true),
                (Direction::Inwards, false),
                (Direction::Outwards, false),
            ]
        );
    }

    #[test]
    fn column_gap_is_the_time_since_that_column_was_last_pressed() {
        let chart = chart![step = 100;
            "x.x....",
            "..x....",
            "x......",
        ];
        let rows = feats(&chart);
        assert!(rows[0].col_gap_us.iter().all(Option::is_none));
        assert_eq!(rows[1].col_gap_us[0], Some(us(100)));
        assert_eq!(rows[1].col_gap_us[1], None);
        assert_eq!(rows[1].col_gap_us[2], Some(us(100)));
        assert_eq!(rows[2].col_gap_us[0], Some(us(200)));
        assert_eq!(rows[2].col_gap_us[2], Some(us(100)));
        assert!(rows[2].col_gap_us[3..].iter().all(Option::is_none));
    }

    #[test]
    fn snap_follows_the_active_red_line_across_a_bpm_change() {
        let drawn = chart![step = 100;
            "x......",
            ".x.....",
            "x......",
            ".x.....",
            "x......",
            ".x.....",
        ];
        let chart = rebuild(&drawn, vec![red(0, 600.0), red(300, 400.0)]);
        let got: Vec<(Option<i64>, Option<u32>, Option<u8>)> = feats(&chart)
            .iter()
            .map(|r| (r.beat_us, r.gap_ticks, r.snap))
            .collect();
        assert_eq!(
            got,
            vec![
                (Some(us(600)), None, None),
                (Some(us(600)), Some(32), Some(6)),
                (Some(us(600)), Some(32), Some(6)),
                (Some(us(400)), Some(48), Some(4)),
                (Some(us(400)), Some(48), Some(4)),
                (Some(us(400)), Some(48), Some(4)),
            ]
        );
    }

    #[test]
    fn rows_before_the_first_red_line_use_it() {
        let chart = taps_at(&[0, 125, 1125], vec![red(1000, 500.0)]);
        let rows = feats(&chart);
        assert_eq!(rows[0].beat_us, Some(us(500)));
        assert_eq!(rows[1].gap_ticks, Some(48));
        assert_eq!(rows[1].snap, Some(4));
        // Two whole beats take the coarsest divisor that fits.
        assert_eq!(rows[2].gap_ticks, Some(384));
        assert_eq!(rows[2].snap, Some(1));
    }

    #[test]
    fn off_snap_gaps_have_no_snap_within_tolerance() {
        let chart = taps_at(&[0, 101, 238, 438, 448], vec![red(0, 400.0)]);
        let rows = feats(&chart);
        // 101 ms is 1/4 plus stable's integer-ms rounding.
        assert_eq!(rows[1].snap, Some(4));
        assert_eq!(rows[2].gap_ticks, Some(66));
        assert_eq!(rows[2].snap, None);
        assert_eq!(rows[3].snap, Some(2));
        // Faster than the finest divisor.
        assert_eq!(rows[4].snap, None);
    }

    #[test]
    fn invalid_red_lines_are_skipped() {
        let chart = taps_at(
            &[0, 125, 250],
            vec![
                red(0, 500.0),
                red(100, 0.0),
                red(200, f64::NAN),
                red(220, -3.0),
            ],
        );
        assert!(feats(&chart).iter().all(|r| r.beat_us == Some(us(500))));

        let none = taps_at(&[0, 125], vec![red(0, f64::INFINITY)]);
        assert!(feats(&none).iter().all(|r| r.beat_us.is_none()));
    }

    #[test]
    fn density_counts_presses_in_a_centred_window() {
        let chart = chart![step = 100;
            "x......",
            "x......",
            "x......",
            "x......",
            "x......",
            "xx.....",
            "x......",
            "x......",
            "x......",
            "x......",
        ];
        let rows = feats(&chart);
        assert_eq!(rows[0].density_milli, 5_000);
        assert_eq!(rows[5].density_milli, 11_000);
        assert_eq!(rows[9].density_milli, 7_000);
    }

    #[test]
    fn layout_keymode_must_match_the_chart() {
        let chart = chart![step = 100; "x......"];
        let err = ChartView::new(
            &chart,
            &Layout::default_for(Keymode::K4),
            &PatternParams::default(),
        )
        .unwrap_err();
        assert_eq!(
            err,
            PatternsError::KeymodeMismatch {
                chart: 7,
                layout: 4
            }
        );
    }

    #[test]
    fn view_exposes_its_inputs() {
        let chart = chart![step = 100; "x......"];
        let layout = Layout::default_for(Keymode::K7);
        let view = ChartView::new(&chart, &layout, &PatternParams::default()).unwrap();
        assert_eq!(view.keymode(), Keymode::K7);
        assert_eq!(view.chart(), &chart);
        assert_eq!(view.layout().id(), layout.id());
    }
}

#[cfg(test)]
mod props {
    use proptest::prelude::*;
    use wolluf_chart::layout::preset_ids;
    use wolluf_chart::{
        Chart, ChartMeta, Diagnostics, Layout, Note, NoteKind, TimingKind, TimingPoint,
    };
    use wolluf_core::{ColMask, Keymode, TimeUs};

    use super::*;
    use crate::params::PatternParams;

    const K: Keymode = Keymode::K7;

    fn arb_input() -> impl Strategy<Value = (Vec<Note>, Vec<TimingPoint>, usize, bool)> {
        let note = (0i64..400, 0u8..7, prop::option::of(1i64..60)).prop_map(|(t, col, len)| {
            let t = TimeUs(t * 25_000);
            let kind = match len {
                None => NoteKind::Tap,
                Some(len) => NoteKind::Hold {
                    end: TimeUs(t.0 + len * 25_000),
                },
            };
            Note { t, col, kind }
        });
        let red = (0i32..10_000, 150u32..1_200).prop_map(|(t, beat)| TimingPoint {
            t: TimeUs::from_ms(t),
            kind: TimingKind::Uninherited {
                beat_len_ms: f64::from(beat),
                meter: 4,
            },
        });
        (
            prop::collection::vec(note, 0..150),
            prop::collection::vec(red, 0..4),
            0usize..5,
            any::<bool>(),
        )
    }

    fn build(notes: Vec<Note>, timing: Vec<TimingPoint>) -> Chart {
        Chart::from_notes(
            K,
            ChartMeta::default(),
            timing,
            notes,
            &mut Diagnostics::new(),
        )
    }

    fn layout(preset: usize, mirrored: bool) -> Layout {
        let id = preset_ids().nth(preset).unwrap();
        let layout = Layout::by_id(id).unwrap();
        if mirrored { layout.mirror() } else { layout }
    }

    fn union(a: ColMask, b: ColMask) -> ColMask {
        ColMask::from_bits(K, a.bits() | b.bits()).unwrap()
    }

    fn disjoint(a: ColMask, b: ColMask) -> bool {
        a.bits() & b.bits() == 0
    }

    proptest! {
        #[test]
        fn features_are_deterministic_and_consistent((notes, timing, preset, mirrored) in arb_input()) {
            let chart = build(notes, timing);
            let layout = layout(preset, mirrored);
            let params = PatternParams::default();
            let view = ChartView::new(&chart, &layout, &params).unwrap();
            let again = ChartView::new(&chart, &layout, &params).unwrap();
            prop_assert_eq!(view.rows(), again.rows());

            let rows = view.rows();
            prop_assert_eq!(rows.len(), chart.rows().len());
            let hands = view.hand_cols();
            prop_assert!(disjoint(hands.left, hands.right) && disjoint(hands.left, hands.both) && disjoint(hands.right, hands.both));
            prop_assert_eq!(union(union(hands.left, hands.right), hands.both), ColMask::full(K));

            let mut notes_total = 0u32;
            for (i, (feat, row)) in rows.iter().zip(chart.rows()).enumerate() {
                prop_assert_eq!(feat.t, row.t);
                prop_assert_eq!(feat.press, union(row.tap, row.ln_head));
                prop_assert_eq!(feat.release, row.ln_tail);
                prop_assert_eq!(feat.held, chart.hold_mask(row.t));
                prop_assert!(disjoint(feat.press, feat.release) && disjoint(feat.press, feat.held) && disjoint(feat.release, feat.held));
                prop_assert_eq!(feat.notes, feat.press.len());
                prop_assert!(feat.jack.is_subset_of(feat.press));
                prop_assert_eq!(feat.jacks, feat.jack.len());
                let h = feat.hands;
                prop_assert!(disjoint(h.left, h.right) && disjoint(h.left, h.both) && disjoint(h.right, h.both));
                prop_assert_eq!(union(union(h.left, h.right), h.both), feat.press);
                prop_assert!(h.left.is_subset_of(hands.left) && h.right.is_subset_of(hands.right) && h.both.is_subset_of(hands.both));

                match feat.prev_press {
                    Some(p) => {
                        prop_assert!(p < i);
                        prop_assert!(!rows[p].press.is_empty());
                        prop_assert!(rows[p + 1..i].iter().all(|r| r.press.is_empty()));
                        prop_assert_eq!(feat.press_gap_us, Some(feat.t.0 - rows[p].t.0));
                        if !feat.press.is_empty() {
                            let prev = rows[p].press;
                            prop_assert_eq!(feat.jack.bits(), feat.press.bits() & prev.bits());
                        }
                    }
                    None => {
                        prop_assert!(rows[..i].iter().all(|r| r.press.is_empty()));
                        prop_assert_eq!(feat.press_gap_us, None);
                        prop_assert!(feat.jack.is_empty());
                    }
                }
                if feat.press.is_empty() {
                    prop_assert!(feat.jack.is_empty());
                    prop_assert_eq!(feat.direction, Direction::Neutral);
                    prop_assert!(!feat.is_roll);
                }
                if feat.is_roll {
                    prop_assert!(feat.jack.is_empty());
                }
                match i {
                    0 => prop_assert_eq!(feat.gap_us, None),
                    _ => prop_assert_eq!(feat.gap_us, Some(feat.t.0 - rows[i - 1].t.0)),
                }
                for col in 0..7u8 {
                    let last = rows[..i].iter().rev().find(|r| r.press.contains(col));
                    prop_assert_eq!(feat.col_gap_us[usize::from(col)], last.map(|r| feat.t.0 - r.t.0));
                }
                prop_assert!(feat.col_gap_us[7..].iter().all(Option::is_none));
                prop_assert!(feat.gap_ticks.is_none() || feat.beat_us.is_some());
                prop_assert!(feat.snap.is_none() || feat.gap_ticks.is_some());
                let own = u64::from(feat.notes) * 1_000_000_000 / params.view.density_window_us as u64;
                prop_assert!(u64::from(feat.density_milli) >= own);
                notes_total += feat.notes;
            }
            let taps: u32 = chart.rows().iter().map(|r| r.tap.len()).sum();
            prop_assert_eq!(notes_total as usize, taps as usize + chart.ln_pairs().len());
        }

        #[test]
        fn mirroring_chart_and_layout_mirrors_the_features((notes, timing, preset, mirrored) in arb_input()) {
            let flipped: Vec<Note> = notes.iter().map(|n| Note { col: 6 - n.col, ..*n }).collect();
            let chart = build(notes, timing.clone());
            let mirror_chart = build(flipped, timing);
            let layout = layout(preset, mirrored);
            let mirror_layout = layout.mirror();
            let params = PatternParams::default();
            let a = ChartView::new(&chart, &layout, &params).unwrap();
            let b = ChartView::new(&mirror_chart, &mirror_layout, &params).unwrap();
            prop_assert_eq!(a.rows().len(), b.rows().len());
            for (x, y) in a.rows().iter().zip(b.rows()) {
                let mut expected = x.clone();
                expected.press = x.press.mirror(K);
                expected.release = x.release.mirror(K);
                expected.held = x.held.mirror(K);
                expected.jack = x.jack.mirror(K);
                expected.hands = HandMasks {
                    left: x.hands.left.mirror(K),
                    right: x.hands.right.mirror(K),
                    both: x.hands.both.mirror(K),
                };
                expected.direction = match x.direction {
                    Direction::Left => Direction::Right,
                    Direction::Right => Direction::Left,
                    other => other,
                };
                for col in 0..7usize {
                    expected.col_gap_us[col] = x.col_gap_us[6 - col];
                }
                prop_assert_eq!(y, &expected);
            }
        }
    }
}
