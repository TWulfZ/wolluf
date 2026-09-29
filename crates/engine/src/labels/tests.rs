use super::*;

// Ids are spelled out as literals on purpose: they are persisted, so a rename must break these tests.
fn lab(
    source: &str,
    scale: &str,
    level_ord: Option<f64>,
    level_text: &str,
    skill: Option<&str>,
    is_variant: bool,
) -> ChartLabel {
    ChartLabel {
        source: source.into(),
        scale: scale.into(),
        level_ord,
        level_text: level_text.into(),
        skill_tag: skill.map(Into::into),
        is_variant,
    }
}

fn run(folder: &str, version: &str, creator: &str, set_id: Option<i32>) -> Vec<ChartLabel> {
    extract_labels(&LabelInput {
        folder,
        version,
        creator,
        set_id,
    })
}

struct Case {
    name: &'static str,
    folder: &'static str,
    version: &'static str,
    creator: &'static str,
    set_id: Option<i32>,
    expected: Vec<ChartLabel>,
}

fn check(cases: Vec<Case>) {
    for c in cases {
        assert_eq!(
            run(c.folder, c.version, c.creator, c.set_id),
            c.expected,
            "case {}",
            c.name
        );
    }
}

const REGULAR: &str = "1340245 Jinjin - 7K Dan Course - Regular Dan Phase 1";
const LN: &str = "1467891 Jinjin - 7K Dan Course - LN Dan Phase 2";
const KD_JACK: &str = "1877617 Various Artists - KomeijiDove 7K Jack Practice";
const BMS_FOLDER: &str = "[_BMS_] Artist - Title";
const RTG: &str = "999 Various - 7K Road to Gamma Dan Pack";

fn jinjin_regular(ord: Option<f64>, text: &str) -> Vec<ChartLabel> {
    vec![lab(
        "jinjin_dan_regular",
        "jinjin_dan_regular",
        ord,
        text,
        Some("regular_overall"),
        false,
    )]
}

#[test]
fn dan_ordinals() {
    let cases = [
        ("0th", Some(0.0), "0th"),
        ("1st Dan", Some(1.0), "1st"),
        ("2nd", Some(2.0), "2nd"),
        ("3rd", Some(3.0), "3rd"),
        ("  10TH Dan  ", Some(10.0), "10th"),
        ("Gamma", Some(11.0), "gamma"),
        ("Azimuth Dan", Some(12.0), "azimuth"),
        ("Zenith", Some(13.0), "zenith"),
        ("stellium", Some(14.0), "stellium"),
        // `re.match` anchors at the start; a suffix-less number is no ordinal.
        ("Dan 9th", None, ""),
        ("9 Dan", None, ""),
        ("Alpha", None, ""),
    ];
    for (version, ord, text) in cases {
        assert_eq!(
            run(REGULAR, version, "Jinjin", None),
            jinjin_regular(ord, text),
            "version {version:?}"
        );
    }
}

