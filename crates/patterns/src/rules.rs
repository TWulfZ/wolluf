//! The rule registry. Rules live in `rules/<id>.rs`, one file each (architecture §9.2).

mod anchor;
mod bracket;
mod burst;
mod chordbracket;
mod chordjack;
mod chordstream_dense;
mod chordstream_light;
mod common;
mod hand_imbalance;
mod handstream;
mod irregular;
mod jumpstream;
mod jumptrill;
mod ln_chord;
mod ln_common;
mod ln_density;
mod ln_hybrid;
mod ln_inverse;
mod ln_release;
mod ln_shield;
mod longjack;
mod minijack;
mod roll;
mod single;
mod split_trill;
mod thumb;
mod trill;

pub use anchor::Anchor;
pub use bracket::Bracket;
pub use burst::Burst;
pub use chordbracket::Chordbracket;
pub use chordjack::Chordjack;
pub use chordstream_dense::ChordstreamDense;
pub use chordstream_light::ChordstreamLight;
pub use hand_imbalance::HandImbalance;
pub use handstream::Handstream;
pub use irregular::Irregular;
pub use jumpstream::Jumpstream;
pub use jumptrill::Jumptrill;
pub use ln_chord::LnChord;
pub use ln_density::LnDensity;
pub use ln_hybrid::LnHybrid;
pub use ln_inverse::LnInverse;
pub use ln_release::LnRelease;
pub use ln_shield::LnShield;
pub use longjack::Longjack;
pub use minijack::Minijack;
pub use roll::Roll;
pub use single::Single;
pub use split_trill::SplitTrill;
pub use thumb::Thumb;
pub use trill::Trill;

use crate::rule::PatternRule;

/// Fixed order: overlap resolution falls back to it after priority and strength, so appending
/// is safe and reordering changes outputs. Taxonomy order within each axis.
pub fn all() -> &'static [&'static dyn PatternRule] {
    &[
        &Minijack,
        &Chordjack,
        &Longjack,
        &Anchor,
        &Single,
        &Jumpstream,
        &Handstream,
        &ChordstreamLight,
        &ChordstreamDense,
        &Roll,
        &Trill,
        &Jumptrill,
        &SplitTrill,
        &Bracket,
        &Chordbracket,
        &Irregular,
        &HandImbalance,
        &Thumb,
        &Burst,
        &LnDensity,
        &LnChord,
        &LnHybrid,
        &LnShield,
        &LnInverse,
        &LnRelease,
    ]
}

#[cfg(test)]
pub(crate) mod testkit {
    use wolluf_chart::{
        Chart, ChartMeta, Diagnostics, Layout, Note, NoteKind, TimingKind, TimingPoint,
    };
    use wolluf_core::{ColMask, Keymode, PatternId, TimeUs};

    use crate::params::PatternParams;
    use crate::rule::{Candidate, PatternRule};
    use crate::view::ChartView;

    pub(crate) fn mask(cols: &[u8]) -> ColMask {
        ColMask::from_cols(Keymode::K7, cols.iter().copied()).unwrap()
    }

    pub(crate) fn ms(t: i32) -> TimeUs {
        TimeUs::from_ms(t)
    }

    pub(crate) fn cand(
        pattern: PatternId,
        t0: i32,
        t1: i32,
        cols: &[u8],
        strength: u32,
    ) -> Candidate {
        Candidate {
            pattern,
            t0: ms(t0),
            t1: ms(t1),
            cols: mask(cols),
            strength,
        }
    }

    pub(crate) fn detect(rule: &dyn PatternRule, chart: &Chart) -> Vec<Candidate> {
        detect_with(rule, chart, &Layout::default_for(chart.keymode()))
    }

    pub(crate) fn detect_with(
        rule: &dyn PatternRule,
        chart: &Chart,
        layout: &Layout,
    ) -> Vec<Candidate> {
        let params = PatternParams::default();
        let view = ChartView::new(chart, layout, &params).unwrap();
        rule.detect(&view, &params)
    }

    /// Same result under both 3|1+3 thumb sides: jack rules never read hands.
    pub(crate) fn detect_both_thumbs(rule: &dyn PatternRule, chart: &Chart) -> Vec<Candidate> {
        let right = detect_with(rule, chart, &Layout::by_id("k7.313_right_thumb").unwrap());
        let left = detect_with(rule, chart, &Layout::by_id("k7.313_left_thumb").unwrap());
        assert_eq!(right, left);
        right
    }

