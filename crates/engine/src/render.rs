//! ASCII playfield window for looking at a chart section before the UI Playfield exists.
//!
//! Orientation is the game's: the earliest row is at the bottom, time grows upwards, and the
//! last line is the key row (one finger initial per column). Columns are drawn in physical
//! order, so under a mirrored layout the notes are flipped exactly as the Mirror mod shows
//! them, while the hands stay put.
//!
//! One line per row (rows exist only at events) with its `mm:ss.mmm` time. Cells: `o` tap,
//! `H` LN head, `:` LN body, `T` LN tail, `.` empty. Between columns, `|` marks a hand change
//! and `+` sets a thumb apart from the rest of its hand, so the right-thumb 7K preset reads
//! `rmi|t+imr` like its `3|1+3` name. A gap longer than [`RenderOpts::gap_ellipsis`] becomes a
//! `...` line that still shows the LN bodies held through it.

use wolluf_chart::{Chart, Finger, Hand, Layout};
use wolluf_core::{ColMask, TimeUs};

pub const TAP: char = 'o';
pub const LN_HEAD: char = 'H';
pub const LN_BODY: char = ':';
pub const LN_TAIL: char = 'T';
pub const EMPTY: char = '.';
pub const HAND_SPLIT: char = '|';
pub const THUMB_SPLIT: char = '+';
const ELLIPSIS: &str = "...";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderOpts {
    /// Consecutive rows further apart than this get an ellipsis line between them.
    pub gap_ellipsis: TimeUs,
    /// Row annotations, e.g. pattern segments; the first matching mark labels a row.
    pub marks: Vec<RowMark>,
}

impl Default for RenderOpts {
    fn default() -> Self {
        Self {
            gap_ellipsis: TimeUs::from_ms(1_000),
            marks: Vec::new(),
        }
    }
}

/// Labels the rows with `t0 <= t <= t1`: `* label` on the row at `t0`, `| label` on the rest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowMark {
    pub t0: TimeUs,
    pub t1: TimeUs,
    pub label: String,
}

const MARK_START: char = '*';
const MARK_CONTINUES: char = '|';

fn mark_of(marks: &[RowMark], t: TimeUs) -> Option<String> {
    let m = marks.iter().find(|m| m.t0 <= t && t <= m.t1)?;
    let lead = if m.t0 == t {
        MARK_START
    } else {
        MARK_CONTINUES
    };
    Some(format!("  {lead} {}", m.label))
}

/// Rows with `from <= t < to`. A layout of another keymode falls back to the chart's generic
/// layout; the key row names the layout actually used.
pub fn render_window(
    chart: &Chart,
    layout: &Layout,
    from: TimeUs,
    to: TimeUs,
    opts: &RenderOpts,
) -> String {
    let fallback;
    let layout = if layout.keymode() == chart.keymode() {
        layout
    } else {
        fallback = Layout::generic(chart.keymode());
        &fallback
    };
    let field = Field::new(layout);

    let rows = chart.rows();
    let start = rows.partition_point(|r| r.t < from);
    let end = rows.partition_point(|r| r.t < to);
    let window = rows.get(start..end).unwrap_or_default();
    // Negative times and minutes past 99 widen `timestamp`; one width keeps the cells aligned.
    let width = window
        .iter()
        .map(|r| timestamp(r.t).len())
        .max()
        .unwrap_or(0)
        .max(ELLIPSIS.len());

    let mut lines = Vec::with_capacity(window.len() + 1);
    let mut prev: Option<TimeUs> = None;
    for row in window {
        if let Some(p) = prev
            && row.t.0.saturating_sub(p.0) > opts.gap_ellipsis.0
        {
            let mid = TimeUs(p.0 + (row.t.0 - p.0) / 2);
            let held = chart.hold_mask(mid);
            let cells = field.draw(|col| if held.contains(col) { LN_BODY } else { ' ' });
            lines.push(format!("{ELLIPSIS:>width$}  {cells}").trim_end().to_owned());
        }
        let held = chart.hold_mask(row.t);
        let cells = field.draw(|col| glyph(row.tap, row.ln_head, row.ln_tail, held, col));
        let mark = mark_of(&opts.marks, row.t).unwrap_or_default();
        lines.push(format!("{:>width$}  {cells}{mark}", timestamp(row.t)));
        prev = Some(row.t);
    }

    let mut out = String::new();
    for line in lines.iter().rev() {
        out.push_str(line);
        out.push('\n');
    }
    let keys = field.draw(|col| layout.column(col).map_or(EMPTY, |(_, f)| finger_initial(f)));
    let mirrored = if layout.is_mirrored() {
        " mirrored"
    } else {
        ""
    };
    out.push_str(&format!(
        "{:width$}  {keys}  {}{mirrored}\n",
        "",
        layout.id()
    ));
    out
}

