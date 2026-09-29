#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! `wolluf osg dump|survey` (spec 006 AC6). The CLI may not depend on `wolluf-source-osu`
//! (layers.toml), so its testkit encoders are unavailable here: the synthetic `.osg` and `.osr`
//! bytes are written by the small encoders below, from the layout in spec 006 H1 and 002's
//! score header.

mod common;

use std::path::{Path, PathBuf};

use common::Env;
use predicates::str::{contains, starts_with};

const CLIENT: i32 = 20_260_924;
const MANIA: u8 = 3;
const SCORE_V2: u32 = 1 << 29;
/// FILETIME = .NET ticks − this (spec 006 R1).
const DOTNET_TO_FILETIME_TICKS: i64 = 504_911_232_000_000_000;
const BASE_TICKS: i64 = 639_140_000_000_000_000;
const OSU_STRING_PRESENT: u8 = 0x0b;
const FULL_HP: u16 = 200;
const MD5_V1: &str = "0123456789abcdef0123456789abcdef";
const MD5_V2: &str = "fedcba9876543210fedcba9876543210";

/// Counts in `.osr`/`.osg` order: 300, 100, 50, MAX, 200, miss (spec 006 R2).
type Counts = [u16; 6];

struct Rec {
    t_ms: i32,
    counts: Counts,
    score: i32,
    combo: u16,
    b25: u8,
}

fn osg_bytes(records: &[Rec], v2: bool) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(CLIENT.to_le_bytes());
    out.extend(i32::try_from(records.len()).unwrap().to_le_bytes());
    for r in records {
        out.extend(r.t_ms.to_le_bytes());
        out.push(0);
        for n in r.counts {
            out.extend(n.to_le_bytes());
        }
        out.extend(r.score.to_le_bytes());
        out.extend(r.combo.to_le_bytes());
        out.extend(r.combo.to_le_bytes());
        out.push(r.b25);
        out.extend(FULL_HP.to_le_bytes());
        out.push(u8::from(v2));
        if v2 {
            out.extend((150.0_f64 * f64::from(r.combo)).to_le_bytes());
            out.extend(0.0_f64.to_le_bytes());
        }
    }
    out
}

fn osu_string(out: &mut Vec<u8>, s: &str) {
    out.push(OSU_STRING_PRESENT);
    let len = u8::try_from(s.len()).unwrap();
    assert!(len < 0x80, "single-byte ULEB128 only");
    out.push(len);
    out.extend(s.as_bytes());
}

/// Only the score header: the survey never reads past it.
fn osr_header(md5: &str, ticks: i64, counts: Counts, score: i32, mods: u32) -> Vec<u8> {
    let mut out = vec![MANIA];
    out.extend(CLIENT.to_le_bytes());
    osu_string(&mut out, md5);
    osu_string(&mut out, "player-01");
    osu_string(&mut out, "00000000000000000000000000000000");
    for n in counts {
        out.extend(n.to_le_bytes());
    }
    out.extend(score.to_le_bytes());
    out.extend(counts.iter().sum::<u16>().to_le_bytes());
    out.push(1);
    out.extend(mods.to_le_bytes());
    osu_string(&mut out, "");
    out.extend(ticks.to_le_bytes());
    out
}

/// One MAX per record, ending on `total` judgements and `score`, as stable writes a clean FC.
fn clean_records(total: u16, score: i32) -> Vec<Rec> {
    (1..=total)
        .map(|i| Rec {
            t_ms: i32::from(i) * 100,
            counts: [0, 0, 0, i, 0, 0],
            score: score * i32::from(i) / i32::from(total),
            combo: i,
            b25: 0,
        })
        .collect()
}

fn write(path: &Path, bytes: &[u8]) -> PathBuf {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
    path.to_path_buf()
}

struct Corpus {
    root: PathBuf,
}

impl Corpus {
    fn new(env: &Env) -> Self {
        Self {
            root: env.dir.path().join("osu!"),
        }
    }

