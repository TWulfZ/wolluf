#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{Env, FIXTURE_7K_TITLE, osu_7k};
use predicates::str::starts_with;

/// Rows at 0, 0.25, 0.5, 0.75 (LN head), 1 (tap under the LN body), 1.5 (LN tail), 1.75 and
/// 3 s.
fn chart() -> Vec<u8> {
    osu_7k(
        &[
            (0, 0),
            (3, 250),
            (6, 500),
            (5, 1_000),
            (4, 1_750),
            (1, 3_000),
        ],
        &[(2, 750, 1_500)],
    )
}

/// A synced env: `sync` chains `IndexLibrary` and waits for it.
fn synced() -> (Env, String) {
    let env = Env::new();
    let (root, md5) = env.install_with_chart(&chart());
    env.set_install(&root);
    env.json(&["sync"]);
    (env, md5)
}

fn stdout(env: &Env, args: &[&str]) -> String {
    let out = env.wolluf().args(args).output().unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {:?}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn index_prints_the_index_library_job() {
    let (env, _) = synced();
    let job = env.json(&["library", "index"]);
    assert_eq!(job["kind"], "index_library", "{job}");
    assert_eq!(job["status"], "ok", "{job}");
    let c = &job["summary"]["counters"];
    assert_eq!(
        c["parsedNew"], 0,
        "the chained index already parsed it: {job}"
    );
    assert_eq!(c["skippedMemoized"], 1, "{job}");

    let text = stdout(&env, &["library", "index"]);
    assert!(
        text.lines()
            .any(|l| l.starts_with("status") && l.ends_with("ok")),
        "{text}"
    );
    assert!(
        text.lines()
            .any(|l| l.starts_with("skipped memoized") && l.ends_with('1')),
        "{text}"
    );
}

#[test]
fn list_shows_the_synthetic_chart() {
    let (env, md5) = synced();
    let list = env.json(&["library", "list"]);
    let list = list.as_array().unwrap();
    assert_eq!(list.len(), 1, "{list:?}");
    assert_eq!(list[0]["md5"], md5.as_str());
    assert_eq!(list[0]["title"], FIXTURE_7K_TITLE);
    assert_eq!(list[0]["keymode"], 7);
    assert_eq!(list[0]["nLn"], 1);

    assert_eq!(
        env.json(&["library", "list", "--keys", "4"]),
        serde_json::json!([])
    );
    assert_eq!(
        env.json(&["library", "list", "--offset", "1"]),
        serde_json::json!([])
    );
    assert_eq!(
        env.json(&["library", "list", "--text", "no-such-title"]),
        serde_json::json!([])
    );

    let table = stdout(&env, &["library", "list"]);
    let mut lines = table.lines();
    assert!(lines.next().unwrap().starts_with("MD5"), "{table}");
    let row = lines.next().unwrap_or_else(|| panic!("{table}"));
    assert!(row.starts_with(&md5), "{table}");
    assert!(row.contains(FIXTURE_7K_TITLE), "{table}");
}

#[test]
fn scales_is_an_array() {
    let (env, _) = synced();
    assert!(env.json(&["library", "scales"]).is_array());
    assert!(stdout(&env, &["library", "scales"]).starts_with("SCALE"));
}

#[test]
fn chart_show_prints_the_window_and_key_row() {
    let (env, md5) = synced();
    let out = stdout(&env, &["chart", "show", &md5, "--from", "0", "--to", "2"]);
    // Rows only at events, earliest at the bottom; the 3 s tap is past `--to`.
    assert_eq!(
        out,
        "00:01.750  ...|.+o..\n\
         00:01.500  ..T|.+...\n\
         00:01.000  ..:|.+.o.\n\
         00:00.750  ..H|.+...\n\
         00:00.500  ...|.+..o\n\
         00:00.250  ...|o+...\n\
         00:00.000  o..|.+...\n\
         \x20          rmi|t+imr  k7.313_right_thumb\n"
    );

    let mmss = stdout(
        &env,
        &["chart", "show", &md5, "--from", "0:00", "--to", "0:02"],
    );
    assert_eq!(mmss, out);

    let left = stdout(
        &env,
        &[
            "chart",
            "show",
            &md5,
            "--to",
            "2",
            "--layout",
            "k7.313_left_thumb",
        ],
    );
    assert!(
        left.lines().last().unwrap().ends_with("k7.313_left_thumb"),
        "{left}"
    );
}

#[test]
fn chart_info_prints_the_detail() {
    let (env, md5) = synced();
    let detail = env.json(&["chart", "info", &md5]);
    assert_eq!(detail["chart"]["md5"], md5.as_str(), "{detail}");
    assert_eq!(detail["diagnostics"], 0, "{detail}");
    let text = stdout(&env, &["chart", "info", &md5]);
    assert!(
        text.lines()
            .any(|l| l.starts_with("title") && l.ends_with(FIXTURE_7K_TITLE)),
        "{text}"
    );
}

#[test]
fn bad_input_exits_2_invalid_input() {
    let (env, md5) = synced();
    for args in [
        vec!["chart", "show", "not-an-md5"],
        vec!["chart", "info", "not-an-md5"],
        vec!["chart", "show", &md5, "--layout", "k7.no_such_layout"],
        vec!["chart", "show", &md5, "--from", "5", "--to", "5"],
    ] {
        env.wolluf()
            .args(&args)
            .assert()
            .code(2)
            .stderr(starts_with("error[INVALID_INPUT]: "));
    }
    // A malformed time never reaches the app: clap rejects it.
    env.wolluf()
        .args(["chart", "show", &md5, "--from", "1:75"])
        .assert()
        .code(2);
}

#[test]
fn unknown_chart_exits_2_not_found() {
    let (env, _) = synced();
    env.wolluf()
        .args(["chart", "show", &"0".repeat(32)])
        .assert()
        .code(2)
        .stderr(starts_with("error[NOT_FOUND]: "));
}

/// Six presses in column 0 from 1 s, 100 ms apart: one `regular.jack.longjack` segment.
fn jacks_synced() -> (Env, String) {
    let env = Env::new();
    let taps: Vec<(u8, i32)> = (0..6).map(|i| (0, 1_000 + i * 100)).collect();
    let (root, md5) = env.install_with_chart(&osu_7k(&taps, &[]));
    env.set_install(&root);
    env.json(&["sync"]);
    (env, md5)
}

#[test]
fn index_reports_segments_written() {
    let (env, _) = jacks_synced();
    let jobs = env.json(&["jobs", "list"]);
    let index = jobs
        .as_array()
        .unwrap()
        .iter()
        .find(|j| j["kind"] == "index_library")
        .unwrap_or_else(|| panic!("{jobs}"));
    assert_eq!(
        index["summary"]["counters"]["segmentsWritten"], 1,
        "{index}"
    );
    let text = stdout(&env, &["library", "index"]);
    assert!(
        text.lines()
            .any(|l| l.starts_with("segments written") && l.ends_with('0')),
        "{text}"
    );
}

#[test]
fn chart_show_segments_marks_rows_and_prints_a_legend() {
    let (env, md5) = jacks_synced();
    let out = stdout(&env, &["chart", "show", &md5, "--to", "3", "--segments"]);
    assert!(
        out.lines()
            .any(|l| l.starts_with("00:01.000") && l.ends_with("* lj")),
        "{out}"
    );
    assert!(
        out.lines()
            .any(|l| l.starts_with("00:01.300") && l.ends_with("| lj")),
        "{out}"
    );
    assert!(
        out.ends_with("segments (k7.313_right_thumb, * first row):\n  lj  regular.jack.longjack\n"),
        "{out}"
    );
    let plain = stdout(&env, &["chart", "show", &md5, "--to", "3"]);
    assert!(!plain.contains("lj"), "{plain}");
    let left = stdout(
        &env,
        &[
            "chart",
            "show",
            &md5,
            "--to",
            "3",
            "--segments",
            "--layout",
            "k7.313_left_thumb",
        ],
    );
    assert!(
        left.contains("segments (k7.313_right_thumb, * first row):"),
        "{left}"
    );
    let none = stdout(
        &env,
        &[
            "chart",
            "show",
            &md5,
            "--from",
            "2",
            "--to",
            "3",
            "--segments",
        ],
    );
    assert!(none.ends_with("segments: none\n"), "{none}");
}

#[test]
fn chart_info_lists_segments() {
    let (env, md5) = jacks_synced();
    let detail = env.json(&["chart", "info", &md5]);
    let segments = detail["segments"].as_array().unwrap();
    assert_eq!(segments.len(), 1, "{detail}");
    assert_eq!(segments[0]["patternId"], "regular.jack.longjack");
    assert_eq!(segments[0]["key"], "lj");
    assert_eq!(
        (segments[0]["t0Ms"].as_i64(), segments[0]["t1Ms"].as_i64()),
        (Some(1_000), Some(1_500))
    );
    let text = stdout(&env, &["chart", "info", &md5]);
    assert!(
        text.lines().any(|l| l.starts_with("segment")
            && l.contains("00:01.000-00:01.500")
            && l.contains("lj regular.jack.longjack")),
        "{text}"
    );
}

#[test]
fn library_patterns_counts_primary_segments() {
    let (env, _) = jacks_synced();
    let counts = env.json(&["library", "patterns"]);
    assert_eq!(counts.as_array().unwrap().len(), 1, "{counts}");
    assert_eq!(counts[0]["patternId"], "regular.jack.longjack");
    assert_eq!(
        (counts[0]["segments"].as_u64(), counts[0]["charts"].as_u64()),
        (Some(1), Some(1))
    );
    let table = stdout(&env, &["library", "patterns"]);
    let mut lines = table.lines();
    assert!(lines.next().unwrap().starts_with("KEYS"), "{table}");
    let row = lines.next().unwrap_or_else(|| panic!("{table}"));
    assert!(
        row.contains("lj") && row.contains("regular.jack.longjack"),
        "{table}"
    );
}