fn glyph(tap: ColMask, head: ColMask, tail: ColMask, held: ColMask, col: u8) -> char {
    if tap.contains(col) {
        TAP
    } else if head.contains(col) {
        LN_HEAD
    } else if tail.contains(col) {
        LN_TAIL
    } else if held.contains(col) {
        LN_BODY
    } else {
        EMPTY
    }
}

/// Screen positions left to right, each with its chart column and the separator before it.
struct Field {
    cols: Vec<(u8, Option<char>)>,
}

impl Field {
    fn new(layout: &Layout) -> Self {
        let keys = layout.keymode().columns();
        let chart_col = |p: u8| {
            if layout.is_mirrored() {
                keys - 1 - p
            } else {
                p
            }
        };
        let mut cols = Vec::with_capacity(usize::from(keys));
        let mut prev: Option<(Hand, Finger)> = None;
        for p in 0..keys {
            let col = chart_col(p);
            let here = layout.column(col);
            let sep = match (prev, here) {
                (Some((h0, _)), Some((h1, _))) if h0 != h1 => Some(HAND_SPLIT),
                (Some((_, f0)), Some((_, f1))) if f0 == Finger::Thumb || f1 == Finger::Thumb => {
                    Some(THUMB_SPLIT)
                }
                _ => None,
            };
            cols.push((col, sep));
            prev = here;
        }
        Self { cols }
    }

    fn draw(&self, cell: impl Fn(u8) -> char) -> String {
        let mut out = String::with_capacity(self.cols.len() * 2);
        for &(col, sep) in &self.cols {
            if let Some(sep) = sep {
                out.push(sep);
            }
            out.push(cell(col));
        }
        out
    }
}

fn finger_initial(finger: Finger) -> char {
    match finger {
        Finger::Pinky => 'p',
        Finger::Ring => 'r',
        Finger::Middle => 'm',
        Finger::Index => 'i',
        Finger::Thumb => 't',
    }
}

/// `mm:ss.mmm`, floored to the millisecond; minutes widen past 99 and negatives get a `-`.
pub fn timestamp(t: TimeUs) -> String {
    let ms = t.as_ms_floor();
    let sign = if ms < 0 { "-" } else { "" };
    let abs = ms.unsigned_abs();
    format!(
        "{sign}{:02}:{:02}.{:03}",
        abs / 60_000,
        abs / 1_000 % 60,
        abs % 1_000
    )
}

#[cfg(test)]
mod tests {
    use wolluf_chart::{ChartMeta, Diagnostics, Layout, Note, NoteKind, chart};
    use wolluf_core::{Keymode, TimeUs};

    use super::*;

    fn s(ms: i32) -> TimeUs {
        TimeUs::from_ms(ms)
    }

    fn k7() -> Layout {
        Layout::by_id("k7.313_right_thumb").unwrap()
    }

    // Rows every 125 ms from 0 to 1750 ms, then a 3 s break with an LN held across it.
    fn sample() -> Chart {
        chart![step = 125;
            "x..x...",
            ".x...x.",
            "[.x....",
            "|..x..x",
            "|x..x..",
            "]..[..x",
            "...|.x.",
            "x..|...",
            "...|...",
            "...|...",
            "...|...",
            "...|...",
            "...|...",
            "...|...",
            "...|...",
            "...|...",
            "...|...",
            "...|...",
            "...|...",
            "...|...",
            "...|...",
            "...|...",
            "...|...",
            "...|...",
            "...|...",
            "...|...",
            "...|...",
            "...|...",
            "...|...",
            "...|...",
            "...|...",
            "...|...",
            "...|...",
            "...|...",
            "...|...",
            "...|...",
            "...|...",
            "...|...",
            "...|...",
            "...]..x",
            "xx.....",
        ]
    }

    #[test]
    fn default_layout() {
        let out = render_window(&sample(), &k7(), s(0), s(10_000), &RenderOpts::default());
        insta::assert_snapshot!(out);
    }

    #[test]
    fn mirrored_layout_draws_what_the_player_sees() {
        let out = render_window(
            &sample(),
            &k7().mirror(),
            s(0),
            s(10_000),
            &RenderOpts::default(),
        );
        insta::assert_snapshot!(out);
    }