    pub(crate) fn layout(id: &str) -> Layout {
        Layout::by_id(id).unwrap()
    }

    /// Single notes cycling through `cols`, the first at 0 and one more after each gap.
    pub(crate) fn seq(gaps_ms: &[i32], cols: &[u8], beat_len_ms: Option<f64>) -> Chart {
        let mut t = 0;
        let mut notes = vec![(0, cols[0])];
        for (i, gap) in gaps_ms.iter().enumerate() {
            t += gap;
            notes.push((t, cols[(i + 1) % cols.len()]));
        }
        taps(&notes, beat_len_ms)
    }

    /// Taps at `(ms, col)` in 7K, optionally under one red line at 0.
    pub(crate) fn taps(notes: &[(i32, u8)], beat_len_ms: Option<f64>) -> Chart {
        let notes = notes
            .iter()
            .map(|&(t, col)| Note {
                t: ms(t),
                col,
                kind: NoteKind::Tap,
            })
            .collect();
        Chart::from_notes(
            Keymode::K7,
            ChartMeta::default(),
            red(beat_len_ms),
            notes,
            &mut Diagnostics::new(),
        )
    }

    /// `chart` with one red line at 0.
    pub(crate) fn with_beat(chart: &Chart, beat_len_ms: f64) -> Chart {
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
            red(Some(beat_len_ms)),
            taps.chain(holds).collect(),
            &mut Diagnostics::new(),
        )
    }

    fn red(beat_len_ms: Option<f64>) -> Vec<TimingPoint> {
        beat_len_ms
            .map(|beat_len_ms| TimingPoint {
                t: TimeUs::ZERO,
                kind: TimingKind::Uninherited {
                    beat_len_ms,
                    meter: 4,
                },
            })
            .into_iter()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn rule_ids_are_unique() {
        let ids: BTreeSet<String> = all().iter().map(|r| r.id().to_string()).collect();
        assert_eq!(ids.len(), all().len());
    }

    #[test]
    fn registry_order_is_fixed() {
        let ids: Vec<String> = all().iter().map(|r| r.id().to_string()).collect();
        assert_eq!(
            ids,
            [
                "regular.jack.minijack",
                "regular.jack.chordjack",
                "regular.jack.longjack",
                "regular.jack.anchor",
                "regular.stream.single",
                "regular.stream.jumpstream",
                "regular.stream.handstream",
                "regular.stream.chordstream_light",
                "regular.stream.chordstream_dense",
                "regular.stream.roll",
                "regular.stream.trill",
                "regular.stream.jumptrill",
                "regular.stream.split_trill",
                "regular.stream.bracket",
                "regular.stream.chordbracket",
                "regular.tech.irregular",
                "regular.tech.hand_imbalance",
                "regular.tech.thumb",
                "regular.speed.burst",
                "ln.general.density",
                "ln.general.chord",
                "ln.tech.hybrid",
                "ln.tech.shield",
                "ln.inverse.gap",
                "ln.release.timing",
            ]
        );
        assert!(all().iter().all(|r| r.version() >= 1));
    }
}

#[cfg(test)]
pub(crate) mod generate {
    //! Chart generators shared by the rule and segmenter property tests.

    use proptest::prelude::*;
    use wolluf_chart::layout::preset_ids;
    use wolluf_chart::{
        Chart, ChartMeta, Diagnostics, Layout, Note, NoteKind, TimingKind, TimingPoint,
    };
    use wolluf_core::{Keymode, TimeUs};

    const K: Keymode = Keymode::K7;

