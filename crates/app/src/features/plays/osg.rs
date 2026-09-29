//! `.osg` spike tooling (spec 006): a read-only dump of one file and a structural survey of a
//! whole `Data/r`. Neither is an IPC surface, so nothing here derives specta (D13).

mod dump;
mod survey;

pub use dump::{
    DiagnosticRow, DumpFormat, DumpView, OsgDump, OsgDumpHeader, OsgEventRow, OsgRecordRow,
    inspect, render_diagnostics, render_dump,
};
pub use survey::{
    Distributions, FileFailure, I5Failure, I6Failure, InvariantRow, MissingOsg, OsgSurveyReport,
    SourceReport, SurveyOptions, SurveySources, render_survey, survey,
};

#[cfg(test)]
mod tests {
    use std::path::Path;

    use wolluf_core::ErrorCode;
    use wolluf_source_osu::codec::osg::{OsgFile, OsgRecord, OsgScoreSystem};
    use wolluf_source_osu::codec::score_header::JudgementCounts;
    use wolluf_source_osu::testkit::encode_osg;

    use super::*;

    const CLIENT: i32 = 20_260_924;

    fn counts(max: u16, n300: u16, n200: u16, miss: u16) -> JudgementCounts {
        JudgementCounts {
            geki: max,
            n300,
            katu: n200,
            miss,
            ..JudgementCounts::default()
        }
    }

    fn rec(
        t_ms: i32,
        c: JudgementCounts,
        score: i32,
        combo: u16,
        v2: Option<[f64; 2]>,
    ) -> OsgRecord {
        OsgRecord {
            t_ms,
            counts: c,
            score,
            max_combo: combo,
            combo,
            hp_raw: 200,
            b4: 0,
            b25: 0,
            b28: u8::from(v2.is_some()),
            v2,
        }
    }

    /// A single MAX, a two-note chord (MAX + 300), a 200, a miss that resets combo, and the
    /// trailing score-only record stable writes. The miss record carries `b25 = 1`, which is
    /// reserved away from the final record (ADR 0012 item 7), so the dump shows a diagnostic.
    fn v1_fixture() -> OsgFile {
        let mut last = rec(1_900, counts(2, 1, 1, 1), 1_205, 0, None);
        last.max_combo = 4;
        let mut miss = rec(1_500, counts(2, 1, 1, 1), 1_200, 0, None);
        miss.max_combo = 4;
        miss.b25 = 1;
        OsgFile {
            client_version: CLIENT,
            score_system: Some(OsgScoreSystem::V1),
            records: vec![
                rec(1_000, counts(1, 0, 0, 0), 320, 1, None),
                rec(1_250, counts(2, 1, 0, 0), 940, 3, None),
                rec(1_400, counts(2, 1, 1, 0), 1_140, 4, None),
                miss,
                last,
            ],
        }
    }

