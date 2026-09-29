//! Ordinal difficulty labels read from a chart's catalog metadata (research 03 l.296–303).
//!
//! A faithful port of `research/scripts/audit/labels.py`: the class order, the stable ids and the
//! level parsing match it, so label counts compare directly with the audit. Divergences:
//! identical BMS tags on one chart collapse to one label (the store keys labels by
//! (scale, level_text)), and the audit's free-text notes are not carried; the one note that
//! matters downstream is [`ChartLabel::is_suspect_o2jam_level`].

mod scan;

use scan::{
    bms_tags, first_number_between, first_parenthesized, has_rate_marker, o2jam_header,
    ordinal_prefix, signed_level, tilde_level,
};

/// Catalog fields of one chart; labels come only from these local strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LabelInput<'a> {
    /// Songs folder name, not a path.
    pub folder: &'a str,
    pub version: &'a str,
    pub creator: &'a str,
    pub set_id: Option<i32>,
}

/// Same fields and meaning as the store's `ChartLabel`; `source`, `scale` and `skill_tag` are
/// persisted stable ids.
#[derive(Debug, Clone, PartialEq)]
pub struct ChartLabel {
    pub source: String,
    pub scale: String,
    /// `None` when the level has no position on its scale (e.g. `gamma_entry`, a BMS `?`).
    pub level_ord: Option<f64>,
    /// The level as written in the name (`9th`, `12+`, `34`), a target such as `gamma_entry`, or
    /// empty when nothing parsed.
    pub level_text: String,
    pub skill_tag: Option<String>,
    /// Rate, OD or full-LN edits and section cuts: evidence for rate models, not labels.
    pub is_variant: bool,
}

impl ChartLabel {
    /// research 03 DESIGN: O2Jam `[H]`-style levels under 10 look bogus; pack-style levels are not
    /// flagged, as in `labels.py`.
    pub fn is_suspect_o2jam_level(&self) -> bool {
        self.source == source::O2JAM
            && self.scale != scale::O2JAM_PACK_LVL
            && self
                .level_ord
                .is_some_and(|l| l < O2JAM_MIN_PLAUSIBLE_LEVEL)
    }
}

pub mod source {
    pub const JINJIN_DAN_REGULAR: &str = "jinjin_dan_regular";
    pub const JINJIN_DAN_LN: &str = "jinjin_dan_ln";
    pub const JINJIN_DAN_LN_V1: &str = "jinjin_dan_ln_v1";
    pub const EMPEROR_LN_DAN: &str = "emperor_ln_dan";
    pub const KOMEIJIDOVE_PRACTICE: &str = "komeijidove_practice";
    pub const WILD_DAN: &str = "wild_dan";
    pub const ROAD_TO_GAMMA: &str = "road_to_gamma";
    pub const BMS_5YNT3CK: &str = "bms_5ynt3ck";
    pub const O2JAM: &str = "o2jam";
    pub const OTHER_DAN_PRACTICE: &str = "other_dan_practice";
}

/// How a source's levels compare: dan ordinals share one ladder across sources, while BMS and
/// O2Jam levels only order charts within their own scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LevelFamily {
    Dan,
    Bms,
    O2jam,
}

impl LevelFamily {
    pub const ALL: [Self; 3] = [Self::Dan, Self::Bms, Self::O2jam];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dan => "dan",
            Self::Bms => "bms",
            Self::O2jam => "o2jam",
        }
    }
}

/// `None` for a source this build does not know.
pub fn level_family(source: &str) -> Option<LevelFamily> {
    match source {
        source::JINJIN_DAN_REGULAR
        | source::JINJIN_DAN_LN
        | source::JINJIN_DAN_LN_V1
        | source::EMPEROR_LN_DAN
        | source::KOMEIJIDOVE_PRACTICE
        | source::WILD_DAN
        | source::ROAD_TO_GAMMA
        | source::OTHER_DAN_PRACTICE => Some(LevelFamily::Dan),
        source::BMS_5YNT3CK => Some(LevelFamily::Bms),
        source::O2JAM => Some(LevelFamily::O2jam),
        _ => None,
    }
}

/// Dan-course sources use their source id as their scale.
pub mod scale {
    pub const JINJIN_DAN: &str = "jinjin_dan";
    pub const BMS_UNLABELLED: &str = "bms_unlabelled";
    pub const O2JAM_PACK_LVL: &str = "o2jam_pack_lvl";
    /// Followed by the table tag (`bms_st`, `bms_n2`).
    pub const BMS_PREFIX: &str = "bms_";
    /// Followed by the lowercased O2Jam difficulty char (`o2jam_h`): scale ids are persisted `StableId`s,
    /// lowercase by convention, unlike `labels.py` (`o2jam_H`).
    pub const O2JAM_PREFIX: &str = "o2jam_";
}