#[test]
fn jinjin_and_other_dan_courses() {
    check(vec![
        Case {
            name: "ln phase",
            folder: LN,
            version: "9th",
            creator: "Jinjin",
            set_id: None,
            expected: vec![lab(
                "jinjin_dan_ln",
                "jinjin_dan_ln",
                Some(9.0),
                "9th",
                Some("ln_overall"),
                false,
            )],
        },
        Case {
            name: "ln phase in an Emperor folder is not Jinjin",
            folder: "Emperor - 7K Dan Course - LN Dan Phase",
            version: "9th",
            creator: "Emperor",
            set_id: None,
            expected: vec![],
        },
        Case {
            name: "old ln insane 2",
            folder: "1 Jinjin - 7K Dan Course - Insane Level 2 (LN)",
            version: "5th",
            creator: "Jinjin",
            set_id: None,
            expected: vec![lab(
                "jinjin_dan_ln_v1",
                "jinjin_dan_ln_v1",
                Some(5.0),
                "5th",
                Some("ln_overall"),
                false,
            )],
        },
        Case {
            name: "old ln insane 3",
            folder: "2 Jinjin - 7K Dan Course - Insane Level 3 (LN)",
            version: "8th",
            creator: "Jinjin",
            set_id: None,
            expected: vec![lab(
                "jinjin_dan_ln_v1",
                "jinjin_dan_ln_v1",
                Some(8.0),
                "8th",
                Some("ln_overall"),
                false,
            )],
        },
        Case {
            name: "old ln extra",
            folder: "3 Jinjin - 7K Dan Course - Extra Level (LN)",
            version: "Gamma",
            creator: "Jinjin",
            set_id: None,
            expected: vec![lab(
                "jinjin_dan_ln_v1",
                "jinjin_dan_ln_v1",
                Some(11.0),
                "gamma",
                Some("ln_overall"),
                false,
            )],
        },
        Case {
            name: "old ln insane 1 is not a v1 course",
            folder: "4 Jinjin - 7K Dan Course - Insane Level 1 (LN)",
            version: "1st",
            creator: "Jinjin",
            set_id: None,
            expected: vec![],
        },
        Case {
            name: "old ln without the (LN) suffix",
            folder: "5 Jinjin - 7K Dan Course - Insane Level 2",
            version: "1st",
            creator: "Jinjin",
            set_id: None,
            expected: vec![],
        },
        Case {
            name: "emperor",
            folder: "77 Various - Emperor's 7K LN Dan",
            version: "3rd Dan",
            creator: "Emperor",
            set_id: None,
            expected: vec![lab(
                "emperor_ln_dan",
                "emperor_ln_dan",
                Some(3.0),
                "3rd",
                Some("ln_overall"),
                false,
            )],
        },
        Case {
            name: "wild",
            folder: "88 tyrcs - Wild 7K Dan Course",
            version: "7th Dan",
            creator: "tyrcs",
            set_id: None,
            expected: vec![lab(
                "wild_dan",
                "wild_dan",
                Some(7.0),
                "7th",
                Some("regular_overall"),
                false,
            )],
        },
        Case {
            name: "delete marker skips every class",
            folder: REGULAR,
            version: "Delete Upon download 9th",
            creator: "Jinjin",
            set_id: None,
            expected: vec![],
        },
        Case {
            name: "unrelated chart",
            folder: "123 Artist - Title",
            version: "Hard",
            creator: "Mapper",
            set_id: Some(123),
            expected: vec![],
        },
    ]);
}

fn kd(ord: Option<f64>, text: &str, skill: &str, variant: bool) -> Vec<ChartLabel> {
    vec![lab(
        "komeijidove_practice",
        "jinjin_dan",
        ord,
        text,
        Some(skill),
        variant,
    )]
}

#[test]
fn komeijidove_practice() {
    check(vec![
        Case {
            name: "folder id",
            folder: KD_JACK,
            version: "~ 9th ~ Some Song",
            creator: "KomeijiDove",
            set_id: Some(1877617),
            expected: kd(Some(9.0), "9th", "jack", false),
        },
        Case {
            name: "dash prefix and named level",
            folder: "1877625 Various Artists - KomeijiDove 7K Tech Practice",
            version: "- Gamma ~ Other Song",
            creator: "KomeijiDove",
            set_id: None,
            expected: kd(Some(11.0), "gamma", "tech", false),
        },
        Case {
            name: "set id when the folder has no id",
            folder: "KomeijiDove 7K LN Release Practice",
            version: "~ Stellium ~ Song",
            creator: "KomeijiDove",
            set_id: Some(1888027),
            expected: kd(Some(14.0), "stellium", "ln_release", false),
        },
        Case {
            name: "folder id wins over set id",
            folder: "1877636 Various Artists - KomeijiDove 7K Speed Practice",
            version: "~ 0th ~ Song",
            creator: "KomeijiDove",
            set_id: Some(1877727),
            expected: kd(Some(0.0), "0th", "speed", false),
        },
        Case {
            name: "unsubmitted local copy keeps its folder id",
            folder: "1877727 Various Artists - KomeijiDove 7K Stream Practice",
            version: "~ 4th ~ Song",
            creator: "KomeijiDove",
            set_id: Some(-1),
            expected: kd(Some(4.0), "4th", "stream", false),
        },
        Case {
            name: "every LN slot",
            folder: "1887981 KomeijiDove LN General",
            version: "~ 1st ~ Song",
            creator: "KomeijiDove",
            set_id: None,
            expected: kd(Some(1.0), "1st", "ln_general", false),
        },
        Case {
            name: "ln tech",
            folder: "1888000 KomeijiDove LN Tech",
            version: "~ 2nd ~ Song",
            creator: "KomeijiDove",
            set_id: None,
            expected: kd(Some(2.0), "2nd", "ln_tech", false),
        },
        Case {
            name: "ln inverse",
            folder: "1888009 KomeijiDove LN Inverse",
            version: "~ Zenith ~ Song",
            creator: "KomeijiDove",
            set_id: None,
            expected: kd(Some(13.0), "zenith", "ln_inverse", false),
        },
        Case {
            name: "rate edit is a variant",
            folder: KD_JACK,
            version: "~ 10th ~ Paraclete 0.86x (172bpm)",
            creator: "KomeijiDove",
            set_id: None,
            expected: kd(Some(10.0), "10th", "jack", true),
        },
        Case {
            name: "od edit is a variant",
            folder: KD_JACK,
            version: "~ 8th ~ Song OD8",
            creator: "KomeijiDove",
            set_id: None,
            expected: kd(Some(8.0), "8th", "jack", true),
        },
        Case {
            name: "full ln edit is a variant",
            folder: KD_JACK,
            version: "~ 8th ~ Song +FLN",
            creator: "KomeijiDove",
            set_id: None,
            expected: kd(Some(8.0), "8th", "jack", true),
        },
        Case {
            name: "bracketed rate at the end is a variant",
            folder: KD_JACK,
            version: "~ 8th ~ Song [0.92x]",
            creator: "KomeijiDove",
            set_id: None,
            expected: kd(Some(8.0), "8th", "jack", true),
        },
        Case {
            name: "section cut is a variant",
            folder: KD_JACK,
            version: "- 9th / Section 2 ~ Song",
            creator: "KomeijiDove",
            set_id: None,
            expected: kd(Some(9.0), "9th", "jack", true),
        },
        Case {
            name: "no closing tilde leaves the level unparsed",
            folder: KD_JACK,
            version: "~ 9th Song",
            creator: "KomeijiDove",
            set_id: None,
            expected: kd(None, "", "jack", false),
        },
        Case {
            name: "blank level between tildes",
            folder: KD_JACK,
            version: "~ ~ Song",
            creator: "KomeijiDove",
            set_id: None,
            expected: kd(None, "", "jack", false),
        },
        Case {
            name: "prefix without a space is not a practice chart",
            folder: KD_JACK,
            version: "~9th ~ Song",
            creator: "KomeijiDove",
            set_id: None,
            expected: vec![],
        },
        Case {
            name: "unknown set id",
            folder: "1877618 Various Artists - Other",
            version: "~ 9th ~ Song",
            creator: "KomeijiDove",
            set_id: Some(1877618),
            expected: vec![],
        },
    ]);
}

