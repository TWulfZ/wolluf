#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{Env, FIXTURE_7K_TITLE, osu_7k};

const KEY_ROW: &str = "rmi|t+imr  k7.313_right_thumb";

/// A tap every 125 ms over 20 s, walking the columns.
fn chart() -> Vec<u8> {
    let taps: Vec<(u8, i32)> = (0..160).map(|i| ((i % 7) as u8, i * 125)).collect();
    osu_7k(&taps, &[])
}

fn synced() -> (Env, String) {
    let env = Env::new();
    let (root, md5) = env.install_with_chart(&chart());
    env.set_install(&root);
    env.json(&["sync"]);
    (env, md5)
}

fn session(env: &Env, script: &str) -> String {
    let out = env
        .wolluf()
        .args(["label", "--seed", "1"])
        .write_stdin(script)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{:?}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// Lines answering a prompt with `prefix`; the prompt has no newline, so it leads the line.
fn replies(out: &str, prefix: &str) -> usize {
    out.lines()
        .filter(|l| l.trim_start_matches("> ").starts_with(prefix))
        .count()
}

#[test]
fn scripted_session_labels_undoes_and_exports() {
    let (env, md5) = synced();
    // Round 1: jumpstream. Round 2: a typo, flags, then two patterns by key and by id.
    // Round 3: undo round 2, then quit.
    let script = "js\nzz\nm\n?\nt regular.jack.chordjack\nu\nq\n";
    let out = session(&env, script);

    assert!(out.contains("seed 1"), "{out}");
    assert!(out.contains(FIXTURE_7K_TITLE), "{out}");
    assert_eq!(out.matches(KEY_ROW).count(), 3, "three rounds drawn: {out}");
    assert!(out.contains("7k.regular.stream"), "legend by axis: {out}");
    assert!(out.contains("js jumpstream"), "{out}");
    assert!(out.contains("error: unknown pattern `zz`"), "{out}");
    assert!(out.contains("flags: mixed unsure"), "{out}");
    assert_eq!(replies(&out, "saved "), 2, "{out}");
    assert_eq!(replies(&out, "undone "), 1, "{out}");
    assert!(out.contains("labelled 2, skipped 0, undone 1"), "{out}");

    let stats = env.json(&["label", "stats"]);
    assert_eq!(stats["total"], 1, "{stats}");
    assert_eq!(stats["mixed"], 0, "{stats}");
    assert_eq!(
        stats["perPattern"],
        serde_json::json!([{"key": "regular.stream.jumpstream", "count": 1}])
    );
    let text = env.wolluf().args(["label", "stats"]).output().unwrap();
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(
        text.lines()
            .any(|l| l.starts_with("total") && l.ends_with('1')),
        "{text}"
    );

    let dir = tempfile::tempdir().unwrap();
    let out = env
        .wolluf()
        .current_dir(dir.path())
        .args(["label", "export"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let file = dir.path().join("fixtures/labels/gold-7k.jsonl");
    let content = std::fs::read_to_string(&file).unwrap();
    let lines: Vec<serde_json::Value> = content
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 1, "{content}");
    let row = lines[0].as_object().unwrap();
    let keys: Vec<&str> = row.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "cols",
            "flags",
            "labelled_at",
            "md5",
            "no_pattern",
            "patterns",
            "t0_us",
            "t1_us",
            "thumb_pref"
        ]
    );
    assert_eq!(row["md5"], md5.as_str());
    let (t0, t1) = (
        row["t0_us"].as_i64().unwrap(),
        row["t1_us"].as_i64().unwrap(),
    );
    assert_eq!(t1 - t0, 4_000_000);
    assert_eq!(t0 % 125_000, 0, "starts on a row");
    assert_eq!(row["cols"], serde_json::json!([1, 2, 3, 4, 5, 6, 7]));
    assert_eq!(
        row["patterns"],
        serde_json::json!(["regular.stream.jumpstream"])
    );
    assert_eq!(row["flags"], serde_json::json!([]));
    assert_eq!(row["no_pattern"], false);
    assert_eq!(row["thumb_pref"], serde_json::Value::Null);
    assert!(row["labelled_at"].as_str().unwrap().ends_with('Z'));
    assert!(!content.contains(FIXTURE_7K_TITLE), "{content}");

    let custom = dir.path().join("x/y.jsonl");
    let done = env.json(&["label", "export", "--out", custom.to_str().unwrap()]);
    assert_eq!(done["rows"], 1, "{done}");
    assert_eq!(std::fs::read_to_string(&custom).unwrap(), content);
}

#[test]
fn window_commands_redraw_and_eof_quits() {
    let (env, _) = synced();
    let out = session(&env, "w+\nn\np\nw-\nw-\nw-\nw-\nw-\nh\ns\n");
    // Round 1: initial draw, w+, n, p, and four w- that shrink 5 s to 1 s; the fifth is
    // refused. Round 2 is drawn, then EOF quits.
    assert_eq!(out.matches(KEY_ROW).count(), 9, "{out}");
    assert!(out.contains("(5.0 s)"), "{out}");
    assert!(out.contains("(2.0 s)"), "{out}");
    assert!(
        out.contains("error: the window cannot be shorter than 1 s"),
        "{out}"
    );
    assert!(out.contains("w+/w-"), "help: {out}");
    assert!(
        out.contains("labelled 0, skipped 1, undone 0"),
        "EOF ends the session: {out}"
    );
    assert_eq!(env.json(&["label", "stats"])["total"], 0);
}

#[test]
fn nothing_to_label_ends_the_session() {
    let env = Env::new();
    let root = env.install();
    env.set_install(&root);
    env.json(&["sync"]);
    let out = session(&env, "");
    assert!(out.contains("no window left to label"), "{out}");
}

#[test]
fn no_pattern_and_thumb_side_are_stored_not_skipped() {
    let (env, _) = synced();
    // Round 1: "no clear pattern", right thumb set alone. Round 2: skip. Round 3: a pattern
    // with the left thumb inline.
    let out = session(&env, "tr\nx\ns\njs tl\nq\n");
    assert!(out.contains("flags: thumb:right"), "{out}");
    assert!(out.contains("x no clear pattern"), "legend: {out}");
    assert!(out.contains("labelled 2, skipped 1, undone 0"), "{out}");
    let stats = env.json(&["label", "stats"]);
    assert_eq!(
        (&stats["total"], &stats["noPattern"]),
        (&serde_json::json!(2), &serde_json::json!(1)),
        "{stats}"
    );
    assert_eq!(
        (&stats["thumbLeft"], &stats["thumbRight"]),
        (&serde_json::json!(1), &serde_json::json!(1)),
        "{stats}"
    );
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("g.jsonl");
    env.json(&["label", "export", "--out", file.to_str().unwrap()]);
    let rows: Vec<serde_json::Value> = std::fs::read_to_string(&file)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let none = rows.iter().find(|r| r["no_pattern"] == true).unwrap();
    assert_eq!(none["patterns"], serde_json::json!([]));
    assert_eq!(none["thumb_pref"], "right");
    let js = rows.iter().find(|r| r["no_pattern"] == false).unwrap();
    assert_eq!(
        js["patterns"],
        serde_json::json!(["regular.stream.jumpstream"])
    );
    assert_eq!(js["thumb_pref"], "left");
}
