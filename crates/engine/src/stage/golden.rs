//! Stage-lock goldens: a canonical text dump of each stage's outputs over synthetic fixtures,
//! hashed with blake3. Floats are quantised or printed in Rust's shortest round-trip form, so
//! the dump is identical on every platform (D3).

use std::fmt::Write as _;

use wolluf_chart::testkit::OsuText;
use wolluf_chart::{Chart, ChartError, TimingKind, chart};
use wolluf_core::TimeUs;

use wolluf_chart::Layout;
use wolluf_chart::testkit::chart_from_rows;
use wolluf_patterns::PatternsError;

use super::chart_parse::{parse_chart, summarize};
use crate::error::EngineError;
use crate::labels::{LabelInput, extract_labels};
use crate::rows_blob::{decode_rows, encode_rows};

/// Ratios are pinned to 1e-6: finer bits would make the golden depend on float formatting.
const RATIO_SCALE: f64 = 1_000_000.0;

pub(super) fn chart_parse() -> String {
    hex(&chart_parse_dump())
}

fn chart_parse_dump() -> String {
    let mut dump = String::new();
    for (name, bytes) in chart_parse_fixtures() {
        let _ = writeln!(dump, "fixture {name}");
        match parse_chart(&bytes) {
            Ok(parsed) => {
                for d in parsed.diagnostics.iter() {
                    let _ = writeln!(dump, "diag {} {}", d.code, d.detail);
                }
                // The cache serves the decoded blob, so that is what the golden pins.
                match encode_rows(&parsed.chart).and_then(|blob| decode_rows(&blob)) {
                    Ok(chart) => dump_chart(&mut dump, &chart),
                    Err(err) => {
                        let _ = writeln!(dump, "blob_error {}", error_id(&err));
                    }
                }
            }
            Err(err) => {
                let _ = writeln!(dump, "error {}", error_id(&err));
            }
        }
    }
    dump
}

pub(super) fn chart_label() -> String {
    hex(&chart_label_dump())
}

fn chart_label_dump() -> String {
    let mut dump = String::new();
    for (folder, version, creator, set_id) in LABEL_FIXTURES {
        let _ = writeln!(dump, "input {folder}|{version}|{creator}|{set_id:?}");
        let input = LabelInput {
            folder,
            version,
            creator,
            set_id: *set_id,
        };
        for l in extract_labels(&input) {
            let _ = writeln!(
                dump,
                "label {}|{}|{:?}|{}|{:?}|{}",
                l.source, l.scale, l.level_ord, l.level_text, l.skill_tag, l.is_variant
            );
        }
    }
    dump
}

pub(super) fn patterns() -> String {
    hex(&patterns_dump())
}

/// One line per segment, every field spelled out; ids are the persisted stable strings.
fn patterns_dump() -> String {
    let mut dump = String::new();
    for (name, chart, layouts) in patterns_fixtures() {
        for layout_id in layouts {
            let _ = writeln!(dump, "fixture {name} layout {layout_id}");
            let layout = Layout::by_id(layout_id);
            let (Some(chart), Some(layout)) = (&chart, layout) else {
                let _ = writeln!(dump, "error fixture");
                continue;
            };
            match super::patterns::run(chart, &layout) {
                Ok(segments) => {
                    for (i, s) in segments.iter().enumerate() {
                        let secondary: Vec<&str> = s.secondary.iter().map(|p| p.as_str()).collect();
                        let _ = writeln!(
                            dump,
                            "segment {i} {} {} cols={} {} {} purity={} strength={} secondary={}",
                            s.t0.0,
                            s.t1.0,
                            s.cols.bits(),
                            s.primary.as_str(),
                            s.axis.as_str(),
                            s.purity,
                            s.strength,
                            secondary.join(",")
                        );
                    }
                }
                Err(err) => {
                    let _ = writeln!(dump, "error {}", error_id(&err));
                }
            }
        }
    }
    dump
}

const PATTERNS_STEP_MS: i32 = 100;

const RIGHT_AND_LEFT_THUMB: [&str; 2] = ["k7.313_right_thumb", "k7.313_left_thumb"];

fn rows_chart(rows: &[&str]) -> Option<Chart> {
    chart_from_rows(0, PATTERNS_STEP_MS, rows).ok()
}

/// `cycle` repeated to `n` rows, then two empty rows so the next section is not the same stream.
fn section(rows: &mut Vec<&'static str>, cycle: &[&'static str], n: usize) {
    rows.extend(cycle.iter().copied().cycle().take(n));
    rows.extend([".......", "......."]);
}