fn rtg(skill: Option<&str>, variant: bool) -> Vec<ChartLabel> {
    vec![lab(
        "road_to_gamma",
        "jinjin_dan",
        None,
        "gamma_entry",
        skill,
        variant,
    )]
}

#[test]
fn road_to_gamma() {
    check(vec![
        Case {
            name: "skill after the last //",
            folder: RTG,
            version: "Artist - Song // Jack 1",
            creator: "Mapper",
            set_id: None,
            expected: rtg(Some("jack"), false),
        },
        Case {
            name: "rate edit",
            folder: RTG,
            version: "Song // a // Speed 0.9x (180bpm)",
            creator: "Mapper",
            set_id: None,
            expected: rtg(Some("speed"), true),
        },
        Case {
            name: "without // the first word is the skill",
            folder: RTG,
            version: "Chordjack Song",
            creator: "Mapper",
            set_id: None,
            expected: rtg(Some("chordjack"), false),
        },
        Case {
            name: "nothing after //",
            folder: RTG,
            version: "Song //   ",
            creator: "Mapper",
            set_id: None,
            expected: rtg(None, false),
        },
        Case {
            // Python's str.split scans left to right: "a///Jack" splits into "a" and "/Jack".
            name: "odd slash runs split from the left",
            folder: RTG,
            version: "Song ///Jack",
            creator: "Mapper",
            set_id: None,
            expected: rtg(Some("/jack"), false),
        },
    ]);
}

fn bms(scale: &str, ord: Option<f64>, text: &str) -> ChartLabel {
    lab(
        "bms_5ynt3ck",
        scale,
        ord,
        text,
        Some("regular_overall"),
        false,
    )
}