    fn v2_fixture() -> OsgFile {
        OsgFile {
            client_version: CLIENT,
            score_system: Some(OsgScoreSystem::V2),
            records: vec![
                rec(500, counts(1, 0, 0, 0), 150, 1, Some([150.0, 0.0])),
                rec(750, counts(2, 0, 0, 0), 300, 2, Some([300.0, 0.0])),
                rec(
                    900,
                    counts(2, 1, 0, 0),
                    537,
                    3,
                    Some([537.744_375_108_173_4, 0.0]),
                ),
            ],
        }
    }

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn dump_of(osg: &OsgFile) -> OsgDump {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "fixture.osg", &encode_osg(osg));
        inspect(&path).unwrap()
    }

    #[test]
    fn inspect_missing_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let err = inspect(&dir.path().join("absent.osg")).unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound);
        assert!(err.args.contains_key("path"));
    }

    #[test]
    fn inspect_garbage_is_parse_failed() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "garbage.osg", b"not an osg file at all");
        let err = inspect(&path).unwrap_err();
        assert_eq!(err.code, ErrorCode::ParseFailed);
        // The CLI prints `PARSE_FAILED: <variant> <details>` from this (spec 006 Behaviour).
        assert!(
            err.details
                .as_deref()
                .unwrap_or("")
                .starts_with("StrideMismatch")
        );
    }

    #[test]
    fn inspect_reports_header_and_deltas() {
        let dump = dump_of(&v1_fixture());
        assert_eq!(dump.header.record_count, 5);
        assert_eq!(dump.header.stride, Some(29));
        assert_eq!(dump.header.score_system, Some("v1"));
        assert_eq!(dump.header.file_size, 8 + 5 * 29);
        assert_eq!(dump.header.diagnostics, 1);
        assert_eq!(dump.diagnostics[0].code, "osg.nonzero_reserved");
        let chord = &dump.records[1];
        assert_eq!(
            (chord.dmax, chord.d300, chord.cmax, chord.c300),
            (1, 1, 2, 1)
        );
        assert!(dump.events[4].score_only);
        assert_eq!(dump.events[1].kinds, vec!["MAX", "300"]);
    }

    #[test]
    fn osg_dump_v1_table() {
        let dump = dump_of(&v1_fixture());
        let out = render_dump(&dump, DumpFormat::Table, DumpView::Records, None).unwrap();
        insta::assert_snapshot!("osg_dump_v1_table", out);
    }

    #[test]
    fn osg_dump_v2_json() {
        let dump = dump_of(&v2_fixture());
        let out = render_dump(&dump, DumpFormat::Json, DumpView::Records, None).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(parsed.get("events").is_none());
        insta::assert_snapshot!("osg_dump_v2_json", out);
    }

    #[test]
    fn osg_dump_events() {
        let dump = dump_of(&v1_fixture());
        let out = render_dump(&dump, DumpFormat::Table, DumpView::Events, None).unwrap();
        insta::assert_snapshot!("osg_dump_events", out);
    }

    #[test]
    fn csv_and_limit() {
        let dump = dump_of(&v2_fixture());
        let out = render_dump(&dump, DumpFormat::Csv, DumpView::Records, Some(2)).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines[0].starts_with("# client_version=20260924"));
        assert!(lines[1].starts_with("idx,t_ms,d300,") && lines[1].ends_with(",b28,f0,f1"));
        assert_eq!(lines.len(), 4, "{out}");
        assert_eq!(
            lines[3],
            "1,750,0,0,0,1,0,0,0,0,0,2,0,0,300,2,2,200,0,0,1,300,0"
        );
        let events = render_dump(&dump, DumpFormat::Json, DumpView::Events, Some(1)).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&events).unwrap();
        assert_eq!(parsed["events"].as_array().unwrap().len(), 1);
        assert!(parsed.get("records").is_none());
    }

    #[test]
    fn empty_file_dumps_without_stride() {
        let dump = dump_of(&OsgFile {
            client_version: CLIENT,
            score_system: None,
            records: vec![],
        });
        assert_eq!((dump.header.stride, dump.header.score_system), (None, None));
        let out = render_dump(&dump, DumpFormat::Table, DumpView::Records, None).unwrap();
        assert!(out.starts_with("client_version=20260924 record_count=0 stride=- score_system=-"));
        assert_eq!(
            render_diagnostics(&dump),
            vec!["osg.empty_graph: 0 records"]
        );
    }

    // ---- survey (T6) ----

    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::time::SystemTime;

    use tokio_util::sync::CancellationToken;
    use wolluf_source_osu::codec::replay_name::{ReplayFileKind, ReplayFileName};
    use wolluf_source_osu::codec::score_header::mods;
    use wolluf_source_osu::snapshot::{SnapshotPolicy, read_stable};
    use wolluf_source_osu::testkit::{
        BeatmapBuilder, FakeInstall, OsrBuilder, OsuDbBuilder, ScoreBuilder,
    };

    use crate::features::plays::testkit::{md5_hex, scores_db, write_file};

    /// Chart A has 3 notes and 1 LN: R3 expects 4 judgements under V1 and 5 under V2.
    const A_CIRCLES: u16 = 3;
    const A_SLIDERS: u16 = 1;

    struct Corpus {
        _dir: tempfile::TempDir,
        root: PathBuf,
    }

    fn materialise(install: &FakeInstall) -> Corpus {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("osu!");
        std::fs::create_dir_all(&root).unwrap();
        for (rel, bytes) in install.files() {
            write_file(&root, &rel, &bytes);
        }
        Corpus { _dir: dir, root }
    }

    fn chart_a() -> String {
        md5_hex(b"survey chart a")
    }

    fn osu_db(md5: &str) -> Vec<u8> {
        OsuDbBuilder::new()
            .beatmap(
                BeatmapBuilder::mania(md5, 7)
                    .object_counts(A_CIRCLES, A_SLIDERS, 0)
                    .build(),
            )
            .encode()
    }

    fn name_of(score: &ScoreBuilder, kind: ReplayFileKind) -> ReplayFileName {
        ReplayFileName {
            kind,
            ..OsrBuilder::new(score.clone()).file_name().unwrap()
        }
    }

    /// One record per judgement in kind order, ending on the header's counts and score, the
    /// shape stable writes for a clean play.
    fn osg_for(c: JudgementCounts, score: i32, v2: bool) -> OsgFile {
        let order = [c.geki, c.n300, c.katu, c.n100, c.n50, c.miss];
        let total: u32 = order.iter().map(|&n| u32::from(n)).sum();
        let mut acc = JudgementCounts::default();
        let mut records = Vec::new();
        let mut i = 0_i32;
        for (slot, &n) in order.iter().enumerate() {
            for _ in 0..n {
                let field = match slot {
                    0 => &mut acc.geki,
                    1 => &mut acc.n300,
                    2 => &mut acc.katu,
                    3 => &mut acc.n100,
                    4 => &mut acc.n50,
                    _ => &mut acc.miss,
                };
                *field += 1;
                i += 1;
                let partial = i64::from(score) * i64::from(i) / i64::from(total);
                let combo = u16::try_from(i).unwrap();
                records.push(rec(
                    i * 100,
                    acc,
                    i32::try_from(partial).unwrap(),
                    combo,
                    v2.then_some([150.0 * f64::from(i), 0.0]),
                ));
            }
        }
        OsgFile {
            client_version: CLIENT,
            score_system: Some(if v2 {
                OsgScoreSystem::V2
            } else {
                OsgScoreSystem::V1
            }),
            records,
        }
    }

    fn play(md5: &str, nth: i64, c: JudgementCounts, score_mods: u32) -> ScoreBuilder {
        ScoreBuilder::mania(md5, "Rosalind", nth)
            .counts(c)
            .score(1_000 * i32::try_from(nth).unwrap())
            .mods(score_mods)
    }

    fn with_pair(install: FakeInstall, score: &ScoreBuilder, osg: &OsgFile) -> FakeInstall {
        install
            .replay(
                &name_of(score, ReplayFileKind::Osr),
                OsrBuilder::new(score.clone()).build(),
            )
            .replay(&name_of(score, ReplayFileKind::Osg), encode_osg(osg))
    }

    fn clean(install: FakeInstall, score: &ScoreBuilder) -> FakeInstall {
        let h = score.header();
        let osg = osg_for(h.counts, h.score, h.is_score_v2());
        with_pair(install, score, &osg)
    }

    fn run(root: &Path) -> OsgSurveyReport {
        survey(root, &SurveyOptions::default(), &CancellationToken::new()).unwrap()
    }

    /// V1 and V2 clean plays, an I5 mismatch (the final record missing), an FC flag in `b25`,
    /// an orphan `.osg`, an `.osr` without `.osg`, and an osu!std play.
    fn mixed_corpus() -> (Corpus, Vec<ScoreBuilder>) {
        let a = chart_a();
        let v1 = play(&a, 1, counts(4, 0, 0, 0), 0);
        let v2 = play(&a, 2, counts(4, 1, 0, 0), mods::SCORE_V2);
        let short = play(&a, 3, counts(3, 1, 0, 0), 0);
        let fc = play(&a, 4, counts(4, 0, 0, 0), 0);
        let orphan = play(&a, 5, counts(4, 0, 0, 0), 0);
        let no_osg = play(&a, 6, counts(4, 0, 0, 0), 0)
            .player(wolluf_source_osu::codec::OsuString::present(*b"Klinsx"));
        let std_play = play(&a, 7, counts(0, 4, 0, 0), 0).mode(0);
        let mut install = FakeInstall::new().osu_db(osu_db(&a)).scores_db(scores_db(&[
            v1.clone(),
            v2.clone(),
            short.clone(),
        ]));
        install = clean(install, &v1);
        install = clean(install, &v2);
        let mut truncated = osg_for(short.header().counts, short.header().score, false);
        truncated.records.pop();
        install = with_pair(install, &short, &truncated);
        let mut flagged = osg_for(fc.header().counts, fc.header().score, false);
        flagged.records.last_mut().unwrap().b25 = 1;
        install = with_pair(install, &fc, &flagged);
        let h = orphan.header();
        install = install.replay(
            &name_of(&orphan, ReplayFileKind::Osg),
            encode_osg(&osg_for(h.counts, h.score, false)),
        );
        install = install.replay(
            &name_of(&no_osg, ReplayFileKind::Osr),
            OsrBuilder::new(no_osg.clone()).build(),
        );
        install = clean(install, &std_play);
        (
            materialise(&install),
            vec![v1, v2, short, fc, orphan, no_osg, std_play],
        )
    }

    fn row<'a>(report: &'a OsgSurveyReport, id: &str) -> &'a InvariantRow {
        report
            .invariants
            .iter()
            .find(|r| r.id == id)
            .unwrap_or_else(|| panic!("no invariant row {id}"))
    }

    fn pfn(r: &InvariantRow) -> (u64, u64, u64) {
        (r.pass, r.fail, r.na)
    }

    #[test]
    fn survey_counts_invariants() {
        let (corpus, plays) = mixed_corpus();
        let report = run(&corpus.root);
        assert_eq!(
            (report.osg_files, report.osr_files, report.orphan_osg),
            (6, 6, 1)
        );
        assert_eq!(pfn(row(&report, "I1")), (6, 0, 0));
        // The orphan has no `.osr` to compare against.
        assert_eq!(pfn(row(&report, "I2")), (5, 0, 1));
        // ADR 0012 item 7: the final-record FC flag passes I3; the `I3_b25` row still records it.
        assert_eq!(pfn(row(&report, "I3")), (6, 0, 0));
        assert!(row(&report, "I3").classes.is_empty());
        assert_eq!(pfn(row(&report, "I3_b25")), (5, 1, 0));
        assert_eq!(row(&report, "I3_b25").classes.get("final_only"), Some(&1));
        assert_eq!(pfn(row(&report, "I3_b28")), (6, 0, 0));
        assert_eq!(pfn(row(&report, "I4")), (6, 0, 0));
        let i5 = row(&report, "I5");
        assert_eq!(pfn(i5), (4, 1, 1));
        let short_osg = name_of(&plays[2], ReplayFileKind::Osg).format();
        assert_eq!(i5.examples, vec![short_osg.clone()]);
        assert_eq!(report.i5_failures.len(), 1);
        assert_eq!(report.i5_failures[0].file, short_osg);
        assert_eq!(report.i5_failures[0].class, "final_short");
        assert!(!report.i5_failures[0].counts_eq);
        // I6: v1, v2 and fc match R3; `short` ends one short; orphan and osu!std are `na`.
        let i6 = row(&report, "I6");
        assert_eq!(pfn(i6), (3, 1, 2));
        assert_eq!(i6.classes.get("short_total"), Some(&1));
        assert_eq!(report.i6_failures[0].expected, 4);
        // I7 only where scores.db has the play: v1 and v2 match, `short` does not.
        assert_eq!(pfn(row(&report, "I7")), (2, 1, 3));
        assert_eq!(report.distributions.modes.get("0"), Some(&1));
        assert_eq!(report.distributions.modes.get("3"), Some(&4));
        assert_eq!(report.distributions.strides.get("45"), Some(&1));
        assert_eq!(report.distributions.client_versions.get(&CLIENT), Some(&6));
        assert_eq!(
            report.distributions.judgements_per_record.get("1"),
            Some(&24)
        );
        let missing = &report.osr_without_osg;
        assert_eq!((missing.total, missing.since_first_osg), (1, 1));
        assert_eq!(missing.by_player.get("Klinsx"), Some(&1));
        assert_eq!(missing.by_month.get("2026-04"), Some(&1));
        assert_eq!(report.sources.osu_db.status, "ok");
        // I5 and I7 have no listed exception class; I6 `short_total` is R4's.
        assert_eq!(report.unexplained_failures(), 2);
    }

    #[test]
    fn survey_i3_fails_on_b25_outside_the_fc_flag() {
        let a = chart_a();
        let early = play(&a, 1, counts(4, 0, 0, 0), 0);
        let odd_value = play(&a, 2, counts(4, 0, 0, 0), 0);
        let mut install = FakeInstall::new().osu_db(osu_db(&a));
        let h = early.header();
        let mut osg = osg_for(h.counts, h.score, false);
        osg.records[0].b25 = 1;
        install = with_pair(install, &early, &osg);
        let h = odd_value.header();
        let mut osg = osg_for(h.counts, h.score, false);
        osg.records.last_mut().unwrap().b25 = 2;
        install = with_pair(install, &odd_value, &osg);
        let corpus = materialise(&install);
        let report = run(&corpus.root);
        let i3 = row(&report, "I3");
        assert_eq!(pfn(i3), (0, 2, 0));
        assert_eq!(i3.classes.get("other"), Some(&2));
        assert_eq!(i3.explained, 0);
        assert_eq!(report.unexplained_failures(), 2);
    }

    #[test]
    fn survey_classifies_v1_split_ln() {
        let a = chart_a();
        let split = play(&a, 1, counts(5, 0, 0, 0), 0);
        let over = play(&a, 2, counts(6, 0, 0, 0), 0);
        let short = play(&a, 3, counts(2, 0, 0, 0), 0);
        let v2_short = play(&a, 4, counts(4, 0, 0, 0), mods::SCORE_V2);
        let mut install = FakeInstall::new().osu_db(osu_db(&a));
        for p in [&split, &over, &short, &v2_short] {
            install = clean(install, p);
        }
        let corpus = materialise(&install);
        let report = run(&corpus.root);
        let i6 = row(&report, "I6");
        assert_eq!(pfn(i6), (0, 4, 0));
        let classes: Vec<(&str, u64)> = i6.classes.iter().map(|(k, v)| (k.as_str(), *v)).collect();
        assert_eq!(
            classes,
            vec![("mismatch", 1), ("short_total", 2), ("v1_split_ln", 1)]
        );
        assert_eq!(i6.explained, 3);
        let by_file: BTreeMap<&str, &str> = report
            .i6_failures
            .iter()
            .map(|f| (f.file.as_str(), f.class))
            .collect();
        let split_name = name_of(&split, ReplayFileKind::Osg).format();
        assert_eq!(by_file.get(split_name.as_str()), Some(&"v1_split_ln"));
        assert_eq!(report.unexplained_failures(), 1);
        assert!(!report.strict_passes());
    }

    #[test]
    fn survey_reports_orphan_osg() {
        let a = chart_a();
        let orphan = play(&a, 1, counts(4, 0, 0, 0), 0);
        let h = orphan.header();
        let install = FakeInstall::new().osu_db(osu_db(&a)).replay(
            &name_of(&orphan, ReplayFileKind::Osg),
            encode_osg(&osg_for(h.counts, h.score, false)),
        );
        let corpus = materialise(&install);
        let report = run(&corpus.root);
        assert_eq!((report.osg_files, report.orphan_osg), (1, 1));
        for id in ["I2", "I5", "I6", "I7"] {
            assert_eq!(pfn(row(&report, id)), (0, 0, 1), "{id}");
        }
        assert_eq!(report.distributions.modes.get("unknown"), Some(&1));
        assert!(report.strict_passes());
    }

    #[test]
    fn survey_continues_after_decode_failure() {
        let a = chart_a();
        let good = play(&a, 1, counts(4, 0, 0, 0), 0);
        let bad = play(&a, 2, counts(4, 0, 0, 0), 0);
        let install = clean(FakeInstall::new().osu_db(osu_db(&a)), &good)
            .replay(
                &name_of(&bad, ReplayFileKind::Osr),
                OsrBuilder::new(bad.clone()).build(),
            )
            .replay(&name_of(&bad, ReplayFileKind::Osg), b"garbage!!".to_vec());
        let corpus = materialise(&install);
        let report = run(&corpus.root);
        assert_eq!(pfn(row(&report, "I1")), (1, 1, 0));
        assert_eq!(row(&report, "I1").classes.get("StrideMismatch"), Some(&1));
        assert_eq!(pfn(row(&report, "I5")), (1, 0, 1));
        assert_eq!(report.decode_failures.len(), 1);
        assert_eq!(report.decode_failures[0].code, "PARSE_FAILED");
        assert_eq!(
            report.decode_failures[0].file,
            name_of(&bad, ReplayFileKind::Osg).format()
        );
        assert!(!report.strict_passes());
    }

    /// Every file under `root` by relative path: size, mtime and sha256.
    fn fingerprint(root: &Path) -> BTreeMap<PathBuf, (u64, SystemTime, [u8; 32])> {
        let mut out = BTreeMap::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    let snap = read_stable(&path, &SnapshotPolicy::default()).unwrap();
                    let rel = path.strip_prefix(root).unwrap().to_path_buf();
                    out.insert(rel, (snap.size, snap.mtime, snap.sha256.0));
                }
            }
        }
        out
    }

    #[test]
    fn survey_does_not_modify_corpus() {
        let (corpus, _) = mixed_corpus();
        let before = fingerprint(&corpus.root);
        assert!(before.len() > 10);
        run(&corpus.root);
        assert_eq!(fingerprint(&corpus.root), before);
    }

    #[test]
    fn survey_output_is_deterministic() {
        let (corpus, _) = mixed_corpus();
        let with_threads = |n: usize| {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(n)
                .build()
                .unwrap();
            serde_json::to_string(&pool.install(|| run(&corpus.root))).unwrap()
        };
        let one = with_threads(1);
        assert_eq!(with_threads(4), one);
        assert_eq!(with_threads(4), one);
        assert_eq!(
            render_survey(&run(&corpus.root)),
            render_survey(&run(&corpus.root))
        );
    }

    #[test]
    fn survey_honours_cancel_and_missing_root() {
        let (corpus, _) = mixed_corpus();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let err = survey(&corpus.root, &SurveyOptions::default(), &cancel).unwrap_err();
        assert_eq!(err.code, ErrorCode::Cancelled);
        let missing = corpus.root.join("nope");
        let err = survey(
            &missing,
            &SurveyOptions::default(),
            &CancellationToken::new(),
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::OsuDirNotFound);
    }

    #[test]
    fn survey_max_files_takes_an_ordered_prefix() {
        let (corpus, _) = mixed_corpus();
        let opts = SurveyOptions {
            max_files: Some(2),
            ..SurveyOptions::default()
        };
        let report = survey(&corpus.root, &opts, &CancellationToken::new()).unwrap();
        assert_eq!(report.osg_files, 2);
        let text = render_survey(&report);
        assert!(text.contains("I1"), "{text}");
    }
}
