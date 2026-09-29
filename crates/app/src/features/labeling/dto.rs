//! Labelling DTOs (D13): camelCase on the wire, no 64-bit integers (spec 005). Times cross as
//! whole milliseconds, which is what osu! stores; the export file keeps microseconds.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PatternDefDto {
    pub id: String,
    pub axis: String,
    /// Short key for typing labels.
    pub key: String,
    pub description: String,
}

/// `[t0Ms, t1Ms)` of one chart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AnchorDto {
    pub md5: String,
    pub t0_ms: i32,
    pub t1_ms: i32,
    /// 1-based, column 1 leftmost, ascending.
    pub cols: Vec<u8>,
}

/// One labelling round. Label bounds are inclusive and match as in the library listing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SampleRequestDto {
    pub keymode: u8,
    /// A `u64` in decimal: the sequence is a pure function of it and the labelled set.
    pub seed: String,
    pub round: u32,
    /// Defaults to the sampler's window.
    pub window_ms: Option<u32>,
    pub scale: Option<String>,
    pub level_min: Option<f64>,
    pub level_max: Option<f64>,
    /// Windows already shown this session (skips are not stored); labelled ones are always
    /// avoided.
    pub exclude: Vec<AnchorDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LabelWindowDto {
    pub anchor: AnchorDto,
    pub title: String,
    pub artist: String,
    pub version: String,
    /// `scale:level` of the label that placed the chart in its stratum.
    pub level: Option<String>,
    /// e.g. `dan_07/nps_2`; display only, never persisted.
    pub stratum: String,
    pub played: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LabelSubmitDto {
    pub anchor: AnchorDto,
    /// Full pattern ids of the chart's keymode: at least one, unless `no_pattern`.
    pub patterns: Vec<String>,
    /// "No clear pattern": a stored answer with empty `patterns` (a skip is never stored).
    pub no_pattern: bool,
    pub mixed: bool,
    pub unsure: bool,
    /// `None` is neutral.
    pub thumb_pref: Option<ThumbPrefDto>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ThumbPrefDto {
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LabelEventDto {
    /// The feedback event's ULID; undo takes it back.
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CountDto {
    pub key: String,
    pub count: u32,
}

/// Over the self profile's labels that are not undone. Lists are sorted by key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LabelStatsDto {
    pub total: u32,
    /// Answers of "no clear pattern".
    pub no_pattern: u32,
    pub mixed: u32,
    pub unsure: u32,
    pub thumb_left: u32,
    pub thumb_right: u32,
    pub per_pattern: Vec<CountDto>,
    pub per_axis: Vec<CountDto>,
    /// A chart no longer in the library counts under `unknown`.
    pub per_stratum: Vec<CountDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LabelExportDto {
    pub path: String,
    pub rows: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum WindowOpDto {
    Widen,
    Narrow,
    Next,
    Prev,
}