    pub(crate) fn arb_input() -> impl Strategy<Value = (Vec<Note>, Vec<TimingPoint>, usize)> {
        // Coarse times so jacks, chords and anchors actually occur.
        let note = (0i64..120, 0u8..7, prop::option::of(1i64..8)).prop_map(|(t, col, len)| {
            let t = TimeUs(t * 80_000);
            let kind = match len {
                None => NoteKind::Tap,
                Some(len) => NoteKind::Hold {
                    end: TimeUs(t.0 + len * 40_000),
                },
            };
            Note { t, col, kind }
        });
        let red = (0i32..5_000, 150u32..900).prop_map(|(t, beat)| TimingPoint {
            t: TimeUs::from_ms(t),
            kind: TimingKind::Uninherited {
                beat_len_ms: f64::from(beat),
                meter: 4,
            },
        });
        let scattered = prop::collection::vec(note, 0..200);
        // Alternations of disjoint masks and repeated shapes, which scattered notes almost
        // never form (jumptrills, dense chordstreams, brackets).
        let segment = (
            prop_oneof![(0u8..7).prop_map(|c| 1u16 << c), 1u16..128],
            any::<u16>(),
            2usize..9,
            any::<bool>(),
        );
        let shaped = prop::collection::vec(segment, 1..12).prop_map(|segments| {
            let mut notes = Vec::new();
            let mut row = 0i64;
            for (a, raw, len, alternate) in segments {
                let rest = !a & 0x7f;
                let b = if rest & raw == 0 { rest } else { rest & raw };
                for step in 0..len {
                    let mask = if alternate && step % 2 == 1 { b } else { a };
                    notes.extend((0u8..7).filter(|c| mask & (1 << c) != 0).map(|col| Note {
                        t: TimeUs(row * 80_000),
                        col,
                        kind: NoteKind::Tap,
                    }));
                    row += 1;
                }
            }
            notes
        });
        // Long-note sections: chords of LNs with short gaps, taps under them, shields and
        // staggered tails, so every LN rule gets material.
        let ln_segment = (
            1u16..128,
            1i64..6,
            1i64..4,
            1usize..6,
            any::<u16>(),
            0i64..60,
            any::<bool>(),
        );
        let ln_shaped = prop::collection::vec(ln_segment, 1..8).prop_map(|segments| {
            let mut notes = Vec::new();
            let mut row = 1i64;
            for (a, len, gap, repeats, taps, stagger_ms, shield) in segments {
                for _ in 0..repeats {
                    for col in (0u8..7).filter(|c| a & (1 << c) != 0) {
                        let head = row * 80_000;
                        if shield {
                            notes.push(Note {
                                t: TimeUs(head - 80_000),
                                col,
                                kind: NoteKind::Tap,
                            });
                        }
                        let end = (row + len) * 80_000 + i64::from(col) * stagger_ms * 1_000;
                        notes.push(Note {
                            t: TimeUs(head),
                            col,
                            kind: NoteKind::Hold { end: TimeUs(end) },
                        });
                    }
                    for col in (0u8..7).filter(|c| taps & !a & (1 << c) != 0) {
                        notes.push(Note {
                            t: TimeUs((row + 1) * 80_000),
                            col,
                            kind: NoteKind::Tap,
                        });
                    }
                    row += len + gap;
                }
            }
            notes
        });
        // Inverse-style blocks: long LNs on most columns with one-row gaps.
        let inverse_segment = (prop_oneof![Just(0x7fu16), 0x3fu16..128], 3i64..9, 2usize..6);
        let inverse_shaped = prop::collection::vec(inverse_segment, 1..4).prop_map(|segments| {
            let mut notes = Vec::new();
            let mut row = 0i64;
            for (a, len, repeats) in segments {
                for _ in 0..repeats {
                    notes.extend((0u8..7).filter(|c| a & (1 << c) != 0).map(|col| Note {
                        t: TimeUs(row * 80_000),
                        col,
                        kind: NoteKind::Hold {
                            end: TimeUs((row + len) * 80_000),
                        },
                    }));
                    row += len + 1;
                }
            }
            notes
        });
        (
            prop_oneof![2 => scattered, 3 => shaped, 2 => ln_shaped, 1 => inverse_shaped],
            prop::collection::vec(red, 0..3),
            0usize..5,
        )
    }

    pub(crate) fn build(notes: Vec<Note>, timing: Vec<TimingPoint>) -> Chart {
        Chart::from_notes(
            K,
            ChartMeta::default(),
            timing,
            notes,
            &mut Diagnostics::new(),
        )
    }

    pub(crate) fn preset_layout(preset: usize) -> Layout {
        Layout::by_id(preset_ids().nth(preset).unwrap()).unwrap()
    }
}

#[cfg(test)]
mod props {
    use proptest::prelude::*;
    use wolluf_chart::Note;
    use wolluf_core::{ColMask, Keymode};