#[test]
fn bms_tables() {
    check(vec![
        Case {
            name: "single tag",
            folder: BMS_FOLDER,
            version: "[BMS] [i_4]",
            creator: "5ynt3ck",
            set_id: None,
            expected: vec![bms("bms_i", Some(4.0), "4")],
        },
        Case {
            name: "multi tag with plus and minus",
            folder: BMS_FOLDER,
            version: "[n2_12+] [sr_3-] [st_0]",
            creator: "5ynt3ck",
            set_id: None,
            expected: vec![
                bms("bms_n2", Some(12.0 + 0.33), "12+"),
                bms("bms_sr", Some(3.0 - 0.33), "3-"),
                bms("bms_st", Some(0.0), "0"),
            ],
        },
        Case {
            name: "non-numeric level",
            folder: BMS_FOLDER,
            version: "[oj_?]",
            creator: "5ynt3ck",
            set_id: None,
            expected: vec![bms("bms_oj", None, "?")],
        },
        Case {
            name: "a level runs to the first ] even across [",
            folder: BMS_FOLDER,
            version: "[i_4 [x_5]",
            creator: "5ynt3ck",
            set_id: None,
            expected: vec![bms("bms_i", None, "4 [x_5")],
        },
        Case {
            name: "two digits after the table letters are no tag",
            folder: BMS_FOLDER,
            version: "[ab12_3] [sl_10]",
            creator: "5ynt3ck",
            set_id: None,
            expected: vec![bms("bms_sl", Some(10.0), "10")],
        },
        Case {
            name: "identical tags collapse to one label",
            folder: BMS_FOLDER,
            version: "[sl_10] [sl_10] [sl_11]",
            creator: "5ynt3ck",
            set_id: None,
            expected: vec![
                bms("bms_sl", Some(10.0), "10"),
                bms("bms_sl", Some(11.0), "11"),
            ],
        },
        Case {
            name: "untagged",
            folder: BMS_FOLDER,
            version: "[BMS] Another",
            creator: "5ynt3ck",
            set_id: None,
            expected: vec![lab("bms_5ynt3ck", "bms_unlabelled", None, "", None, false)],
        },
        Case {
            name: "another creator",
            folder: BMS_FOLDER,
            version: "[i_4]",
            creator: "Mapper",
            set_id: None,
            expected: vec![],
        },
        Case {
            name: "folder without the prefix",
            folder: "Artist - Title [_BMS_]",
            version: "[i_4]",
            creator: "5ynt3ck",
            set_id: None,
            expected: vec![],
        },
        Case {
            name: "a bms chart never falls through to o2jam",
            folder: BMS_FOLDER,
            version: "[O2Jam] [H] [30]",
            creator: "5ynt3ck",
            set_id: None,
            expected: vec![lab("bms_5ynt3ck", "bms_unlabelled", None, "", None, false)],
        },
    ]);
}

fn o2jam(scale: &str, ord: f64, text: &str) -> Vec<ChartLabel> {
    vec![lab("o2jam", scale, Some(ord), text, None, false)]
}

#[test]
fn o2jam_levels() {
    check(vec![
        Case {
            name: "hard",
            folder: "[_O2Jam_] Artist - Title",
            version: "[O2Jam] [H] [34]",
            creator: "Mapper",
            set_id: None,
            expected: o2jam("o2jam_h", 34.0, "34"),
        },
        Case {
            name: "normal with a tail",
            folder: "[_O2Jam_] Artist - Title",
            version: "[O2Jam] [N] [5] extra",
            creator: "Mapper",
            set_id: None,
            expected: o2jam("o2jam_n", 5.0, "5"),
        },
        Case {
            name: "two-letter difficulty",
            folder: "[_O2Jam_] Artist - Title",
            version: "[O2Jam] [HX] [3]",
            creator: "Mapper",
            set_id: None,
            expected: vec![],
        },
        Case {
            name: "anchored at the start",
            folder: "[_O2Jam_] Artist - Title",
            version: " [O2Jam] [H] [3]",
            creator: "Mapper",
            set_id: None,
            expected: vec![],
        },
        Case {
            name: "pack lvl in an o2jam folder, any case",
            folder: "O2JAM Pack #3",
            version: "Song [lvl 42]",
            creator: "Mapper",
            set_id: None,
            expected: o2jam("o2jam_pack_lvl", 42.0, "42"),
        },
        Case {
            name: "pack lvl outside an o2jam folder",
            folder: "Some Pack",
            version: "Song [lvl 42]",
            creator: "Mapper",
            set_id: None,
            expected: vec![],
        },
        Case {
            name: "Lv. for O2Jam",
            folder: "Some Pack",
            version: "Song Lv.37 For O2Jam",
            creator: "Mapper",
            set_id: None,
            expected: o2jam("o2jam_pack_lvl", 37.0, "37"),
        },
        Case {
            name: "the level is the first Lv. in the name",
            folder: "Some Pack",
            version: "Lv.3 Easy Lv.12 For O2Jam",
            creator: "Mapper",
            set_id: None,
            expected: o2jam("o2jam_pack_lvl", 3.0, "3"),
        },
    ]);
}

