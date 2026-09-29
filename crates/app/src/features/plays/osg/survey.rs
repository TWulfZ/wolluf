//! `wolluf osg survey` (spec 006 Behaviour): checks the structural hypotheses (I1–I8) on every
//! `.osg` of a `Data/r` against the paired `.osr` header, osu!.db object counts and scores.db.
//! The corpus is only ever read (R8, AC7). The Python oracle `research/scripts/osg/osg.py
//! --survey` must agree on I1–I5 counts (AC9), so the `na` rules below mirror it. The one
//! deliberate divergence is I3: ADR 0012 item 7 accepts the final-record FC flag, which the
//! oracle still counts as an I3 failure (its `I3_*` rows still agree).

use std::collections::BTreeMap;
use std::fmt::{Display, Write as _};
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use rayon::prelude::*;
use serde::Serialize;
use tokio_util::sync::CancellationToken;
use wolluf_core::{ChartMd5, DotNetTicks, ErrorCode, FileTime};
use wolluf_source_osu::CodecError;
use wolluf_source_osu::codec::osg::{
    OsgFile, OsgRecord, STRIDE_V1, STRIDE_V2, decode_osg, is_fc_flag,
};
use wolluf_source_osu::codec::osu_db::decode_osu_db;
use wolluf_source_osu::codec::score_header::{JudgementCounts, ScoreHeader, read_score_header};
use wolluf_source_osu::codec::scores_db::decode_scores_db;
use wolluf_source_osu::codec::{FileKind, Reader};
use wolluf_source_osu::replay_dir::{self, ReplayFiles};
use wolluf_source_osu::snapshot::{SnapshotPolicy, read_stable};
use wolluf_store::time::format_rfc3339_ms;

use crate::errors::AppError;
use crate::features::plays::record::chart_md5;
use crate::features::plays::sync::{MANIA_MODE, OSU_DB, SCORES_DB, root_file};

/// Spec 006 Behaviour: up to 5 example file names per failing invariant.
const DEFAULT_EXAMPLES: usize = 5;
/// Every field the survey needs precedes the life-bar string; a long life bar only costs a
/// second, full read.
const OSR_HEADER_PREFIX_BYTES: u64 = 16 * 1024;
const UNKNOWN: &str = "unknown";
const EMPTY_STRIDE: &str = "empty";
/// Month bucket (`YYYY-MM`) of an RFC 3339 timestamp.
const MONTH_LEN: usize = 7;

/// Per-record judgement buckets, as the oracle prints them; a negative step only appears when a
/// count went down (an I4 failure).
const JUDGEMENT_BUCKETS: [&str; 5] = ["negative", "0", "1", "2", "3+"];
const ZERO_POSITIONS: [&str; 3] = ["first", "middle", "last"];

const CLASS_MISMATCH: &str = "mismatch";
const CLASS_OTHER: &str = "other";
/// Spec 006 R3: ScoreV1 plays whose totals are split like ScoreV2 (`n_obj + n_ln`).
const CLASS_V1_SPLIT_LN: &str = "v1_split_ln";
/// Spec 006 R4: a total short of R3 is the saved-failed-play signature.
const CLASS_SHORT_TOTAL: &str = "short_total";
const CLASS_FINAL_ONLY: &str = "final_only";
const CLASS_NONZERO: &str = "nonzero";
const CLASS_FINAL_SHORT: &str = "final_short";
const CLASS_SCORE_ONLY: &str = "score_only";
const CLASS_TIME: &str = "time_decreases";
const CLASS_COUNT: &str = "count_decreases";
const CLASS_BOTH: &str = "both";
const CLASS_READ_FAILED: &str = "read_failed";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Inv {
    I1,
    I2,
    I3,
    I3B28,
    I3B4,
    I3B25,
    I4,
    I5,
    I6,
    I7,
}

const INVARIANTS: [Inv; 10] = [
    Inv::I1,
    Inv::I2,
    Inv::I3,
    Inv::I3B28,
    Inv::I3B4,
    Inv::I3B25,
    Inv::I4,
    Inv::I5,
    Inv::I6,
    Inv::I7,
];