pub mod skill {
    pub const REGULAR_OVERALL: &str = "regular_overall";
    pub const LN_OVERALL: &str = "ln_overall";
}

/// Named dans after 10th, ladder order per mania-hub `algorithms/player/dan-courses.ts:128-158`.
pub const DAN_NAMED_ORDINALS: [(&str, u8); 4] = [
    ("gamma", 11),
    ("azimuth", 12),
    ("zenith", 13),
    ("stellium", 14),
];

/// KomeijiDove practice beatmap sets → skill slot (research 03: 8 skills × 15 levels).
pub const KOMEIJIDOVE_SETS: [(i32, &str); 8] = [
    (1877617, "jack"),
    (1877625, "tech"),
    (1877636, "speed"),
    (1877727, "stream"),
    (1887981, "ln_general"),
    (1888000, "ln_tech"),
    (1888009, "ln_inverse"),
    (1888027, "ln_release"),
];

/// A BMS `+`/`-` level sits this far from its integer level (`labels.py`).
pub const BMS_LEVEL_SIGN_STEP: f64 = 0.33;

pub const O2JAM_MIN_PLAUSIBLE_LEVEL: f64 = 10.0;

const DELETE_MARKER: &str = "Delete Upon download";
const REGULAR_PHASE: &str = "7K Dan Course - Regular Dan Phase";
const LN_PHASE: &str = "7K Dan Course - LN Dan Phase";
const LN_V1_COURSES: [&str; 3] = [
    "7K Dan Course - Insane Level 2 (LN)",
    "7K Dan Course - Insane Level 3 (LN)",
    "7K Dan Course - Extra Level (LN)",
];
const EMPEROR: &str = "Emperor";
const EMPEROR_LN_DAN: &str = "Emperor's 7K LN Dan";
const WILD_DAN: &str = "Wild 7K Dan Course";
const ROAD_TO_GAMMA: &str = "7K Road to Gamma Dan Pack";
const BMS_CREATOR: &str = "5ynt3ck";
const BMS_FOLDER_PREFIX: &str = "[_BMS_]";
const PRACTICE_PACKS: [&str; 2] = ["gamma practice pack", "azimuth dan practice pack"];
const PRACTICE_SKILLS: [&str; 8] = [
    "Delay",
    "Chordjack",
    "Bracket",
    "Stamina",
    "Tech",
    "Jack",
    "Speed",
    "Stream",
];
const GAMMA_ENTRY: &str = "gamma_entry";
const AZIMUTH_ENTRY: &str = "azimuth_entry";

/// Labels of one chart in `labels.py` order: the first matching class wins, and BMS yields one
/// label per table tag.
pub fn extract_labels(input: &LabelInput<'_>) -> Vec<ChartLabel> {
    let LabelInput {
        folder: f,
        version: v,
        creator,
        set_id,
    } = *input;
    if v.starts_with(DELETE_MARKER) {
        return Vec::new();
    }
    let dan = |source: &str, skill: &str| {
        let (ord, text) = dan_ordinal(v);
        vec![label(source, source, ord, text, Some(skill), false)]
    };
    if f.contains(REGULAR_PHASE) {
        return dan(source::JINJIN_DAN_REGULAR, skill::REGULAR_OVERALL);
    }
    if f.contains(LN_PHASE) && !f.contains(EMPEROR) {
        return dan(source::JINJIN_DAN_LN, skill::LN_OVERALL);
    }
    if LN_V1_COURSES.iter().any(|c| f.contains(c)) {
        return dan(source::JINJIN_DAN_LN_V1, skill::LN_OVERALL);
    }
    if f.contains(EMPEROR_LN_DAN) {
        return dan(source::EMPEROR_LN_DAN, skill::LN_OVERALL);
    }
    let practice_prefix = v.starts_with("~ ") || v.starts_with("- ");
    if let Some(slot) = komeijidove_slot(f, set_id).filter(|_| practice_prefix) {
        let level = tilde_level(v);
        let (ord, text) = dan_ordinal(level);
        let variant = has_rate_marker(v) || level.contains('/');
        return vec![label(
            source::KOMEIJIDOVE_PRACTICE,
            scale::JINJIN_DAN,
            ord,
            text,
            Some(slot),
            variant,
        )];
    }
    if f.contains(WILD_DAN) {
        return dan(source::WILD_DAN, skill::REGULAR_OVERALL);
    }
    if f.contains(ROAD_TO_GAMMA) {
        let tail = v.split("//").last().unwrap_or(v).trim();
        let skill = tail.split(' ').next().unwrap_or(tail).to_lowercase();
        return vec![label(
            source::ROAD_TO_GAMMA,
            scale::JINJIN_DAN,
            None,
            GAMMA_ENTRY.into(),
            Some(&skill),
            has_rate_marker(v),
        )];
    }
    if creator == BMS_CREATOR && f.starts_with(BMS_FOLDER_PREFIX) {
        return bms_labels(v);
    }
    if let Some((diff, level)) = o2jam_header(v) {
        let scale = format!("{}{}", scale::O2JAM_PREFIX, diff.to_ascii_lowercase());
        return vec![o2jam_label(&scale, level)];
    }
    let folder_lower = f.to_lowercase();
    if folder_lower.contains("o2jam")
        && let Some(level) = first_number_between(v, "[lvl ", "]")
    {
        return vec![o2jam_label(scale::O2JAM_PACK_LVL, level)];
    }
    // labels.py takes the first `Lv.N` of the name, not necessarily the one before " For O2Jam".
    if first_number_between(v, "Lv.", " For O2Jam").is_some()
        && let Some(level) = first_number_between(v, "Lv.", "")
    {
        return vec![o2jam_label(scale::O2JAM_PACK_LVL, level)];
    }
    if PRACTICE_PACKS.iter().any(|p| folder_lower.contains(p)) {
        let target = if folder_lower.contains("gamma") {
            GAMMA_ENTRY
        } else {
            AZIMUTH_ENTRY
        };
        let skill = first_parenthesized(v, &PRACTICE_SKILLS).map(str::to_lowercase);
        return vec![label(
            source::OTHER_DAN_PRACTICE,
            scale::JINJIN_DAN,
            None,
            target.into(),
            skill.as_deref(),
            has_rate_marker(v),
        )];
    }
    Vec::new()
}

