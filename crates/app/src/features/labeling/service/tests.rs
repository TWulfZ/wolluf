use std::collections::BTreeSet;

use wolluf_core::ErrorCode;

use super::*;
use crate::features::labeling::dto::{AnchorDto, LabelSubmitDto, SampleRequestDto, WindowOpDto};
use crate::features::library::testkit::{Map, osu_text, synced};
use crate::features::plays::testkit::Fixture;

const REGULAR_DAN: &str = "1 7K Dan Course - Regular Dan Phase";
const ALL_COLS: [u8; 7] = [1, 2, 3, 4, 5, 6, 7];

/// A 7K chart with a tap every 125 ms over 20 s; the title keeps the bytes unique.
fn long_map(title: &str) -> Map {
    let taps: Vec<(u8, i32)> = (0..160).map(|i| ((i % 7) as u8, i * 125)).collect();
    Map::new(title, 7, osu_text(7, title, &taps, &[]))
}

fn maps() -> Vec<Map> {
    vec![
        long_map("three").named(REGULAR_DAN, "3rd Dan"),
        long_map("eight").named(REGULAR_DAN, "8th Dan"),
        long_map("plain"),
    ]
}

fn request(seed: &str, round: u32, exclude: Vec<AnchorDto>) -> SampleRequestDto {
    SampleRequestDto {
        keymode: 7,
        seed: seed.to_owned(),
        round,
        window_ms: None,
        scale: None,
        level_min: None,
        level_max: None,
        exclude,
    }
}

fn submit(anchor: &AnchorDto, patterns: &[&str]) -> LabelSubmitDto {
    LabelSubmitDto {
        anchor: anchor.clone(),
        patterns: patterns.iter().map(|p| (*p).to_owned()).collect(),
        no_pattern: false,
        mixed: false,
        unsure: false,
        thumb_pref: None,
    }
}

fn anchor(md5: &str, t0_ms: i32, t1_ms: i32) -> AnchorDto {
    AnchorDto {
        md5: md5.to_owned(),
        t0_ms,
        t1_ms,
        cols: ALL_COLS.to_vec(),
    }
}

fn overlaps(a: &AnchorDto, b: &AnchorDto) -> bool {
    a.md5 == b.md5 && a.t0_ms < b.t1_ms && b.t0_ms < a.t1_ms
}

async fn library() -> (Fixture, Vec<Map>) {
    let maps = maps();
    let (f, _) = synced(&maps, &[]).await;
    (f, maps)
}

#[tokio::test(flavor = "multi_thread")]
async fn taxonomy_lists_the_keymode_patterns() {
    let (f, _) = library().await;
    let svc = f.ctx.labeling();
    let k7 = svc.taxonomy(7).unwrap();
    assert_eq!(k7.len(), 25);
    let minijack = k7.iter().find(|p| p.id == "regular.jack.minijack").unwrap();
    assert_eq!(
        (minijack.axis.as_str(), minijack.key.as_str()),
        ("7k.regular.jack", "mj")
    );
    assert_eq!(svc.taxonomy(4).unwrap_err().code, ErrorCode::InvalidInput);
}

#[tokio::test(flavor = "multi_thread")]
async fn sample_is_deterministic_and_skips_labelled_windows() {
    let (f, maps) = library().await;
    let svc = f.ctx.labeling();
    let first = svc.sample(request("42", 0, vec![])).await.unwrap().unwrap();
    assert_eq!(
        svc.sample(request("42", 0, vec![])).await.unwrap(),
        Some(first.clone())
    );
    assert_eq!(first.anchor.t1_ms - first.anchor.t0_ms, 4_000);
    assert_eq!(first.anchor.cols, ALL_COLS);
    let map = maps.iter().find(|m| m.md5 == first.anchor.md5).unwrap();
    assert_eq!(first.title, map.title);
    assert!(first.stratum.contains("/nps_"), "{first:?}");

    svc.submit(submit(&first.anchor, &["regular.stream.single"]))
        .await
        .unwrap();
    for round in 0..6 {
        let w = svc
            .sample(request("42", round, vec![]))
            .await
            .unwrap()
            .unwrap();
        assert!(
            !overlaps(&w.anchor, &first.anchor),
            "stored labels are avoided without being passed in: {w:?}"
        );
    }
    let mut seen = vec![first.anchor.clone()];
    for round in 0..9 {
        let Some(w) = svc
            .sample(request("42", round, seen.clone()))
            .await
            .unwrap()
        else {
            break;
        };
        assert!(
            !overlaps(&w.anchor, &first.anchor),
            "round {round} reuses a labelled window: {w:?}"
        );
        assert!(!seen.iter().any(|s| overlaps(s, &w.anchor)), "{w:?}");
        seen.push(w.anchor);
    }
    let strata: BTreeSet<String> = futures_strata(&svc).await;
    assert!(strata.len() >= 3, "dan 3, dan 8 and unlevelled: {strata:?}");
}