impl Inv {
    const fn id(self) -> &'static str {
        match self {
            Self::I1 => "I1",
            Self::I2 => "I2",
            Self::I3 => "I3",
            Self::I3B28 => "I3_b28",
            Self::I3B4 => "I3_b4",
            Self::I3B25 => "I3_b25",
            Self::I4 => "I4",
            Self::I5 => "I5",
            Self::I6 => "I6",
            Self::I7 => "I7",
        }
    }

    /// The `I3_*` rows break I3 down per byte; they are not counted twice under `--strict`.
    const fn part_of(self) -> Option<&'static str> {
        match self {
            Self::I3B28 | Self::I3B4 | Self::I3B25 => Some("I3"),
            _ => None,
        }
    }

    /// Spec 006 Domain rules list the only exception classes `--strict` forgives.
    fn explains(self, class: &str) -> bool {
        self == Self::I6 && (class == CLASS_V1_SPLIT_LN || class == CLASS_SHORT_TOTAL)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Na,
    Pass,
    Fail(&'static str),
}

type Verdicts = [Verdict; INVARIANTS.len()];

fn set(v: &mut Verdicts, inv: Inv, verdict: Verdict) {
    v[inv as usize] = verdict;
}

fn check(ok: bool, class: &'static str) -> Verdict {
    if ok {
        Verdict::Pass
    } else {
        Verdict::Fail(class)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurveyOptions {
    /// Surveys only the first N `.osg` keys in `Data/r` order (with the `.osr`-only keys
    /// between them), so a quick run is a prefix of the full one.
    pub max_files: Option<usize>,
    pub examples: usize,
}

impl Default for SurveyOptions {
    fn default() -> Self {
        Self {
            max_files: None,
            examples: DEFAULT_EXAMPLES,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OsgSurveyReport {
    pub osg_files: u64,
    pub osr_files: u64,
    pub orphan_osg: u64,
    pub first_osg: Option<String>,
    pub sources: SurveySources,
    pub invariants: Vec<InvariantRow>,
    pub distributions: Distributions,
    pub decode_failures: Vec<FileFailure>,
    pub osr_failures: Vec<FileFailure>,
    pub i5_failures: Vec<I5Failure>,
    pub i6_failures: Vec<I6Failure>,
    pub osr_without_osg: MissingOsg,
}

impl OsgSurveyReport {
    /// Failures outside the listed exception classes, over the top-level invariants.
    pub fn unexplained_failures(&self) -> u64 {
        self.invariants
            .iter()
            .filter(|r| r.part_of.is_none())
            .map(|r| r.fail - r.explained)
            .sum()
    }

    /// The `--strict` verdict (spec 006 Behaviour).
    pub fn strict_passes(&self) -> bool {
        self.unexplained_failures() == 0
    }

    fn row_mut(&mut self, inv: Inv) -> &mut InvariantRow {
        &mut self.invariants[inv as usize]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurveySources {
    pub osu_db: SourceReport,
    pub scores_db: SourceReport,
}

/// A missing or unreadable DB is reported, not fatal: I6/I7 become `na`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceReport {
    /// `ok`, `missing` or `failed`.
    pub status: &'static str,
    pub entries: u64,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InvariantRow {
    pub id: &'static str,
    pub part_of: Option<&'static str>,
    pub pass: u64,
    pub fail: u64,
    pub na: u64,
    /// Failures in a listed exception class (R3 `v1_split_ln`, R4 short totals).
    pub explained: u64,
    pub classes: BTreeMap<String, u64>,
    pub examples: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Distributions {
    pub client_versions: BTreeMap<i32, u64>,
    pub strides: BTreeMap<String, u64>,
    pub modes: BTreeMap<String, u64>,
    pub client_version_equals_osr: BTreeMap<String, u64>,
    /// I8.
    pub judgements_per_record: BTreeMap<String, u64>,
    /// I8: where the 0-judgement records sit.
    pub zero_judgement_position: BTreeMap<String, u64>,
    pub records: u64,
    pub empty_graph: u64,
    pub nonzero_b25_records: u64,
    pub diagnostics: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileFailure {
    pub file: String,
    pub code: &'static str,
    pub detail: String,
}

/// Counts in `.osr` order (300, 100, 50, MAX, 200, miss), as the oracle prints them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct I5Failure {
    pub file: String,
    pub class: &'static str,
    pub v2: bool,
    pub mode: u8,
    pub mods: u32,
    pub counts_eq: bool,
    pub score_eq: bool,
    pub osg_counts: [u16; 6],
    pub osr_counts: [u16; 6],
    pub osg_score: i32,
    pub osr_score: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct I6Failure {
    pub file: String,
    pub class: &'static str,
    pub v2: bool,
    pub total: u32,
    pub expected: u32,
    pub n_obj: u32,
    pub n_ln: u32,
}

/// `.osr` files without an `.osg` (spec 006 Risks "Coverage").
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct MissingOsg {
    pub total: u64,
    pub since_first_osg: u64,
    pub by_month: BTreeMap<String, u64>,
    pub by_player: BTreeMap<String, u64>,
    pub by_version: BTreeMap<i32, u64>,
    pub by_mode: BTreeMap<u8, u64>,
    pub since_first_osg_by_player: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Copy)]
struct ChartInfo {
    mode: u8,
    n_obj: u32,
    n_ln: u32,
}

#[derive(Debug, Clone)]
struct OsrInfo {
    mode: u8,
    version: i32,
    player: String,
    counts: JudgementCounts,
    score: i32,
    mods: u32,
}

impl OsrInfo {
    fn from_header(h: &ScoreHeader) -> Self {
        Self {
            mode: h.mode,
            version: h.version,
            player: h
                .player
                .to_string_lossy()
                .map(|p| p.into_owned())
                .unwrap_or_default(),
            counts: h.counts,
            score: h.score,
            mods: h.mods,
        }
    }

    const fn is_v2(&self) -> bool {
        self.mods & wolluf_source_osu::codec::score_header::mods::SCORE_V2 != 0
    }
}

/// Everything one `.osg` contributes, so the records are dropped inside the worker: the pilot
/// corpus holds ~14 M of them.
#[derive(Debug)]
struct OsgSummary {
    file: String,
    verdicts: Verdicts,
    decoded: Option<DecodedStats>,
    failure: Option<FileFailure>,
    i5: Option<I5Failure>,
    i6: Option<I6Failure>,
}

#[derive(Debug)]
struct DecodedStats {
    client_version: i32,
    stride: Option<usize>,
    mode: Option<u8>,
    version_eq_osr: Option<bool>,
    records: u64,
    per_record: [u64; JUDGEMENT_BUCKETS.len()],
    zero_positions: [u64; ZERO_POSITIONS.len()],
    nonzero_b25: u64,
    diagnostics: Vec<&'static str>,
}

#[derive(Debug)]
struct KeyResult {
    filetime: FileTime,
    osr: Option<Result<OsrInfo, FileFailure>>,
    osg: Option<OsgSummary>,
}

type ChartIndex = BTreeMap<ChartMd5, ChartInfo>;
type ScoresIndex = BTreeMap<(ChartMd5, FileTime), JudgementCounts>;

struct Lookups {
    charts: Option<ChartIndex>,
    scores: Option<ScoresIndex>,
}

pub fn survey(
    root: &Path,
    opts: &SurveyOptions,
    cancel: &CancellationToken,
) -> Result<OsgSurveyReport, AppError> {
    if cancel.is_cancelled() {
        return Err(AppError::cancelled());
    }
    if !root.is_dir() {
        return Err(AppError::osu_dir_not_found(root.to_string_lossy()));
    }
    let index = replay_dir::index(root)?;
    let (charts, osu_db) = load_charts(root);
    let (scores, scores_db) = load_scores(root);
    let lookups = Lookups { charts, scores };
    let items = take_prefix(index, opts.max_files);
    let results: Vec<Option<KeyResult>> = items
        .par_iter()
        .map(|((md5, filetime), files)| {
            (!cancel.is_cancelled()).then(|| survey_key(*md5, *filetime, files, &lookups))
        })
        .collect();
    if cancel.is_cancelled() {
        return Err(AppError::cancelled());
    }
    let mut report = empty_report(SurveySources { osu_db, scores_db });
    aggregate(&mut report, results.into_iter().flatten(), opts.examples);
    Ok(report)
}

fn take_prefix(
    index: BTreeMap<(ChartMd5, FileTime), ReplayFiles>,
    max_files: Option<usize>,
) -> Vec<((ChartMd5, FileTime), ReplayFiles)> {
    let mut items = Vec::new();
    let mut osg = 0usize;
    for (key, files) in index {
        if max_files.is_some_and(|max| osg >= max) {
            break;
        }
        osg += usize::from(files.osg.is_some());
        items.push((key, files));
    }
    items
}

fn source_missing() -> SourceReport {
    SourceReport {
        status: "missing",
        entries: 0,
        detail: None,
    }
}

fn source_failed(detail: String) -> SourceReport {
    SourceReport {
        status: "failed",
        entries: 0,
        detail: Some(detail),
    }
}

fn source_ok(entries: usize) -> SourceReport {
    SourceReport {
        status: "ok",
        entries: entries as u64,
        detail: None,
    }
}

/// 003's stable read, so a DB osu! is writing is retried rather than half-read.
fn read_db(root: &Path, name: &str) -> Result<Vec<u8>, SourceReport> {
    let path = root_file(root, name);
    if !path.is_file() {
        return Err(source_missing());
    }
    read_stable(&path, &SnapshotPolicy::default())
        .map(|s| s.bytes)
        .map_err(|e| source_failed(format!("{}: {e}", e.code().as_str())))
}

/// Mania keeps LNs in the slider count (spec 006 H5); the first entry of a duplicated md5 wins,
/// as in the catalog.
fn load_charts(root: &Path) -> (Option<ChartIndex>, SourceReport) {
    let bytes = match read_db(root, OSU_DB) {
        Ok(bytes) => bytes,
        Err(report) => return (None, report),
    };
    match decode_osu_db(&bytes) {
        Ok((db, _)) => {
            let mut charts = BTreeMap::new();
            for b in &db.beatmaps {
                if let Some(md5) = b.beatmap_md5() {
                    charts.entry(md5).or_insert(ChartInfo {
                        mode: b.mode,
                        n_obj: u32::from(b.n_circles) + u32::from(b.n_sliders),
                        n_ln: u32::from(b.n_sliders),
                    });
                }
            }
            let report = source_ok(charts.len());
            (Some(charts), report)
        }
        Err(e) => (None, source_failed(format!("{e:?}"))),
    }
}

fn load_scores(root: &Path) -> (Option<ScoresIndex>, SourceReport) {
    let bytes = match read_db(root, SCORES_DB) {
        Ok(bytes) => bytes,
        Err(report) => return (None, report),
    };
    match decode_scores_db(&bytes) {
        Ok((db, _)) => {
            let mut scores = BTreeMap::new();
            for s in db.scores() {
                let h = &s.header;
                let key = chart_md5(h)
                    .ok()
                    .zip(DotNetTicks(h.timestamp_ticks).to_filetime());
                if let Some(key) = key {
                    scores.entry(key).or_insert(h.counts);
                }
            }
            let report = source_ok(scores.len());
            (Some(scores), report)
        }
        Err(e) => (None, source_failed(format!("{e:?}"))),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn io_failure(file: String, e: &io::Error) -> FileFailure {
    FileFailure {
        file,
        code: ErrorCode::Internal.as_str(),
        detail: e.to_string(),
    }
}

fn codec_failure(file: String, e: &CodecError) -> FileFailure {
    FileFailure {
        file,
        code: e.code().as_str(),
        detail: format!("{e:?}"),
    }
}

/// The variant name of a codec error (`StrideMismatch`), used as the I1 failure class.
fn variant(e: &CodecError) -> &'static str {
    match e {
        CodecError::UnsupportedFormat { .. } => "UnsupportedFormat",
        CodecError::Truncated { .. } => "Truncated",
        CodecError::BadStringTag { .. } => "BadStringTag",
        CodecError::UnexpectedTag { .. } => "UnexpectedTag",
        CodecError::UnexpectedValue { .. } => "UnexpectedValue",
        CodecError::InvalidCount { .. } => "InvalidCount",
        CodecError::Uleb128Overflow { .. } => "Uleb128Overflow",
        CodecError::EntrySizeMismatch { .. } => "EntrySizeMismatch",
        CodecError::StrideMismatch { .. } => "StrideMismatch",
        CodecError::TrailingBytes { .. } => "TrailingBytes",
    }
}

fn parse_osr_header(bytes: &[u8]) -> Result<ScoreHeader, CodecError> {
    read_score_header(&mut Reader::new(bytes, FileKind::Osr))
}

fn read_osr(path: &Path) -> Result<OsrInfo, FileFailure> {
    let name = file_name(path);
    let mut prefix = Vec::new();
    File::open(path)
        .and_then(|f| f.take(OSR_HEADER_PREFIX_BYTES).read_to_end(&mut prefix))
        .map_err(|e| io_failure(name.clone(), &e))?;
    let header = match parse_osr_header(&prefix) {
        Err(CodecError::Truncated { .. }) if prefix.len() as u64 == OSR_HEADER_PREFIX_BYTES => {
            let full = std::fs::read(path).map_err(|e| io_failure(name.clone(), &e))?;
            parse_osr_header(&full)
        }
        parsed => parsed,
    };
    header
        .map(|h| OsrInfo::from_header(&h))
        .map_err(|e| codec_failure(name, &e))
}

fn survey_key(
    md5: ChartMd5,
    filetime: FileTime,
    files: &ReplayFiles,
    lookups: &Lookups,
) -> KeyResult {
    let osr = files.osr.as_deref().map(read_osr);
    let osg = files.osg.as_deref().map(|path| {
        let header = osr.as_ref().and_then(|r| r.as_ref().ok());
        let chart = lookups.charts.as_ref().and_then(|c| c.get(&md5));
        let row = lookups
            .scores
            .as_ref()
            .and_then(|s| s.get(&(md5, filetime)));
        survey_osg(path, header, chart, row)
    });
    KeyResult { filetime, osr, osg }
}

fn failed_osg(file: String, class: &'static str, failure: FileFailure) -> OsgSummary {
    let mut verdicts = [Verdict::Na; INVARIANTS.len()];
    set(&mut verdicts, Inv::I1, Verdict::Fail(class));
    OsgSummary {
        file,
        verdicts,
        decoded: None,
        failure: Some(failure),
        i5: None,
        i6: None,
    }
}

fn survey_osg(
    path: &Path,
    osr: Option<&OsrInfo>,
    chart: Option<&ChartInfo>,
    scores_row: Option<&JudgementCounts>,
) -> OsgSummary {
    let file = file_name(path);
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) => {
            let failure = io_failure(file.clone(), &e);
            return failed_osg(file, CLASS_READ_FAILED, failure);
        }
    };
    match decode_osg(&bytes) {
        Ok((osg, diags)) => {
            let diagnostics = diags.iter().map(|d| d.code.as_str()).collect();
            evaluate(file, &osg, diagnostics, osr, chart, scores_row)
        }
        Err(e) => {
            let failure = codec_failure(file.clone(), &e);
            failed_osg(file, variant(&e), failure)
        }
    }
}

fn counts_array(c: &JudgementCounts) -> [u16; 6] {
    [c.n300, c.n100, c.n50, c.geki, c.katu, c.miss]
}

fn non_decreasing<T: PartialOrd>(mut values: impl Iterator<Item = T>) -> bool {
    let Some(mut prev) = values.next() else {
        return true;
    };
    for v in values {
        if v < prev {
            return false;
        }
        prev = v;
    }
    true
}

fn evaluate(
    file: String,
    osg: &OsgFile,
    diagnostics: Vec<&'static str>,
    osr: Option<&OsrInfo>,
    chart: Option<&ChartInfo>,
    scores_row: Option<&JudgementCounts>,
) -> OsgSummary {
    let mut v = [Verdict::Na; INVARIANTS.len()];
    set(&mut v, Inv::I1, Verdict::Pass);
    let stride = osg.score_system.map(|s| match s {
        wolluf_source_osu::codec::osg::OsgScoreSystem::V1 => STRIDE_V1,
        wolluf_source_osu::codec::osg::OsgScoreSystem::V2 => STRIDE_V2,
    });
    let is_v2_stride = stride == Some(STRIDE_V2);
    if let (Some(h), Some(_)) = (osr, stride) {
        set(
            &mut v,
            Inv::I2,
            check(is_v2_stride == h.is_v2(), CLASS_MISMATCH),
        );
    }
    let mut stats = DecodedStats {
        client_version: osg.client_version,
        stride,
        mode: osr.map(|h| h.mode),
        version_eq_osr: osr.map(|h| h.version == osg.client_version),
        records: osg.records.len() as u64,
        per_record: [0; JUDGEMENT_BUCKETS.len()],
        zero_positions: [0; ZERO_POSITIONS.len()],
        nonzero_b25: 0,
        diagnostics,
    };
    let mut summary = OsgSummary {
        file,
        verdicts: v,
        decoded: None,
        failure: None,
        i5: None,
        i6: None,
    };
    let Some(last) = osg.records.last() else {
        summary.decoded = Some(stats);
        return summary;
    };
    check_bytes(
        &mut summary.verdicts,
        &osg.records,
        is_v2_stride,
        &mut stats,
    );
    check_monotone(&mut summary.verdicts, &osg.records);
    judgement_steps(&osg.records, &mut stats);
    if let Some(h) = osr {
        let (verdict, failure) = check_final(&summary.file, last, h);
        set(&mut summary.verdicts, Inv::I5, verdict);
        summary.i5 = failure;
        if let Some(c) = chart.filter(|c| c.mode == MANIA_MODE && h.mode == MANIA_MODE) {
            let (verdict, failure) = check_totals(&summary.file, last, h.is_v2(), c);
            set(&mut summary.verdicts, Inv::I6, verdict);
            summary.i6 = failure;
        }
    }
    if let Some(row) = scores_row {
        set(
            &mut summary.verdicts,
            Inv::I7,
            check(last.counts == *row, CLASS_MISMATCH),
        );
    }
    summary.decoded = Some(stats);
    summary
}

/// I3: `b28` flags the stride, `b4` and `b25` are reserved (H1), except the final-record FC
/// flag (ADR 0012 item 7). `I3_b25` still records every nonzero `b25`: it is the FC-flag evidence
/// the corpus harness cross-checks against the `.osr` perfect byte.
fn check_bytes(v: &mut Verdicts, records: &[OsgRecord], v2: bool, stats: &mut DecodedStats) {
    let want_b28 = u8::from(v2);
    let ok_b28 = records.iter().all(|r| r.b28 == want_b28);
    let ok_b4 = records.iter().all(|r| r.b4 == 0);
    let b25: Vec<usize> = (0..records.len())
        .filter(|&i| records[i].b25 != 0)
        .collect();
    let b25_final_only = b25 == [records.len() - 1];
    stats.nonzero_b25 = b25.len() as u64;
    set(v, Inv::I3B28, check(ok_b28, CLASS_MISMATCH));
    set(v, Inv::I3B4, check(ok_b4, CLASS_NONZERO));
    let b25_class = if b25_final_only {
        CLASS_FINAL_ONLY
    } else {
        CLASS_OTHER
    };
    set(v, Inv::I3B25, check(b25.is_empty(), b25_class));
    let ok_b25 = b25.iter().all(|&i| is_fc_flag(records, i));
    set(v, Inv::I3, check(ok_b28 && ok_b4 && ok_b25, CLASS_OTHER));
}

/// I4: time and every cumulative count never go down.
fn check_monotone(v: &mut Verdicts, records: &[OsgRecord]) {
    let time_ok = non_decreasing(records.iter().map(|r| r.t_ms));
    let counts_ok =
        (0..6).all(|k| non_decreasing(records.iter().map(|r| counts_array(&r.counts)[k])));
    let class = match (time_ok, counts_ok) {
        (false, false) => CLASS_BOTH,
        (false, true) => CLASS_TIME,
        _ => CLASS_COUNT,
    };
    set(v, Inv::I4, check(time_ok && counts_ok, class));
}

/// I8: judgements added per record, and where the 0-judgement records sit.
fn judgement_steps(records: &[OsgRecord], stats: &mut DecodedStats) {
    let last = records.len() - 1;
    let mut prev = 0i64;
    for (i, r) in records.iter().enumerate() {
        let total = i64::from(r.counts.total());
        let step = total - prev;
        prev = total;
        let bucket = match step {
            s if s < 0 => 0,
            0 => 1,
            1 => 2,
            2 => 3,
            _ => 4,
        };
        stats.per_record[bucket] += 1;
        if step == 0 {
            let position = if i == 0 {
                0
            } else if i == last {
                2
            } else {
                1
            };
            stats.zero_positions[position] += 1;
        }
    }
}

/// I5 (H3): the last record against the `.osr` header.
fn check_final(file: &str, last: &OsgRecord, h: &OsrInfo) -> (Verdict, Option<I5Failure>) {
    let counts_eq = last.counts == h.counts;
    let score_eq = last.score == h.score;
    if counts_eq && score_eq {
        return (Verdict::Pass, None);
    }
    let (osg_counts, osr_counts) = (counts_array(&last.counts), counts_array(&h.counts));
    let class = if counts_eq {
        CLASS_SCORE_ONLY
    } else if osg_counts.iter().zip(osr_counts).all(|(o, r)| *o <= r) {
        CLASS_FINAL_SHORT
    } else {
        CLASS_OTHER
    };
    let failure = I5Failure {
        file: file.to_owned(),
        class,
        v2: h.is_v2(),
        mode: h.mode,
        mods: h.mods,
        counts_eq,
        score_eq,
        osg_counts,
        osr_counts,
        osg_score: last.score,
        osr_score: h.score,
    };
    (Verdict::Fail(class), Some(failure))
}

/// I6 (H5, R3): final total against osu!.db object counts.
fn check_totals(
    file: &str,
    last: &OsgRecord,
    v2: bool,
    c: &ChartInfo,
) -> (Verdict, Option<I6Failure>) {
    let total = last.counts.total();
    let expected = if v2 { c.n_obj + c.n_ln } else { c.n_obj };
    if total == expected {
        return (Verdict::Pass, None);
    }
    let class = if total < expected {
        CLASS_SHORT_TOTAL
    } else if !v2 && c.n_ln > 0 && total == c.n_obj + c.n_ln {
        CLASS_V1_SPLIT_LN
    } else {
        CLASS_MISMATCH
    };
    let failure = I6Failure {
        file: file.to_owned(),
        class,
        v2,
        total,
        expected,
        n_obj: c.n_obj,
        n_ln: c.n_ln,
    };
    (Verdict::Fail(class), Some(failure))
}

fn empty_report(sources: SurveySources) -> OsgSurveyReport {
    OsgSurveyReport {
        osg_files: 0,
        osr_files: 0,
        orphan_osg: 0,
        first_osg: None,
        sources,
        invariants: INVARIANTS
            .iter()
            .map(|inv| InvariantRow {
                id: inv.id(),
                part_of: inv.part_of(),
                pass: 0,
                fail: 0,
                na: 0,
                explained: 0,
                classes: BTreeMap::new(),
                examples: Vec::new(),
            })
            .collect(),
        distributions: Distributions::default(),
        decode_failures: Vec::new(),
        osr_failures: Vec::new(),
        i5_failures: Vec::new(),
        i6_failures: Vec::new(),
        osr_without_osg: MissingOsg::default(),
    }
}

fn bump<K: Ord>(map: &mut BTreeMap<K, u64>, key: K) {
    *map.entry(key).or_default() += 1;
}

fn add(map: &mut BTreeMap<String, u64>, labels: &[&str], counts: &[u64]) {
    for (label, &n) in labels.iter().zip(counts) {
        if n > 0 {
            *map.entry((*label).to_owned()).or_default() += n;
        }
    }
}

fn month(filetime: FileTime) -> String {
    filetime.to_dotnet_ticks().map_or_else(
        || UNKNOWN.to_owned(),
        |t| format_rfc3339_ms(t.to_unix_us())[..MONTH_LEN].to_owned(),
    )
}

/// Sequential and in key order, so the report is identical whatever rayon's scheduling (D3).
fn aggregate(
    report: &mut OsgSurveyReport,
    results: impl Iterator<Item = KeyResult>,
    examples: usize,
) {
    let mut missing: Vec<(FileTime, OsrInfo)> = Vec::new();
    let mut first_osg: Option<FileTime> = None;
    for r in results {
        match r.osr {
            Some(Ok(info)) => {
                report.osr_files += 1;
                if r.osg.is_none() {
                    missing.push((r.filetime, info));
                }
            }
            Some(Err(failure)) => {
                report.osr_files += 1;
                report.osr_failures.push(failure);
            }
            None if r.osg.is_some() => report.orphan_osg += 1,
            None => {}
        }
        if let Some(osg) = r.osg {
            report.osg_files += 1;
            first_osg = Some(first_osg.map_or(r.filetime, |f| f.min(r.filetime)));
            add_osg(report, osg, examples);
        }
    }
    report.first_osg = first_osg
        .and_then(FileTime::to_dotnet_ticks)
        .map(|t| format_rfc3339_ms(t.to_unix_us()));
    let m = &mut report.osr_without_osg;
    for (filetime, info) in missing {
        m.total += 1;
        bump(&mut m.by_month, month(filetime));
        bump(&mut m.by_player, info.player.clone());
        bump(&mut m.by_version, info.version);
        bump(&mut m.by_mode, info.mode);
        if first_osg.is_some_and(|first| filetime >= first) {
            m.since_first_osg += 1;
            bump(&mut m.since_first_osg_by_player, info.player);
        }
    }
}

fn add_osg(report: &mut OsgSurveyReport, osg: OsgSummary, examples: usize) {
    for inv in INVARIANTS {
        let row = report.row_mut(inv);
        match osg.verdicts[inv as usize] {
            Verdict::Na => row.na += 1,
            Verdict::Pass => row.pass += 1,
            Verdict::Fail(class) => {
                row.fail += 1;
                if inv.explains(class) {
                    row.explained += 1;
                }
                bump(&mut row.classes, class.to_owned());
                if row.examples.len() < examples {
                    row.examples.push(osg.file.clone());
                }
            }
        }
    }
    report.decode_failures.extend(osg.failure);
    report.i5_failures.extend(osg.i5);
    report.i6_failures.extend(osg.i6);
    let Some(s) = osg.decoded else { return };
    let d = &mut report.distributions;
    bump(&mut d.client_versions, s.client_version);
    bump(
        &mut d.strides,
        s.stride
            .map_or_else(|| EMPTY_STRIDE.to_owned(), |x| x.to_string()),
    );
    bump(
        &mut d.modes,
        s.mode.map_or_else(|| UNKNOWN.to_owned(), |m| m.to_string()),
    );
    if let Some(eq) = s.version_eq_osr {
        bump(&mut d.client_version_equals_osr, eq.to_string());
    }
    add(
        &mut d.judgements_per_record,
        &JUDGEMENT_BUCKETS,
        &s.per_record,
    );
    add(
        &mut d.zero_judgement_position,
        &ZERO_POSITIONS,
        &s.zero_positions,
    );
    d.records += s.records;
    d.empty_graph += u64::from(s.stride.is_none());
    d.nonzero_b25_records += s.nonzero_b25;
    for code in s.diagnostics {
        bump(&mut d.diagnostics, code.to_owned());
    }
}

fn kv<K: Display>(map: &BTreeMap<K, u64>) -> String {
    let parts: Vec<String> = map.iter().map(|(k, v)| format!("{k}={v}")).collect();
    if parts.is_empty() {
        "-".to_owned()
    } else {
        parts.join(" ")
    }
}

fn source_line(name: &str, s: &SourceReport) -> String {
    match &s.detail {
        Some(detail) => format!("{name} {} ({detail})", s.status),
        None => format!("{name} {} ({} entries)", s.status, s.entries),
    }
}

/// Plain-text report for the terminal; `--json` serialises the report itself.
pub fn render_survey(report: &OsgSurveyReport) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "osg {}  osr {}  orphan_osg {}  first_osg {}",
        report.osg_files,
        report.osr_files,
        report.orphan_osg,
        report.first_osg.as_deref().unwrap_or("-"),
    );
    let _ = writeln!(
        out,
        "sources: {}; {}",
        source_line("osu!.db", &report.sources.osu_db),
        source_line("scores.db", &report.sources.scores_db),
    );
    let _ = writeln!(
        out,
        "invariant     pass   fail     na  explained  classes  examples"
    );
    for r in &report.invariants {
        let _ = writeln!(
            out,
            "{:<9} {:>8} {:>6} {:>6} {:>10}  {}  {}",
            r.id,
            r.pass,
            r.fail,
            r.na,
            r.explained,
            kv(&r.classes),
            if r.examples.is_empty() {
                "-".to_owned()
            } else {
                r.examples.join(" ")
            },
        );
    }
    let d = &report.distributions;
    let _ = writeln!(
        out,
        "I8 judgements_per_record: {}; zero_judgement_position: {}",
        kv(&d.judgements_per_record),
        kv(&d.zero_judgement_position),
    );
    let _ = writeln!(out, "client_versions: {}", kv(&d.client_versions));
    let _ = writeln!(out, "strides: {}", kv(&d.strides));
    let _ = writeln!(out, "modes: {}", kv(&d.modes));
    let _ = writeln!(
        out,
        "client_version_equals_osr: {}",
        kv(&d.client_version_equals_osr)
    );
    let _ = writeln!(
        out,
        "records: {}  empty_graph: {}  nonzero_b25_records: {}",
        d.records, d.empty_graph, d.nonzero_b25_records
    );
    let _ = writeln!(out, "diagnostics: {}", kv(&d.diagnostics));
    for (title, failures) in [
        ("decode_failures", &report.decode_failures),
        ("osr_failures", &report.osr_failures),
    ] {
        let _ = writeln!(out, "{title} ({}):", failures.len());
        for f in failures {
            let _ = writeln!(out, "  {} {} {}", f.file, f.code, f.detail);
        }
    }
    let _ = writeln!(out, "i5_failures ({}):", report.i5_failures.len());
    for f in &report.i5_failures {
        let _ = writeln!(
            out,
            "  {} class={} v2={} mode={} mods={} counts_eq={} score_eq={} osg={:?}/{} osr={:?}/{}",
            f.file,
            f.class,
            f.v2,
            f.mode,
            f.mods,
            f.counts_eq,
            f.score_eq,
            f.osg_counts,
            f.osg_score,
            f.osr_counts,
            f.osr_score,
        );
    }
    let _ = writeln!(out, "i6_failures ({}):", report.i6_failures.len());
    for f in &report.i6_failures {
        let _ = writeln!(
            out,
            "  {} class={} v2={} total={} expected={} n_obj={} n_ln={}",
            f.file, f.class, f.v2, f.total, f.expected, f.n_obj, f.n_ln,
        );
    }
    let m = &report.osr_without_osg;
    let _ = writeln!(
        out,
        "osr_without_osg: total {}  since_first_osg {}",
        m.total, m.since_first_osg
    );
    let _ = writeln!(out, "  by_month: {}", kv(&m.by_month));
    let _ = writeln!(out, "  by_player: {}", kv(&m.by_player));
    let _ = writeln!(out, "  by_version: {}", kv(&m.by_version));
    let _ = writeln!(out, "  by_mode: {}", kv(&m.by_mode));
    let _ = writeln!(
        out,
        "  since_first_osg_by_player: {}",
        kv(&m.since_first_osg_by_player)
    );
    let _ = writeln!(
        out,
        "strict: {} ({} unexplained failures)",
        if report.strict_passes() {
            "pass"
        } else {
            "fail"
        },
        report.unexplained_failures(),
    );
    out
}