    fn pair(&self, md5: &str, nth: i64, records: &[Rec], header: (Counts, i32), mods: u32) {
        let ticks = BASE_TICKS + nth;
        let stem = format!("{md5}-{}", ticks - DOTNET_TO_FILETIME_TICKS);
        let dir = self.root.join("Data").join("r");
        write(
            &dir.join(format!("{stem}.osr")),
            &osr_header(md5, ticks, header.0, header.1, mods),
        );
        let v2 = mods & SCORE_V2 != 0;
        write(&dir.join(format!("{stem}.osg")), &osg_bytes(records, v2));
    }

    fn clean(&self, md5: &str, nth: i64, total: u16, score: i32, mods: u32) {
        let records = clean_records(total, score);
        self.pair(md5, nth, &records, ([0, 0, 0, total, 0, 0], score), mods);
    }
}

fn v1_file(env: &Env) -> PathBuf {
    write(
        &env.dir.path().join("one.osg"),
        &osg_bytes(&clean_records(3, 900), false),
    )
}

#[test]
fn dump_table_exits_0() {
    let env = Env::new();
    let file = v1_file(&env);
    let out = env
        .wolluf()
        .args(["osg", "dump"])
        .arg(&file)
        .assert()
        .code(0);
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let mut lines = stdout.lines();
    assert_eq!(
        lines.next(),
        Some(
            "client_version=20260924 record_count=3 stride=29 score_system=v1 file_size=95 \
             diagnostics=0"
        )
    );
    assert!(lines.next().unwrap().starts_with("idx t_ms d300"));
    assert_eq!(stdout.lines().count(), 2 + 3, "{stdout}");
    // The spike tools read one file; they never open, lock or create the data dir.
    assert!(!env.data_dir().exists());
}

#[test]
fn dump_events_csv_and_limit() {
    let env = Env::new();
    let file = v1_file(&env);
    let out = env
        .wolluf()
        .args(["osg", "dump", "--events", "--format", "csv", "--limit", "2"])
        .arg(&file)
        .assert()
        .code(0);
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[1], "idx,t_ms,kinds,n,combo_delta,score_delta");
    assert_eq!(lines.len(), 2 + 2, "{stdout}");
    assert!(lines[2].starts_with("0,100,MAX,1,"), "{stdout}");
}

#[test]
fn dump_json_is_valid() {
    let env = Env::new();
    let file = v1_file(&env);
    for args in [
        vec!["osg", "dump", "--format", "json"],
        vec!["--json", "osg", "dump"],
    ] {
        let out = env.wolluf().args(&args).arg(&file).output().unwrap();
        assert_eq!(out.status.code(), Some(0), "{args:?}");
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["header"]["record_count"], 3);
        assert_eq!(v["header"]["stride"], 29);
        assert_eq!(v["records"].as_array().unwrap().len(), 3);
        assert_eq!(v["records"][2]["cmax"], 3);
        assert!(v.get("events").is_none());
    }
    let events = env.json(&["osg", "dump", "--events", file.to_str().unwrap()]);
    assert_eq!(events["events"][0]["kinds"], serde_json::json!(["MAX"]));
}

#[test]
fn dump_warnings_go_to_stderr_and_exit_0() {
    let env = Env::new();
    let mut records = clean_records(2, 600);
    records[0].b25 = 1;
    let file = write(
        &env.dir.path().join("reserved.osg"),
        &osg_bytes(&records, false),
    );
    env.wolluf()
        .args(["osg", "dump"])
        .arg(&file)
        .assert()
        .code(0)
        .stdout(contains("diagnostics=1"))
        .stderr(starts_with("warning: osg.nonzero_reserved"));
}

#[test]
fn dump_final_record_fc_flag_is_silent() {
    let env = Env::new();
    let mut records = clean_records(2, 600);
    records[1].b25 = 1;
    let file = write(&env.dir.path().join("fc.osg"), &osg_bytes(&records, false));
    env.wolluf()
        .args(["osg", "dump"])
        .arg(&file)
        .assert()
        .code(0)
        .stdout(contains("diagnostics=0"))
        .stderr("");
}

#[test]
fn survey_strict_accepts_final_record_fc_flag() {
    let env = Env::new();
    let corpus = Corpus::new(&env);
    let mut records = clean_records(4, 1_000_000);
    records[3].b25 = 1;
    corpus.pair(MD5_V1, 1, &records, ([0, 0, 0, 4, 0, 0], 1_000_000), 0);
    env.wolluf()
        .args(["osg", "survey", "--strict", "--corpus"])
        .arg(&corpus.root)
        .assert()
        .code(0);
}