    #[test]
    fn earliest_row_is_at_the_bottom_above_the_key_row() {
        let chart = chart![step = 100; "x......", ".x.....", "..x...."];
        let out = render_window(&chart, &k7(), s(0), s(1_000), &RenderOpts::default());
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines,
            [
                "00:00.200  ..o|.+...",
                "00:00.100  .o.|.+...",
                "00:00.000  o..|.+...",
                "           rmi|t+imr  k7.313_right_thumb",
            ]
        );
    }

    #[test]
    fn marks_annotate_the_rows_they_cover_and_flag_the_first() {
        let chart = chart![step = 100; "x......", ".x.....", "..x....", "...x..."];
        let opts = RenderOpts {
            marks: vec![RowMark {
                t0: s(100),
                t1: s(200),
                label: "st".to_owned(),
            }],
            ..RenderOpts::default()
        };
        let out = render_window(&chart, &k7(), s(0), s(1_000), &opts);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines,
            [
                "00:00.300  ...|o+...",
                "00:00.200  ..o|.+...  | st",
                "00:00.100  .o.|.+...  * st",
                "00:00.000  o..|.+...",
                "           rmi|t+imr  k7.313_right_thumb",
            ]
        );
    }

    #[test]
    fn window_is_half_open() {
        let chart = chart![step = 100; "x......", ".x.....", "..x....", "...x..."];
        let out = render_window(&chart, &k7(), s(100), s(300), &RenderOpts::default());
        let times: Vec<&str> = out.lines().map(|l| &l[..9]).collect();
        assert_eq!(times, ["00:00.200", "00:00.100", "         "]);
    }

    #[test]
    fn ln_body_shows_when_head_is_before_the_window() {
        let chart = chart![step = 100; "[......", "|x.....", "]......"];
        let out = render_window(&chart, &k7(), s(100), s(1_000), &RenderOpts::default());
        let rows: Vec<&str> = out.lines().map(|l| &l[11..]).collect();
        assert_eq!(
            rows,
            ["T..|.+...", ":o.|.+...", "rmi|t+imr  k7.313_right_thumb"]
        );
    }

    #[test]
    fn gap_threshold_is_a_parameter() {
        let chart = chart![step = 500; "x......", ".......", ".x....."];
        let tight = RenderOpts {
            gap_ellipsis: s(999),
            ..RenderOpts::default()
        };
        let loose = RenderOpts {
            gap_ellipsis: s(1_000),
            ..RenderOpts::default()
        };
        let with = render_window(&chart, &k7(), s(0), s(2_000), &tight);
        let without = render_window(&chart, &k7(), s(0), s(2_000), &loose);
        assert_eq!(with.lines().count(), 4);
        assert_eq!(with.lines().nth(1), Some("      ...     | +"));
        assert_eq!(without.lines().count(), 3);
    }

    #[test]
    fn separators_follow_the_layout() {
        let chart = chart![step = 100; "x.....x"];
        let footer = |id: &str| {
            let out = render_window(
                &chart,
                &Layout::by_id(id).unwrap(),
                s(0),
                s(1_000),
                &RenderOpts::default(),
            );
            out.lines().map(|l| l[11..].to_owned()).collect::<Vec<_>>()
        };
        assert_eq!(
            footer("k7.313_left_thumb"),
            ["o..+.|..o", "rmi+t|imr  k7.313_left_thumb"]
        );
        assert_eq!(
            footer("k7.both_thumbs"),
            ["o..|.|..o", "rmi|t|imr  k7.both_thumbs"]
        );
        assert_eq!(footer("k7.43"), ["o...|..o", "prmi|imr  k7.43"]);
    }

    #[test]
    fn generic_over_keymode() {
        let chart = chart![step = 100; "x..[", ".x.|", "..x]"];
        let out = render_window(
            &chart,
            &Layout::default_for(Keymode::K4),
            s(0),
            s(1_000),
            &RenderOpts::default(),
        );
        assert_eq!(
            out,
            "00:00.200  ..|oT\n00:00.100  .o|.:\n00:00.000  o.|.H\n           mi|im  k4.generic\n"
        );
    }

    #[test]
    fn layout_of_another_keymode_falls_back_to_generic() {
        let chart = chart![step = 100; "x..x"];
        let out = render_window(&chart, &k7(), s(0), s(1_000), &RenderOpts::default());
        assert!(out.ends_with("mi|im  k4.generic\n"), "{out}");
    }

    #[test]
    fn time_column_widens_to_the_longest_timestamp_in_the_window() {
        let notes = vec![
            Note {
                t: s(-500),
                col: 0,
                kind: NoteKind::Tap,
            },
            Note {
                t: s(-500),
                col: 3,
                kind: NoteKind::Hold { end: s(6_000_000) },
            },
            Note {
                t: s(6_000_000),
                col: 1,
                kind: NoteKind::Tap,
            },
        ];
        let chart = Chart::from_notes(
            Keymode::K4,
            ChartMeta::default(),
            Vec::new(),
            notes,
            &mut Diagnostics::new(),
        );
        let out = render_window(
            &chart,
            &Layout::default_for(Keymode::K4),
            s(-1_000),
            s(6_001_000),
            &RenderOpts::default(),
        );
        assert_eq!(
            out.lines().collect::<Vec<_>>(),
            [
                "100:00.000  .o|.T",
                "       ...    | :",
                "-00:00.500  o.|.H",
                "            mi|im  k4.generic",
            ]
        );
    }

    #[test]
    fn timestamps() {
        assert_eq!(timestamp(TimeUs(61_234_567)), "01:01.234");
        assert_eq!(timestamp(TimeUs(0)), "00:00.000");
        assert_eq!(timestamp(TimeUs(-500_000)), "-00:00.500");
        assert_eq!(timestamp(TimeUs(-1)), "-00:00.001");
        assert_eq!(timestamp(TimeUs(6_000_000_000)), "100:00.000");
    }
}