/// Together the fixtures put every ADR 0017 pattern id in the dump, as a primary or a tag
/// (`patterns_golden_covers_every_pattern_id`): rice, jacks, streams, LN sections, off-grid
/// timing, under both 3|1+3 thumb sides (the thumb column changes hand-dependent rules), plus
/// a keymode without an axis table.
fn patterns_fixtures() -> Vec<(&'static str, Option<Chart>, Vec<&'static str>)> {
    let jumpstream = [
        "x......", "..xx...", "x......", ".x..x..", "..x....", "x....x.", "...x...", ".x..x..",
    ];
    let hybrid = [
        "x.....[", "..x...|", ".x....|", "...x..]", "..x...[", "....x.|", "...x..|", ".x....]",
    ];
    let mut mixed: Vec<&'static str> = Vec::new();
    for _ in 0..4 {
        mixed.extend(jumpstream);
    }
    mixed.extend([
        ".......", ".......", "xxx....", "xxx....", "xx.x...", "xx.x...", "x.xx...",
    ]);
    mixed.extend([".......", ".......", "......."]);
    for block in 0..4 {
        mixed.push("[[[[[[[");
        mixed.extend(["|||||||"; 5]);
        mixed.push("]]]]]]]");
        if block < 3 {
            mixed.push(".......");
        }
    }
    mixed.push(".......");
    for _ in 0..3 {
        mixed.extend(hybrid);
    }

    let mut streams: Vec<&'static str> = Vec::new();
    let zigzag = [
        "x......", ".x.....", "..x....", "...x...", "....x..", ".....x.", "......x", ".....x.",
        "....x..", "...x...", "..x....", ".x.....",
    ];
    section(&mut streams, &zigzag, 24);
    section(
        &mut streams,
        &[
            "xxx....", "....x..", "...x.xx", ".x.....", "x.x.x..", "......x",
        ],
        24,
    );
    section(
        &mut streams,
        &[
            "xx.xx..", "..x..x.", "xx..x.x", "..xx...", "xx...xx", "..x....",
        ],
        24,
    );
    // One note per hand per row, so no hand can bracket.
    section(
        &mut streams,
        &["x...x..", ".x...x.", "..x...x", ".x...x."],
        24,
    );
    section(&mut streams, &["xx.....", "..xx..."], 24);
    section(&mut streams, &["x.x....", ".x....."], 24);

    let mut ln: Vec<&'static str> = Vec::new();
    for _ in 0..4 {
        ln.extend([
            "[[[[...", "||||...", "]|||...", ".]||...", "..]|...", "...]...", ".......",
        ]);
    }
    ln.extend([".......", "......."]);
    ln.extend([
        "x......", "[......", "|x.....", "][.....", ".|x....", ".][....", "..|x...", "..][...",
        "...|x..", "...][..", "....|x.", "....][.", ".....|x", ".....][", "......]",
    ]);

    let thumb_trill: Vec<&'static str> = ["...x...", "....x.."]
        .iter()
        .copied()
        .cycle()
        .take(24)
        .collect();
    vec![
        (
            "k7_mixed",
            rows_chart(&mixed),
            RIGHT_AND_LEFT_THUMB.to_vec(),
        ),
        (
            "k7_streams",
            rows_chart(&streams),
            vec!["k7.313_right_thumb"],
        ),
        (
            "k7_ln_release_shield",
            rows_chart(&ln),
            vec!["k7.313_right_thumb"],
        ),
        ("k7_off_grid", off_grid(), vec!["k7.313_right_thumb"]),
        (
            "k7_thumb_trill",
            rows_chart(&thumb_trill),
            RIGHT_AND_LEFT_THUMB.to_vec(),
        ),
        (
            "k4_generic",
            rows_chart(&["x...", ".x..", "..x.", "...x"]),
            vec!["k4.generic"],
        ),
    ]
}

/// A single-note stream alternating 1/4 and 1/3 gaps of a 400 ms beat: the DSL draws no red
/// lines, so this one goes through the decoder.
fn off_grid() -> Option<Chart> {
    let mut text = OsuText::mania(7).timing_line("0,400,4,1,0,100,1,0");
    let cols = [0, 2, 1, 3];
    let mut t = 0;
    for i in 0..22 {
        text = text.tap(cols[i % cols.len()], TimeUs::from_ms(t));
        t += if i % 2 == 0 { 100 } else { 133 };
    }
    parse_chart(text.build().as_bytes()).ok().map(|p| p.chart)
}

fn hex(dump: &str) -> String {
    blake3::hash(dump.as_bytes()).to_hex().to_string()
}

