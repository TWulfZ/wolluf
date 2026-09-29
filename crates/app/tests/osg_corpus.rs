#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Spec 006 AC8: the `.osg` structural invariants over the pilot corpus, with every failure
//! printed by class. The survey only reads the corpus (AC7), which the tree check re-proves.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::str::FromStr;

use tokio_util::sync::CancellationToken;
use wolluf_app::features::plays::osg::{
    InvariantRow, OsgSurveyReport, SurveyOptions, render_survey, survey,
};
use wolluf_core::{ChartMd5, DotNetTicks, FileTime};
use wolluf_source_osu::codec::osr::decode_osr;
use wolluf_source_osu::codec::replay_name::{ReplayFileKind, ReplayFileName};
use wolluf_source_osu::codec::scores_db::decode_scores_db;

const SCORES_DB: &str = "scores.db";
const REPLAY_DIR: &str = "Data/r";
/// Spec 006 AC8: I5 may miss on at most 0.5% of the files it applies to.
const I5_MIN_PASS_SHARE: f64 = 0.995;
const CLASS_FINAL_ONLY: &str = "final_only";

type ReplayKey = (ChartMd5, FileTime);

fn row<'a>(report: &'a OsgSurveyReport, id: &str) -> &'a InvariantRow {
    report
        .invariants
        .iter()
        .find(|r| r.id == id)
        .unwrap_or_else(|| panic!("no invariant row {id}"))
}

fn assert_all_pass(report: &OsgSurveyReport, id: &str) {
    let r = row(report, id);
    assert_eq!(
        (r.pass, r.fail, r.na),
        (report.osg_files, 0, 0),
        "{id} must hold on every .osg: {r:?}"
    );
}

fn replay_keys(root: &Path, kind: ReplayFileKind) -> BTreeSet<ReplayKey> {
    fs::read_dir(root.join(REPLAY_DIR))
        .unwrap()
        .filter_map(|e| {
            let name = ReplayFileName::parse(e.unwrap().file_name().to_str()?)?;
            (name.kind == kind).then_some((name.md5, name.filetime))
        })
        .collect()
}

/// The `.osr` "perfect" (full combo) byte of every play that has an `.osg`: scores.db where the
/// play has a row, else the `.osr` header itself (replay-only plays).
fn full_combo_by_key(root: &Path, osg_keys: &BTreeSet<ReplayKey>) -> BTreeMap<ReplayKey, bool> {
    let (db, _) = decode_scores_db(&fs::read(root.join(SCORES_DB)).unwrap()).unwrap();
    let from_db: BTreeMap<ReplayKey, bool> = db
        .scores()
        .map(|s| {
            let md5 = std::str::from_utf8(s.header.beatmap_md5.as_bytes().unwrap()).unwrap();
            let key = (
                ChartMd5::from_str(md5).unwrap(),
                DotNetTicks(s.header.timestamp_ticks).to_filetime().unwrap(),
            );
            (key, s.header.perfect != 0)
        })
        .collect();
    osg_keys
        .iter()
        .map(|key| {
            let fc = from_db.get(key).copied().unwrap_or_else(|| {
                let name = ReplayFileName {
                    md5: key.0,
                    filetime: key.1,
                    kind: ReplayFileKind::Osr,
                };
                let bytes = fs::read(root.join(REPLAY_DIR).join(name.format())).unwrap();
                decode_osr(&bytes).unwrap().0.header.perfect != 0
            });
            (*key, fc)
        })
        .collect()
}

#[test]
#[ignore = "corpus: needs WOLLUF_CORPUS"]
fn osg_corpus_invariants() {
    let root = common::corpus();
    let tree_before = common::tree_state(&root);
    let opts = SurveyOptions {
        // Every failing file is listed, so the b25 cross-check below sees all of them.
        examples: usize::MAX,
        ..SurveyOptions::default()
    };
    let report = survey(&root, &opts, &CancellationToken::new()).unwrap();
    println!("{}", render_survey(&report));
    for f in &report.i5_failures {
        println!("I5 {}: {f:?}", f.class);
    }
    for f in &report.i6_failures {
        println!("I6 {}: {f:?}", f.class);
    }

    assert!(
        report.osg_files > 0,
        "no .osg files under {}",
        root.display()
    );
    assert!(
        report.decode_failures.is_empty(),
        "{:?}",
        report.decode_failures
    );
    // AC8 as reworded by ADR 0012 item 7: I3 holds on every file once the final-record `b25`
    // FC flag is not counted as reserved.
    for id in ["I1", "I2", "I3", "I3_b28", "I3_b4", "I4"] {
        assert_all_pass(&report, id);
    }
    assert_eq!(
        report.distributions.diagnostics.get("osg.nonzero_reserved"),
        None,
        "{:?}",
        report.distributions.diagnostics
    );

    // `b25` is set on the last record only and only on full-combo plays (the `.osr` perfect
    // byte); the pilot has one full combo without it (36 of 37), so the flag is asserted one way,
    // never as zero.
    let b25 = row(&report, "I3_b25");
    assert_eq!(
        b25.classes.keys().collect::<Vec<_>>(),
        if b25.fail == 0 {
            vec![]
        } else {
            vec![CLASS_FINAL_ONLY]
        },
        "b25 set anywhere but the last record: {b25:?}"
    );
    let b25_files: BTreeSet<ReplayKey> = b25
        .examples
        .iter()
        .map(|f| {
            let n = ReplayFileName::parse(f).unwrap();
            (n.md5, n.filetime)
        })
        .collect();
    assert_eq!(b25_files.len() as u64, b25.fail);
    let osg_keys = replay_keys(&root, ReplayFileKind::Osg);
    let fc = full_combo_by_key(&root, &osg_keys);
    let full_combos: BTreeSet<ReplayKey> =
        fc.iter().filter(|(_, v)| **v).map(|(k, _)| *k).collect();
    println!(
        "b25 on the final record: {} files; full-combo plays with an .osg: {}; both: {}",
        b25_files.len(),
        full_combos.len(),
        b25_files.intersection(&full_combos).count()
    );
    println!(
        "full combos without b25: {:?}",
        full_combos.difference(&b25_files).collect::<Vec<_>>()
    );
    let not_full_combo: Vec<&ReplayKey> = b25_files.difference(&full_combos).collect();
    assert!(
        not_full_combo.is_empty(),
        "final-record b25 on plays that are not full combos: {not_full_combo:?}"
    );

    let i5 = row(&report, "I5");
    let applicable = i5.pass + i5.fail;
    assert!(applicable > 0);
    let share = i5.pass as f64 / applicable as f64;
    assert!(
        share >= I5_MIN_PASS_SHARE,
        "I5 holds on {}/{applicable} = {share:.4}",
        i5.pass
    );
    let i6 = row(&report, "I6");
    assert_eq!(
        report.i5_failures.len() as u64,
        i5.fail,
        "every I5 failure listed"
    );
    assert_eq!(
        report.i6_failures.len() as u64,
        i6.fail,
        "every I6 failure listed"
    );
    assert!(
        report
            .i5_failures
            .iter()
            .map(|f| f.class)
            .chain(report.i6_failures.iter().map(|f| f.class))
            .all(|c| !c.is_empty())
    );

    common::assert_unchanged("the survey", &tree_before, &common::tree_state(&root));
}