fn label(
    source: &str,
    scale: &str,
    level_ord: Option<f64>,
    level_text: String,
    skill: Option<&str>,
    is_variant: bool,
) -> ChartLabel {
    ChartLabel {
        source: source.into(),
        scale: scale.into(),
        level_ord,
        level_text,
        // labels.py writes a missing skill as ''.
        skill_tag: skill.filter(|s| !s.is_empty()).map(Into::into),
        is_variant,
    }
}

/// `dan_ord` of `labels.py`; the text is the token that matched.
fn dan_ordinal(raw: &str) -> (Option<f64>, String) {
    let s = raw.trim().to_lowercase();
    if let Some((number, token)) = ordinal_prefix(&s) {
        return (number.parse().ok(), token.into());
    }
    DAN_NAMED_ORDINALS
        .iter()
        .find(|(name, _)| s.starts_with(name))
        .map_or((None, String::new()), |&(name, ord)| {
            (Some(f64::from(ord)), name.into())
        })
}

/// The folder's leading id is tried before the set id, and compared as a string, as in
/// `labels.py`.
fn komeijidove_slot(folder: &str, set_id: Option<i32>) -> Option<&'static str> {
    let folder_id = folder.split(' ').next().unwrap_or(folder);
    let by_folder = KOMEIJIDOVE_SETS
        .iter()
        .find(|(id, _)| id.to_string() == folder_id);
    let by_set = || KOMEIJIDOVE_SETS.iter().find(|(id, _)| Some(*id) == set_id);
    by_folder.or_else(by_set).map(|&(_, slot)| slot)
}

fn bms_labels(v: &str) -> Vec<ChartLabel> {
    let tags = bms_tags(v);
    if tags.is_empty() {
        return vec![label(
            source::BMS_5YNT3CK,
            scale::BMS_UNLABELLED,
            None,
            String::new(),
            None,
            false,
        )];
    }
    let mut out: Vec<ChartLabel> = Vec::with_capacity(tags.len());
    for (table, level) in tags {
        let scale = format!("{}{table}", scale::BMS_PREFIX);
        if out
            .iter()
            .any(|l| l.scale == scale && l.level_text == level)
        {
            continue;
        }
        let ord = signed_level(level).and_then(|(number, sign)| {
            let n: f64 = number.parse().ok()?;
            Some(match sign {
                Some('+') => n + BMS_LEVEL_SIGN_STEP,
                Some(_) => n - BMS_LEVEL_SIGN_STEP,
                None => n,
            })
        });
        out.push(label(
            source::BMS_5YNT3CK,
            &scale,
            ord,
            level.into(),
            Some(skill::REGULAR_OVERALL),
            false,
        ));
    }
    out
}

fn o2jam_label(scale: &str, level: &str) -> ChartLabel {
    label(
        source::O2JAM,
        scale,
        level.parse().ok(),
        level.into(),
        None,
        false,
    )
}

#[cfg(test)]
mod tests;