/// Stable ids, not `Display` prose, so rewording a message does not move the golden.
fn error_id(err: &EngineError) -> String {
    match err {
        EngineError::Chart(ChartError::UnsupportedMode(mode)) => format!("unsupported_mode {mode}"),
        EngineError::Chart(ChartError::InvalidMode(_)) => "invalid_mode".to_owned(),
        EngineError::Chart(ChartError::Read(_)) => "read".to_owned(),
        EngineError::Core(_) => "core".to_owned(),
        EngineError::EmptyRowsBlob => "empty_rows_blob".to_owned(),
        EngineError::UnsupportedRowsFormat(v) => format!("unsupported_rows_format {v}"),
        EngineError::CorruptRowsBlob(_) => "corrupt_rows_blob".to_owned(),
        EngineError::Patterns(PatternsError::KeymodeMismatch { chart, layout }) => {
            format!("keymode_mismatch {chart} {layout}")
        }
    }
}

fn dump_chart(dump: &mut String, chart: &Chart) {
    let s = summarize(chart);
    let quantise = |v: f64| (v * RATIO_SCALE).round() as i64;
    let _ = writeln!(
        dump,
        "summary keys={} notes={} lns={} ln_ratio_e6={} length_ms={} nps_e6={}",
        s.keymode.columns(),
        s.n_notes,
        s.n_ln,
        quantise(s.ln_ratio),
        s.length_ms,
        quantise(s.nps)
    );
    let m = chart.meta();
    let _ = writeln!(
        dump,
        "meta {}|{}|{}|{}|{:?}|{:?}|{}|{}|{}|{:?}",
        m.title,
        m.artist,
        m.version,
        m.creator,
        m.set_id,
        m.beatmap_id,
        m.od,
        m.hp,
        m.audio_filename,
        m.format_version
    );
    // Spelled out, not `{:?}`: renaming a Rust variant or field must not move the golden.
    for p in chart.timing() {
        let _ = match p.kind {
            TimingKind::Uninherited { beat_len_ms, meter } => writeln!(
                dump,
                "timing {} uninherited beat_len_ms={beat_len_ms} meter={meter}",
                p.t.0
            ),
            TimingKind::Inherited { sv } => writeln!(dump, "timing {} inherited sv={sv}", p.t.0),
        };
    }
    for r in chart.rows() {
        let _ = writeln!(
            dump,
            "row {} {} {} {}",
            r.t.0,
            r.tap.bits(),
            r.ln_head.bits(),
            r.ln_tail.bits()
        );
    }
    for ln in chart.ln_pairs() {
        let _ = writeln!(dump, "ln {} {} {}", ln.head.0, ln.tail.0, ln.col);
    }
}

fn chart_parse_fixtures() -> Vec<(&'static str, Vec<u8>)> {
    let osu = |text: OsuText| text.build().into_bytes();
    let from = |chart: &Chart| OsuText::from_chart(chart);
    let ms = TimeUs::from_ms;
    vec![
        (
            "k7_rice_chords",
            osu(from(&chart![step = 125;
                "x..x...",
                ".x...x.",
                "..x.x..",
                "x.....x",
                "xxx.xxx",
                "...x...",
                ".x.x.x.",
            ])),
        ),
        (
            "k7_ln_mix",
            osu(from(&chart![step = 100, start = 1500;
                "[..x..[",
                "|.x.x.|",
                "|x...x]",
                "]..x...",
                "[[[.[[[",
                "|||.|||",
                "]]]x]]]",
            ])),
        ),
        (
            "k7_timing_and_meta",
            osu(
                from(&chart![step = 250; "x.....x", ".x...x.", "..x.x..", "...x..."])
                    .metadata("Title", "Golden")
                    .metadata("Artist", "wolluf")
                    .metadata("BeatmapSetID", "42")
                    .difficulty("OverallDifficulty", "8.5")
                    .difficulty("HPDrainRate", "7")
                    .timing_line("0,333.3333333,4,1,0,100,1,0")
                    .timing_line("500,-50,4,1,0,100,0,0")
                    .timing_line("750,-125,4,1,0,100,0,0"),
            ),
        ),
        (
            "k4_generic",
            osu(from(&chart![step = 100; "x..[", ".x.|", "..x]", "x..x"])),
        ),
        ("k7_empty", osu(OsuText::mania(7))),
        (
            "k7_diagnostics",
            osu(OsuText::mania(7)
                .tap(1, ms(100))
                .tap(1, ms(100))
                .hold(2, ms(300), ms(200))
                .hold(3, ms(400), ms(900))
                .tap(3, ms(600))
                .hit_object("not,a,hit,object")),
        ),
        ("not_mania", osu(OsuText::mania(7).general("Mode", "0"))),
    ]
}