#[test]
fn o2jam_low_levels_are_suspect() {
    let low = run("[_O2Jam_] A - T", "[O2Jam] [H] [9]", "Mapper", None);
    let ok = run("[_O2Jam_] A - T", "[O2Jam] [H] [10]", "Mapper", None);
    let pack = run("o2jam pack", "Song [lvl 3]", "Mapper", None);
    let dan = run(REGULAR, "1st", "Jinjin", None);
    assert!(low[0].is_suspect_o2jam_level());
    assert!(!ok[0].is_suspect_o2jam_level());
    assert!(!pack[0].is_suspect_o2jam_level());
    assert!(!dan[0].is_suspect_o2jam_level());
}

fn practice(target: &str, skill: Option<&str>, variant: bool) -> Vec<ChartLabel> {
    vec![lab(
        "other_dan_practice",
        "jinjin_dan",
        None,
        target,
        skill,
        variant,
    )]
}

#[test]
fn practice_packs() {
    check(vec![
        Case {
            name: "gamma pack",
            folder: "Various - Gamma Practice Pack",
            version: "Song (Chordjack)",
            creator: "Mapper",
            set_id: None,
            expected: practice("gamma_entry", Some("chordjack"), false),
        },
        Case {
            name: "azimuth pack, lower case",
            folder: "various - azimuth dan practice pack",
            version: "Song (Stamina) [1.1x]",
            creator: "Mapper",
            set_id: None,
            expected: practice("azimuth_entry", Some("stamina"), true),
        },
        Case {
            name: "gamma anywhere in the folder picks gamma",
            folder: "Gamma Fans - Azimuth Dan Practice Pack",
            version: "Song (Delay)",
            creator: "Mapper",
            set_id: None,
            expected: practice("gamma_entry", Some("delay"), false),
        },
        Case {
            name: "skill is case-sensitive",
            folder: "Gamma Practice Pack",
            version: "Song (jack)",
            creator: "Mapper",
            set_id: None,
            expected: practice("gamma_entry", None, false),
        },
        Case {
            name: "first skill in the name",
            folder: "Gamma Practice Pack",
            version: "(Bracket) Song (Tech)",
            creator: "Mapper",
            set_id: None,
            expected: practice("gamma_entry", Some("bracket"), false),
        },
    ]);
}

#[test]
fn class_precedence_follows_the_elif_chain() {
    // A Jinjin folder wins over the BMS creator/prefix test.
    assert_eq!(
        run(
            "[_BMS_] 7K Dan Course - Regular Dan Phase",
            "[i_4]",
            "5ynt3ck",
            None
        ),
        jinjin_regular(None, "")
    );
    // KomeijiDove wins over Wild.
    assert_eq!(
        run(
            "1877617 Wild 7K Dan Course",
            "~ 2nd ~ Song",
            "KomeijiDove",
            None
        ),
        kd(Some(2.0), "2nd", "jack", false)
    );
}

#[test]
fn rate_marker() {
    let cases = [
        ("1.25x (200bpm)", true),
        ("Song 1x (200bpm)", true),
        ("Song x (200bpm)", false),
        ("Song 1.x (200bpm)", false),
        ("Song 1.2x(200bpm)", false),
        ("Song 1.2x (bpm)", false),
        ("OD8 Song", true),
        ("Song -OD9", true),
        ("Song HOD8", false),
        ("Song OD", false),
        ("Song +FLN", true),
        ("Song [1.25x]", true),
        ("Song [1.25x] extra", false),
        ("Song [12.5x]", false),
        ("Song [1x]", false),
        ("Song", false),
    ];
    for (version, expected) in cases {
        assert_eq!(has_rate_marker(version), expected, "{version:?}");
    }
}

#[test]
fn level_family_of_every_source() {
    for dan in [
        source::JINJIN_DAN_REGULAR,
        source::JINJIN_DAN_LN,
        source::JINJIN_DAN_LN_V1,
        source::EMPEROR_LN_DAN,
        source::KOMEIJIDOVE_PRACTICE,
        source::WILD_DAN,
        source::ROAD_TO_GAMMA,
        source::OTHER_DAN_PRACTICE,
    ] {
        assert_eq!(level_family(dan), Some(LevelFamily::Dan), "{dan}");
    }
    assert_eq!(level_family(source::BMS_5YNT3CK), Some(LevelFamily::Bms));
    assert_eq!(level_family(source::O2JAM), Some(LevelFamily::O2jam));
    assert_eq!(level_family("nope"), None);
    let ids: Vec<&str> = LevelFamily::ALL.iter().map(|f| f.as_str()).collect();
    assert_eq!(ids, ["dan", "bms", "o2jam"]);
}