async fn futures_strata(svc: &LabelingService<'_>) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for round in 0..3 {
        if let Some(w) = svc.sample(request("7", round, vec![])).await.unwrap() {
            out.insert(w.stratum);
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn sample_filters_by_label_and_window() {
    let (f, maps) = library().await;
    let svc = f.ctx.labeling();
    let eight = &maps[1];
    for round in 0..3 {
        let w = svc
            .sample(SampleRequestDto {
                scale: Some("jinjin_dan_regular".into()),
                level_min: Some(5.0),
                window_ms: Some(2_000),
                ..request("1", round, vec![])
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(w.anchor.md5, eight.md5);
        assert_eq!(w.anchor.t1_ms - w.anchor.t0_ms, 2_000);
        assert_eq!(w.level.as_deref(), Some("jinjin_dan_regular:8th"));
    }
    let bad_seed = svc.sample(request("-1", 0, vec![])).await.unwrap_err();
    assert_eq!(bad_seed.code, ErrorCode::InvalidInput);
    let zero = svc
        .sample(SampleRequestDto {
            window_ms: Some(0),
            ..request("1", 0, vec![])
        })
        .await
        .unwrap_err();
    assert_eq!(zero.code, ErrorCode::InvalidInput);
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_undo_and_stats_roundtrip() {
    let (f, maps) = library().await;
    let svc = f.ctx.labeling();
    let (three, eight) = (&maps[0].md5, &maps[1].md5);
    let a = svc
        .submit(LabelSubmitDto {
            mixed: true,
            ..submit(
                &anchor(three, 1_000, 5_000),
                &["regular.stream.single", "regular.jack.minijack"],
            )
        })
        .await
        .unwrap();
    let b = svc
        .submit(LabelSubmitDto {
            unsure: true,
            ..submit(&anchor(eight, 0, 4_000), &["regular.jack.minijack"])
        })
        .await
        .unwrap();
    assert!(b.id > a.id, "ids follow append order");

    let stats = svc.stats().await.unwrap();
    assert_eq!((stats.total, stats.mixed, stats.unsure), (2, 1, 1));
    let count = |list: &[crate::features::labeling::dto::CountDto], key: &str| {
        list.iter().find(|c| c.key == key).map(|c| c.count)
    };
    assert_eq!(count(&stats.per_pattern, "regular.jack.minijack"), Some(2));
    assert_eq!(count(&stats.per_pattern, "regular.stream.single"), Some(1));
    assert_eq!(count(&stats.per_axis, "7k.regular.jack"), Some(2));
    assert_eq!(count(&stats.per_axis, "7k.regular.stream"), Some(1));
    let strata: u32 = stats.per_stratum.iter().map(|c| c.count).sum();
    assert_eq!(strata, 2);
    assert!(
        stats
            .per_stratum
            .iter()
            .any(|c| c.key.starts_with("dan_08/")),
        "{stats:?}"
    );

    svc.undo(&a.id).await.unwrap();
    let stats = svc.stats().await.unwrap();
    assert_eq!((stats.total, stats.mixed), (1, 0));
    assert_eq!(svc.undo(&a.id).await.unwrap_err().code, ErrorCode::Conflict);
    let unknown = ulid::Ulid::from_parts(1, 1).to_string();
    assert_eq!(
        svc.undo(&unknown).await.unwrap_err().code,
        ErrorCode::NotFound
    );
    assert_eq!(
        svc.undo("not a ulid").await.unwrap_err().code,
        ErrorCode::InvalidInput
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_rejects_bad_input() {
    let (f, maps) = library().await;
    let svc = f.ctx.labeling();
    let md5 = &maps[0].md5;
    let ok = anchor(md5, 0, 4_000);
    let cases = [
        (submit(&ok, &[]), ErrorCode::InvalidInput),
        (submit(&ok, &["regular.jack.nope"]), ErrorCode::InvalidInput),
        (submit(&ok, &["mj"]), ErrorCode::InvalidInput),
        (
            submit(&anchor(md5, 4_000, 4_000), &["regular.jack.minijack"]),
            ErrorCode::InvalidInput,
        ),
        (
            submit(
                &AnchorDto {
                    cols: vec![0],
                    ..ok.clone()
                },
                &["regular.jack.minijack"],
            ),
            ErrorCode::InvalidInput,
        ),
        (
            submit(
                &AnchorDto {
                    cols: vec![8],
                    ..ok.clone()
                },
                &["regular.jack.minijack"],
            ),
            ErrorCode::InvalidInput,
        ),
        (
            submit(&anchor("nope", 0, 4_000), &["regular.jack.minijack"]),
            ErrorCode::InvalidInput,
        ),
        (
            submit(
                &anchor(&"0".repeat(32), 0, 4_000),
                &["regular.jack.minijack"],
            ),
            ErrorCode::NotFound,
        ),
    ];
    for (req, code) in cases {
        let err = svc.submit(req.clone()).await.unwrap_err();
        assert_eq!(err.code, code, "{req:?}");
    }
    assert_eq!(svc.stats().await.unwrap().total, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn export_is_sorted_anchors_and_ids_only() {
    let (f, maps) = library().await;
    let svc = f.ctx.labeling();
    let mut md5s: Vec<&str> = maps.iter().map(|m| m.md5.as_str()).collect();
    md5s.sort_unstable();
    svc.submit(LabelSubmitDto {
        unsure: true,
        mixed: true,
        ..submit(&anchor(md5s[1], 8_000, 12_000), &["regular.stream.roll"])
    })
    .await
    .unwrap();
    // Neither append order nor its reverse is the export order.
    svc.submit(submit(
        &anchor(md5s[0], 4_000, 8_000),
        &["regular.stream.trill", "regular.jack.anchor"],
    ))
    .await
    .unwrap();
    let undone = svc
        .submit(submit(
            &anchor(md5s[2], 0, 4_000),
            &["regular.stream.trill"],
        ))
        .await
        .unwrap();
    svc.submit(submit(
        &anchor(md5s[1], 0, 4_000),
        &["regular.stream.trill"],
    ))
    .await
    .unwrap();
    svc.undo(&undone.id).await.unwrap();

    let text = svc.export_jsonl().await.unwrap();
    let lines: Vec<serde_json::Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 3, "{text}");
    let keys: Vec<(String, i64)> = lines
        .iter()
        .map(|l| {
            (
                l["md5"].as_str().unwrap().to_owned(),
                l["t0_us"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        keys,
        [
            (md5s[0].to_owned(), 4_000_000),
            (md5s[1].to_owned(), 0),
            (md5s[1].to_owned(), 8_000_000)
        ]
    );
    assert_eq!(
        lines[0],
        serde_json::json!({
            "md5": md5s[0],
            "t0_us": 4_000_000,
            "t1_us": 8_000_000,
            "cols": ALL_COLS,
            "patterns": ["regular.jack.anchor", "regular.stream.trill"],
            "no_pattern": false,
            "flags": [],
            "thumb_pref": null,
            "labelled_at": "2026-09-28T23:13:56.636Z",
        })
    );
    assert_eq!(lines[2]["flags"], serde_json::json!(["mixed", "unsure"]));
    for m in &maps {
        assert!(!text.contains(&m.title), "no titles: {text}");
    }
    assert!(
        !text.contains("TWulfZ") && !text.contains("wolluf -"),
        "{text}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn export_to_writes_the_file_outside_the_osu_folder() {
    let (f, maps) = library().await;
    let svc = f.ctx.labeling();
    svc.submit(submit(
        &anchor(&maps[0].md5, 0, 4_000),
        &["regular.stream.trill"],
    ))
    .await
    .unwrap();
    let out = f.dir.path().join("out/labels/gold.jsonl");
    let done = svc.export_to(out.clone()).await.unwrap();
    assert_eq!(done.rows, 1);
    assert_eq!(done.path, out.to_string_lossy());
    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        svc.export_jsonl().await.unwrap()
    );
    let inside = f.root.join("labels.jsonl");
    let err = svc.export_to(inside.clone()).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    assert!(!inside.exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn resolve_patterns_takes_keys_or_ids() {
    let (f, _) = library().await;
    let svc = f.ctx.labeling();
    let tokens = |t: &[&str]| t.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    let got = svc
        .resolve_patterns(7, &tokens(&["CJ", "regular.stream.jumpstream", "cj", "a"]))
        .unwrap();
    let ids: Vec<&str> = got.iter().map(|p| p.as_str()).collect();
    assert_eq!(
        ids,
        [
            "regular.jack.chordjack",
            "regular.stream.jumpstream",
            "regular.jack.anchor"
        ],
        "input order, deduplicated"
    );
    let unknown = svc.resolve_patterns(7, &tokens(&["js", "zz"])).unwrap_err();
    assert_eq!(unknown.code, ErrorCode::InvalidInput);
    assert_eq!(unknown.message_key, super::super::keys::UNKNOWN_PATTERN);
    assert_eq!(unknown.args["pattern"], "zz");
    assert_eq!(
        svc.resolve_patterns(7, &[]).unwrap_err().code,
        ErrorCode::InvalidInput
    );
    assert_eq!(
        svc.resolve_patterns(4, &tokens(&["js"])).unwrap_err().code,
        ErrorCode::InvalidInput
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn reshape_clamps_to_the_chart_rows() {
    let (f, maps) = library().await;
    let svc = f.ctx.labeling();
    let md5 = &maps[0].md5;
    // Rows run from 0 to 19.875 s, so the chart span ends at 19.876 s.
    let at = |a: AnchorDto| (a.t0_ms, a.t1_ms);
    let reshape = |a: AnchorDto, op| svc.reshape(a, op);
    assert_eq!(
        at(reshape(anchor(md5, 1_000, 5_000), WindowOpDto::Prev)
            .await
            .unwrap()),
        (0, 4_000)
    );
    assert_eq!(
        at(reshape(anchor(md5, 15_000, 19_000), WindowOpDto::Next)
            .await
            .unwrap()),
        (15_876, 19_876)
    );
    assert_eq!(
        at(reshape(anchor(md5, 15_876, 19_876), WindowOpDto::Widen)
            .await
            .unwrap()),
        (14_876, 19_876),
        "at the end, widening grows backwards"
    );
    assert_eq!(
        at(reshape(anchor(md5, 0, 2_000), WindowOpDto::Narrow)
            .await
            .unwrap()),
        (0, 1_000)
    );
    let short = reshape(anchor(md5, 0, 1_000), WindowOpDto::Narrow)
        .await
        .unwrap_err();
    assert_eq!(short.code, ErrorCode::InvalidInput);
    assert_eq!(short.message_key, super::super::keys::WINDOW_TOO_SHORT);
    let unknown = reshape(anchor(&"0".repeat(32), 0, 4_000), WindowOpDto::Next).await;
    assert_eq!(unknown.unwrap_err().code, ErrorCode::NotFound);
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_rejects_windows_outside_the_chart_and_overlaps() {
    let (f, maps) = library().await;
    let svc = f.ctx.labeling();
    let md5 = &maps[0].md5;
    for (t0, t1) in [(-500, 3_500), (17_000, 21_000)] {
        let err = svc
            .submit(submit(&anchor(md5, t0, t1), &["regular.stream.trill"]))
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput, "{t0}..{t1}");
    }
    let first = svc
        .submit(submit(
            &anchor(md5, 4_000, 8_000),
            &["regular.stream.trill"],
        ))
        .await
        .unwrap();
    let overlap = svc
        .submit(submit(
            &anchor(md5, 7_999, 11_999),
            &["regular.stream.roll"],
        ))
        .await
        .unwrap_err();
    assert_eq!(overlap.code, ErrorCode::Conflict);
    svc.submit(submit(
        &anchor(md5, 8_000, 12_000),
        &["regular.stream.roll"],
    ))
    .await
    .expect("touching windows do not overlap");
    svc.submit(submit(
        &anchor(&maps[1].md5, 4_000, 8_000),
        &["regular.stream.roll"],
    ))
    .await
    .expect("another chart");
    svc.undo(&first.id).await.unwrap();
    svc.submit(submit(&anchor(md5, 4_000, 8_000), &["regular.stream.roll"]))
        .await
        .expect("an undone label frees its window");
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_and_undo_need_the_self_profile() {
    let f = Fixture::new(&wolluf_source_osu::testkit::FakeInstall::new()).await;
    let svc = f.ctx.labeling();
    let err = svc
        .submit(submit(
            &anchor(&"0".repeat(32), 0, 4_000),
            &["regular.stream.trill"],
        ))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    assert_eq!(err.args["profile"], "self");
    let err = svc
        .undo(&ulid::Ulid::from_parts(1, 1).to_string())
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    assert_eq!(err.args["profile"], "self");
    assert_eq!(svc.stats().await.unwrap().total, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn another_profiles_label_is_neither_undone_nor_counted() {
    use wolluf_core::{ColMask, Keymode, PatternId, SegmentAnchor, TimeUs};
    use wolluf_store::repo::labels::{NewGoldLabel, append_gold_label};
    use wolluf_store::repo::players::{MergeMode, NewProfile, ProfileKind, profile};

    let (f, maps) = library().await;
    let md5: wolluf_core::ChartMd5 = maps[0].md5.parse().unwrap();
    let id = ulid::Ulid::from_parts(1, 1);
    f.ctx
        .user_db()
        .write(move |tx| {
            let other = profile::insert(
                tx,
                &NewProfile {
                    kind: ProfileKind::Other,
                    label: "Rival".into(),
                    is_default: false,
                    merge_mode: MergeMode::Merged,
                    created_at: wolluf_core::UnixUs(0),
                },
            )?;
            append_gold_label(
                tx,
                &NewGoldLabel {
                    id,
                    ts: wolluf_core::UnixUs(0),
                    profile_id: other,
                    keymode: Keymode::K7,
                    anchor: SegmentAnchor::new(
                        md5,
                        TimeUs::from_ms(0),
                        TimeUs::from_ms(4_000),
                        ColMask::full(Keymode::K7),
                        Keymode::K7,
                    )
                    .unwrap(),
                    answer: wolluf_store::repo::labels::GoldAnswer::Patterns(vec![
                        PatternId::from_static("regular.stream.trill"),
                    ]),
                    mixed: false,
                    unsure: false,
                    thumb_pref: None,
                    app_version: "test".into(),
                },
            )
        })
        .unwrap();
    let svc = f.ctx.labeling();
    assert_eq!(
        svc.undo(&id.to_string()).await.unwrap_err().code,
        ErrorCode::NotFound
    );
    assert_eq!(svc.stats().await.unwrap().total, 0);
    assert!(svc.export_jsonl().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn played_means_played_by_the_self_profile() {
    use crate::features::library::testkit::install;
    use crate::features::plays::testkit::scores_db;
    use wolluf_source_osu::testkit::ScoreBuilder;

    let maps = maps();
    let (three, eight) = (&maps[0], &maps[1]);
    let inst = install(&maps, &[])
        .cfg("fixture", "Username = TWulfZ\r\n")
        .scores_db(scores_db(&[
            ScoreBuilder::mania(&three.md5, "TWulfZ", 1),
            ScoreBuilder::mania(&eight.md5, "Kovacs", 2),
        ]));
    let f = Fixture::new(&inst).await;
    f.sync().await;
    let svc = f.ctx.labeling();
    let window = |min: Option<f64>, max: Option<f64>| {
        svc.sample(SampleRequestDto {
            scale: Some("jinjin_dan_regular".into()),
            level_min: min,
            level_max: max,
            ..request("3", 0, vec![])
        })
    };
    let mine = window(None, Some(5.0)).await.unwrap().unwrap();
    assert_eq!(
        (mine.anchor.md5.as_str(), mine.played),
        (three.md5.as_str(), true)
    );
    let theirs = window(Some(5.0), None).await.unwrap().unwrap();
    assert_eq!(
        (theirs.anchor.md5.as_str(), theirs.played),
        (eight.md5.as_str(), false),
        "another player's play is not the user's"
    );
}

#[test]
fn export_target_resolves_relative_paths_and_guards_installs() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().join("work");
    let osu = dir.path().join("osu!");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&osu).unwrap();
    let roots = [osu.clone()];
    let canon = |p: &std::path::Path| std::fs::canonicalize(p).unwrap();

    let rel = export_target(
        std::path::Path::new("fixtures/labels/g.jsonl"),
        &cwd,
        &roots,
    )
    .unwrap();
    assert_eq!(rel, cwd.join("fixtures/labels/g.jsonl"));

    for bad in ["../osu!/x.jsonl", "a/../../osu!/x.jsonl", ".."] {
        let err = export_target(std::path::Path::new(bad), &cwd, &roots).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput, "{bad}");
    }
    let inside_rel = export_target(std::path::Path::new("sub/new/x.jsonl"), &osu, &roots);
    assert_eq!(inside_rel.unwrap_err().code, ErrorCode::InvalidInput);
    let inside_abs = export_target(&canon(&osu).join("x.jsonl"), &cwd, &roots);
    assert_eq!(inside_abs.unwrap_err().code, ErrorCode::InvalidInput);
    assert!(
        !osu.join("sub").exists(),
        "nothing created inside the install"
    );

    #[cfg(unix)]
    {
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&osu, &link).unwrap();
        let via_link = export_target(&link.join("new/x.jsonl"), &cwd, &roots);
        assert_eq!(via_link.unwrap_err().code, ErrorCode::InvalidInput);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn export_to_rejects_parent_components() {
    let (f, _) = library().await;
    let err = f
        .ctx
        .labeling()
        .export_to(f.dir.path().join("out/../x.jsonl"))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    assert!(!f.dir.path().join("x.jsonl").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn no_pattern_answers_and_thumb_sides_are_stored_counted_and_exported() {
    use crate::features::labeling::dto::ThumbPrefDto;

    let (f, maps) = library().await;
    let svc = f.ctx.labeling();
    let md5 = &maps[0].md5;
    svc.submit(LabelSubmitDto {
        no_pattern: true,
        thumb_pref: Some(ThumbPrefDto::Left),
        ..submit(&anchor(md5, 0, 4_000), &[])
    })
    .await
    .unwrap();
    svc.submit(LabelSubmitDto {
        thumb_pref: Some(ThumbPrefDto::Right),
        ..submit(&anchor(md5, 4_000, 8_000), &["regular.tech.thumb"])
    })
    .await
    .unwrap();
    for bad in [
        LabelSubmitDto {
            no_pattern: true,
            ..submit(&anchor(md5, 8_000, 12_000), &["regular.stream.trill"])
        },
        submit(&anchor(md5, 8_000, 12_000), &[]),
    ] {
        assert_eq!(
            svc.submit(bad.clone()).await.unwrap_err().code,
            ErrorCode::InvalidInput,
            "{bad:?}"
        );
    }

    let stats = svc.stats().await.unwrap();
    assert_eq!(
        (
            stats.total,
            stats.no_pattern,
            stats.thumb_left,
            stats.thumb_right
        ),
        (2, 1, 1, 1)
    );
    assert_eq!(stats.per_pattern.len(), 1, "{stats:?}");
    assert_eq!(stats.per_stratum.iter().map(|c| c.count).sum::<u32>(), 2);

    let text = svc.export_jsonl().await.unwrap();
    let lines: Vec<serde_json::Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines[0]["patterns"], serde_json::json!([]));
    assert_eq!(lines[0]["no_pattern"], true);
    assert_eq!(lines[0]["thumb_pref"], "left");
    assert_eq!(lines[1]["no_pattern"], false);
    assert_eq!(lines[1]["thumb_pref"], "right");
}