    use super::generate::{arb_input, build, preset_layout as layout};
    use super::*;
    use crate::params::PatternParams;
    use crate::rule::{Candidate, STRENGTH_MAX};
    use crate::view::ChartView;

    const K: Keymode = Keymode::K7;

    fn mirrored(c: &Candidate) -> Candidate {
        Candidate {
            cols: c.cols.mirror(K),
            ..c.clone()
        }
    }

    proptest! {
        #[test]
        fn candidates_are_sorted_in_bounds_and_on_pressed_columns((notes, timing, preset) in arb_input()) {
            let chart = build(notes, timing);
            let layout = layout(preset);
            let params = PatternParams::default();
            let view = ChartView::new(&chart, &layout, &params).unwrap();
            for rule in all() {
                let found = rule.detect(&view, &params);
                prop_assert_eq!(&found, &rule.detect(&view, &params));
                prop_assert!(found.windows(2).all(|w| w[0] < w[1]), "{} not strictly sorted", rule.id());
                for c in &found {
                    prop_assert_eq!(&c.pattern, &rule.id());
                    prop_assert!(c.t0 <= c.t1);
                    prop_assert!(c.strength <= STRENGTH_MAX);
                    prop_assert!(!c.cols.is_empty());
                    let span: Vec<_> = view.rows().iter().filter(|r| c.t0 <= r.t && r.t <= c.t1).collect();
                    // LN rules may start or end on a release and name held columns.
                    let ln = c.pattern.as_str().starts_with("ln.");
                    prop_assert!(span.first().is_some_and(|r| r.t == c.t0 && (ln || !r.press.is_empty())));
                    prop_assert!(span.last().is_some_and(|r| r.t == c.t1 && (ln || !r.press.is_empty())));
                    let touched = span.iter().fold(0u16, |acc, r| {
                        acc | r.press.bits() | if ln { r.release.bits() | r.held.bits() } else { 0 }
                    });
                    prop_assert!(c.cols.is_subset_of(ColMask::from_bits(K, touched).unwrap()), "{}", rule.id());
                }
            }
        }

        #[test]
        fn hand_agnostic_rules_are_mirror_symmetric((notes, timing, preset) in arb_input()) {
            let flipped: Vec<Note> = notes.iter().map(|n| Note { col: 6 - n.col, ..*n }).collect();
            let chart = build(notes, timing.clone());
            let mirror_chart = build(flipped, timing);
            let layout = layout(preset);
            let params = PatternParams::default();
            let a = ChartView::new(&chart, &layout, &params).unwrap();
            let b = ChartView::new(&mirror_chart, &layout, &params).unwrap();
            let agnostic: [&dyn PatternRule; 21] = [
                &Minijack, &Chordjack, &Longjack, &Anchor, &Single, &Jumpstream, &Handstream,
                &ChordstreamLight, &ChordstreamDense, &Roll, &Trill, &Jumptrill, &Chordbracket,
                &Irregular, &Burst, &LnDensity, &LnChord, &LnHybrid, &LnShield, &LnInverse,
                &LnRelease,
            ];
            for rule in agnostic {
                let mut expected: Vec<Candidate> = rule.detect(&a, &params).iter().map(mirrored).collect();
                expected.sort();
                prop_assert_eq!(rule.detect(&b, &params), expected, "{}", rule.id());
            }
        }

        #[test]
        fn every_rule_is_mirror_symmetric_with_a_mirrored_layout((notes, timing, preset) in arb_input()) {
            let flipped: Vec<Note> = notes.iter().map(|n| Note { col: 6 - n.col, ..*n }).collect();
            let chart = build(notes, timing.clone());
            let mirror_chart = build(flipped, timing);
            let layout = layout(preset);
            let mirror_layout = layout.mirror();
            let params = PatternParams::default();
            let a = ChartView::new(&chart, &layout, &params).unwrap();
            let b = ChartView::new(&mirror_chart, &mirror_layout, &params).unwrap();
            for rule in all() {
                let mut expected: Vec<Candidate> = rule.detect(&a, &params).iter().map(mirrored).collect();
                expected.sort();
                prop_assert_eq!(rule.detect(&b, &params), expected, "{}", rule.id());
            }
        }
    }
}