const LABEL_FIXTURES: &[(&str, &str, &str, Option<i32>)] = &[
    (
        "Jinjin - 7K Dan Course - Regular Dan Phase",
        "9th",
        "Jinjin",
        None,
    ),
    (
        "Jinjin - 7K Dan Course - Regular Dan Phase",
        "Gamma",
        "Jinjin",
        None,
    ),
    (
        "Jinjin - 7K Dan Course - LN Dan Phase",
        "Azimuth",
        "Jinjin",
        None,
    ),
    (
        "1 Jinjin - 7K Dan Course - Insane Level 2 (LN)",
        "5th",
        "Jinjin",
        None,
    ),
    ("Emperor's 7K LN Dan", "3rd", "Emperor", None),
    (
        "1877617 Various Artists - KomeijiDove 7K Jack Practice",
        "~ 9th ~ Some Song (1.1x)",
        "KomeijiDove",
        Some(1877617),
    ),
    ("Wild 7K Dan Course", "10th", "Wild", None),
    (
        "7K Road to Gamma Dan Pack",
        "Road to Gamma // Speed 1.1x",
        "Jinjin",
        None,
    ),
    (
        "[_BMS_] Artist - Song",
        "[n2_12+] [sr_3-] [st_0]",
        "5ynt3ck",
        None,
    ),
    ("[_BMS_] Artist - Song", "[BMS] [i_4]", "5ynt3ck", None),
    ("[_O2Jam_] A - T", "[O2Jam] [H] [9]", "Mapper", None),
    ("Gamma Practice Pack", "Gamma (Jack) 1", "Someone", None),
    ("Some Artist - Song", "Delete Upon download", "x", None),
    ("Some Artist - Song", "Hard", "x", None),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixtures_exercise_errors_diagnostics_and_edge_charts() {
        let dump = chart_parse_dump();
        for expected in [
            "fixture not_mania\nerror unsupported_mode 0\n",
            "diag chart.duplicate_note ",
            "diag chart.ln_tail_not_after_head ",
            "diag chart.overlapping_note ",
            "diag osu.malformed_line ",
            "fixture k7_empty\nsummary keys=7 notes=0 lns=0 ln_ratio_e6=0 length_ms=0 nps_e6=0\n",
            "summary keys=4 ",
            "timing 0 uninherited beat_len_ms=333.3333333 meter=4\n",
            "timing 500000 inherited sv=2\n",
            "|Some(42)|None|8.5|7|",
        ] {
            assert!(dump.contains(expected), "missing {expected:?} in\n{dump}");
        }
        assert!(!dump.contains("blob_error"));
        let labels = chart_label_dump();
        assert!(labels.contains("label bms_5ynt3ck|bms_n2|"), "{labels}");
        assert!(
            labels.contains("label komeijidove_practice|jinjin_dan|"),
            "{labels}"
        );
    }

    #[test]
    fn patterns_golden_covers_every_pattern_id() {
        let dump = patterns_dump();
        let mut seen = std::collections::BTreeSet::new();
        for line in dump.lines().filter(|l| l.starts_with("segment ")) {
            let fields: Vec<&str> = line.split(' ').collect();
            seen.insert(fields[5].to_owned());
            let secondary = line.rsplit_once("secondary=").unwrap().1;
            seen.extend(
                secondary
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned),
            );
        }
        let missing: Vec<&str> = wolluf_patterns::axes::K7
            .iter()
            .map(|(p, _)| p.as_str())
            .filter(|p| !seen.contains(*p))
            .collect();
        assert!(
            missing.is_empty(),
            "never in the golden: {missing:?}\n{dump}"
        );
    }

    #[test]
    fn patterns_golden_spells_out_every_segment_field() {
        let dump = patterns_dump();
        for expected in [
            "fixture k7_mixed layout k7.313_right_thumb\n",
            "fixture k7_mixed layout k7.313_left_thumb\n",
            "fixture k7_thumb_trill layout k7.313_right_thumb\nsegment 0 0 2300000 cols=24 regular.stream.trill ",
            "fixture k7_thumb_trill layout k7.313_left_thumb\nsegment 0 0 2300000 cols=24 regular.stream.split_trill ",
            " regular.stream.jumpstream 7k.regular.stream ",
            " ln.inverse.gap 7k.ln.inverse ",
            " purity=",
            " strength=",
            " secondary=",
        ] {
            assert!(dump.contains(expected), "missing {expected:?} in\n{dump}");
        }
        assert!(!dump.contains("error"), "{dump}");
        // No axis table for 4K yet, so no segments.
        assert!(
            dump.ends_with("fixture k4_generic layout k4.generic\n"),
            "{dump}"
        );
    }
}
