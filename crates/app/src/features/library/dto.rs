//! Library DTOs (D13): camelCase on the wire, no 64-bit integers (spec 005).

use serde::{Deserialize, Serialize};

/// Label bounds are inclusive, and any label criterion keeps only charts with a matching
/// label, ordered by scale and level; without one the order is by md5.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LibraryFilterDto {
    pub keymode: u8,
    pub scale: Option<String>,
    pub level_min: Option<f64>,
    pub level_max: Option<f64>,
    pub label_source: Option<String>,
    /// Case-insensitive substring of title, artist, difficulty name or creator.
    pub text: Option<String>,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ChartLabelDto {
    pub source: String,
    pub scale: String,
    pub level_ord: Option<f64>,
    pub level_text: String,
    pub skill_tag: Option<String>,
    pub is_variant: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LibraryChartDto {
    pub md5: String,
    pub title: String,
    pub artist: String,
    /// The difficulty name.
    pub version: String,
    pub creator: String,
    pub keymode: u8,
    pub n_notes: u32,
    pub n_ln: u32,
    pub ln_ratio: f64,
    pub length_ms: u32,
    pub nps: f64,
    pub labels: Vec<ChartLabelDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ChartDetailDto {
    pub chart: LibraryChartDto,
    /// Decoder warnings of the `.osu` as it is now; `None` when the file is unavailable.
    pub diagnostics: Option<u32>,
    pub segments: Vec<SegmentDto>,
}

/// One pattern segment under the keymode profile's default layout, rows `t0Ms..=t1Ms`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SegmentDto {
    pub t0_ms: i32,
    pub t1_ms: i32,
    /// Bit `i` is column `i`.
    pub cols: u16,
    pub pattern_id: String,
    /// The taxonomy's short key.
    pub key: String,
    pub axis_id: String,
    pub secondary: Vec<String>,
    /// Permille.
    pub purity: u16,
    /// Permille.
    pub strength: u16,
}

/// Primary segments of one pattern over the library.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PatternCountDto {
    pub keymode: u8,
    pub pattern_id: String,
    pub key: String,
    pub axis_id: String,
    pub segments: u32,
    pub charts: u32,
    pub total_s: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ScaleCountDto {
    pub scale: String,
    pub rows: u32,
    pub charts: u32,
}