#[test]
fn dump_missing_file_exits_2_not_found() {
    let env = Env::new();
    let missing = env.dir.path().join("absent.osg");
    env.wolluf()
        .args(["osg", "dump"])
        .arg(&missing)
        .assert()
        .code(2)
        .stdout("")
        .stderr(format!("NOT_FOUND: {}\n", missing.display()));
}

#[test]
fn dump_garbage_exits_2_parse_failed() {
    let env = Env::new();
    let file = write(
        &env.dir.path().join("garbage.osg"),
        b"not an osg file at all",
    );
    env.wolluf()
        .args(["osg", "dump"])
        .arg(&file)
        .assert()
        .code(2)
        .stdout("")
        .stderr(starts_with("PARSE_FAILED: StrideMismatch {"));
}

#[test]
fn survey_synthetic_corpus_strict_passes() {
    let env = Env::new();
    let corpus = Corpus::new(&env);
    corpus.clean(MD5_V1, 1, 4, 1_000_000, 0);
    corpus.clean(MD5_V2, 2, 5, 1_000_000, SCORE_V2);

    let report = env.json(&[
        "osg",
        "survey",
        "--strict",
        "--corpus",
        corpus.root.to_str().unwrap(),
    ]);
    assert_eq!(
        (report["osg_files"].as_u64(), report["osr_files"].as_u64()),
        (Some(2), Some(2))
    );
    let row = |id: &str| {
        report["invariants"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == id)
            .unwrap_or_else(|| panic!("{id}"))
            .clone()
    };
    for id in ["I1", "I2", "I3", "I4", "I5"] {
        assert_eq!(
            (row(id)["pass"].as_u64(), row(id)["fail"].as_u64()),
            (Some(2), Some(0)),
            "{id}"
        );
    }
    // Neither DB exists in this corpus, so the R3 total and scores.db checks are `na`.
    assert_eq!(row("I6")["na"], 2);
    assert_eq!(report["sources"]["osu_db"]["status"], "missing");

    env.wolluf()
        .args(["osg", "survey", "--strict", "--corpus"])
        .arg(&corpus.root)
        .assert()
        .code(0)
        .stdout(starts_with("osg 2  osr 2  orphan_osg 0"));
}

#[test]
fn survey_strict_fails_on_unexplained_failure() {
    let env = Env::new();
    let corpus = Corpus::new(&env);
    corpus.clean(MD5_V1, 1, 4, 1_000_000, 0);
    // The final record is missing, so the last counts fall short of the `.osr` header (I5).
    let mut records = clean_records(4, 1_000_000);
    records.pop();
    corpus.pair(MD5_V2, 2, &records, ([0, 0, 0, 4, 0, 0], 1_000_000), 0);

    env.wolluf()
        .args(["osg", "survey", "--corpus"])
        .arg(&corpus.root)
        .assert()
        .code(0);
    env.wolluf()
        .args(["osg", "survey", "--strict", "--corpus"])
        .arg(&corpus.root)
        .assert()
        .code(1)
        .stderr(contains("strict: 1 unexplained failure"));
}

#[test]
fn survey_max_files_limits_the_prefix() {
    let env = Env::new();
    let corpus = Corpus::new(&env);
    corpus.clean(MD5_V1, 1, 4, 1_000_000, 0);
    corpus.clean(MD5_V2, 2, 5, 1_000_000, SCORE_V2);
    let report = env.json(&[
        "osg",
        "survey",
        "--max-files",
        "1",
        "--corpus",
        corpus.root.to_str().unwrap(),
    ]);
    assert_eq!(report["osg_files"], 1);
}

#[test]
fn survey_missing_root_exits_2() {
    let env = Env::new();
    env.wolluf()
        .args(["osg", "survey", "--corpus"])
        .arg(env.dir.path().join("nope"))
        .assert()
        .code(2)
        .stderr(starts_with("error[OSU_DIR_NOT_FOUND]: "));
}
