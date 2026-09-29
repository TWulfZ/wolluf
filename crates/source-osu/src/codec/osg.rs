//! `.osg` decoder (spec 006 H0/H1, oracle `research/scripts/osg/osg.py`). Lenient on purpose:
//! the layout is still a spike hypothesis, so unexplained bytes are kept raw and oddities are
//! warnings; only a structurally impossible file is an error (§7).

pub mod events;

use crate::codec::score_header::JudgementCounts;
use crate::codec::{FileKind, Reader};
use crate::diag::{DiagCode, Diagnostics};
use crate::error::CodecError;

const KIND: FileKind = FileKind::Osg;
/// Builds seen on the pilot (spec 006 Risks). A data table, not a threshold: an unknown build is
/// only a warning because the stride check (I1) is the real format gate (R7).
pub const KNOWN_CLIENT_VERSIONS: [i32; 7] = [
    20_260_312, 20_260_412, 20_260_612, 20_260_622, 20_260_624, 20_260_711, 20_260_924,
];
const HEADER_LEN: usize = 4 + 4;
/// ScoreV1 record: t_ms, b4, 6 counts, score, max combo, combo, b25, hp_raw, b28.
pub const STRIDE_V1: usize = 4 + 1 + 6 * 2 + 4 + 2 + 2 + 1 + 2 + 1;
/// ScoreV2 appends two f64 (`f0`, `f1`).
pub const STRIDE_V2: usize = STRIDE_V1 + 2 * 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsgScoreSystem {
    V1,
    V2,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OsgFile {
    /// The client build (seven values on the pilot, each equal to the paired `.osr` version).
    pub client_version: i32,
    /// From the stride; `None` for a file with no records.
    pub score_system: Option<OsgScoreSystem>,
    pub records: Vec<OsgRecord>,
}

/// One record per score-changing update (H1). `b4`, `b25` and `b28` are kept raw; only the
/// final-record `b25` is interpreted (`is_fc_flag`).
#[derive(Debug, Clone, PartialEq)]
pub struct OsgRecord {
    pub t_ms: i32,
    /// Cumulative, in `.osr` order (300, 100, 50, MAX, 200, miss).
    pub counts: JudgementCounts,
    pub score: i32,
    pub max_combo: u16,
    pub combo: u16,
    pub hp_raw: u16,
    pub b4: u8,
    pub b25: u8,
    pub b28: u8,
    /// `f0`, `f1`; present exactly in 45-byte records.
    pub v2: Option<[f64; 2]>,
}

/// ADR 0012 item 7: `b25 = 1` on the final record is stable's full-combo flag. It is not an FC
/// oracle (one pilot FC lacks it); FC status comes from the `.osr` header.
pub const FC_FLAG: u8 = 1;

pub fn is_fc_flag(records: &[OsgRecord], idx: usize) -> bool {
    idx + 1 == records.len() && records[idx].b25 == FC_FLAG
}

pub fn decode_osg(bytes: &[u8]) -> Result<(OsgFile, Diagnostics), CodecError> {
    let mut r = Reader::new(bytes, KIND);
    let client_version = r.i32()?;
    let count_at = r.offset();
    let raw_count = r.i32()?;
    let count = usize::try_from(raw_count).map_err(|_| CodecError::InvalidCount {
        kind: KIND,
        offset: count_at,
        count: i64::from(raw_count),
    })?;
    let stride = stride_for(bytes.len() - HEADER_LEN, count).ok_or(CodecError::StrideMismatch {
        len: bytes.len() as u64,
        count: raw_count,
    })?;
    let mut diags = Diagnostics::new();
    let score_system = stride.map(|s| {
        if s == STRIDE_V2 {
            OsgScoreSystem::V2
        } else {
            OsgScoreSystem::V1
        }
    });
    let mut records = Vec::with_capacity(count);
    for _ in 0..count {
        records.push(read_record(
            &mut r,
            score_system == Some(OsgScoreSystem::V2),
        )?);
    }
    r.expect_eof()?;
    if !KNOWN_CLIENT_VERSIONS.contains(&client_version) {
        diags.general(
            DiagCode::OsgUnknownClientVersion,
            format!("client version {client_version}"),
        );
    }
    match stride {
        Some(stride) => check_records(&records, stride, &mut diags),
        None => diags.general(DiagCode::OsgEmptyGraph, "0 records"),
    }
    let osg = OsgFile {
        client_version,
        score_system,
        records,
    };
    Ok((osg, diags))
}

/// H0 is the format gate: the body must split into `count` records of exactly one known
/// stride. `Some(None)` is the valid empty file.
fn stride_for(body: usize, count: usize) -> Option<Option<usize>> {
    if count == 0 {
        return (body == 0).then_some(None);
    }
    [STRIDE_V1, STRIDE_V2]
        .into_iter()
        .find(|&s| count.checked_mul(s) == Some(body))
        .map(Some)
}

/// Tallies one warning kind across a file so a corpus of millions of records yields one
/// diagnostic per kind per file, pointing at the first offender.
#[derive(Default)]
struct Tally {
    first: Option<usize>,
    count: usize,
}

impl Tally {
    fn hit(&mut self, idx: usize) {
        self.first.get_or_insert(idx);
        self.count += 1;
    }

    fn report(&self, diags: &mut Diagnostics, code: DiagCode, stride: usize) {
        if let Some(first) = self.first {
            diags.at_offset(
                code,
                (HEADER_LEN + first * stride) as u64,
                format!("first record {first}, {} records", self.count),
            );
        }
    }
}

fn counts_array(c: &JudgementCounts) -> [u16; 6] {
    [c.n300, c.n100, c.n50, c.geki, c.katu, c.miss]
}

/// H1/I3/I4 checks as warnings: the spike reports them, it does not reject files for them.
fn check_records(records: &[OsgRecord], stride: usize, diags: &mut Diagnostics) {
    let v2_flag = u8::from(stride == STRIDE_V2);
    let (mut flag, mut reserved, mut time, mut counts) = (
        Tally::default(),
        Tally::default(),
        Tally::default(),
        Tally::default(),
    );
    for (idx, r) in records.iter().enumerate() {
        if r.b28 != v2_flag {
            flag.hit(idx);
        }
        if r.b4 != 0 || (r.b25 != 0 && !is_fc_flag(records, idx)) {
            reserved.hit(idx);
        }
        if let Some(prev) = idx.checked_sub(1).and_then(|p| records.get(p)) {
            if r.t_ms < prev.t_ms {
                time.hit(idx);
            }
            let (now, before) = (counts_array(&r.counts), counts_array(&prev.counts));
            if now.iter().zip(before).any(|(n, b)| *n < b) {
                counts.hit(idx);
            }
        }
    }
    flag.report(diags, DiagCode::OsgFlagStrideDisagree, stride);
    reserved.report(diags, DiagCode::OsgNonzeroReserved, stride);
    time.report(diags, DiagCode::OsgTimeDecreases, stride);
    counts.report(diags, DiagCode::OsgCountDecreases, stride);
}

fn read_record(r: &mut Reader<'_>, v2: bool) -> Result<OsgRecord, CodecError> {
    let t_ms = r.i32()?;
    let b4 = r.u8()?;
    let counts = JudgementCounts {
        n300: r.u16()?,
        n100: r.u16()?,
        n50: r.u16()?,
        geki: r.u16()?,
        katu: r.u16()?,
        miss: r.u16()?,
    };
    Ok(OsgRecord {
        t_ms,
        b4,
        counts,
        score: r.i32()?,
        max_combo: r.u16()?,
        combo: r.u16()?,
        b25: r.u8()?,
        hp_raw: r.u16()?,
        b28: r.u8()?,
        v2: if v2 { Some([r.f64()?, r.f64()?]) } else { None },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::DiagCode;
    use crate::error::CodecError;
    use crate::testkit::encode_osg;

    const V: i32 = 20_260_924;

    fn record(t_ms: i32, n300: u16, v2: Option<[f64; 2]>) -> OsgRecord {
        OsgRecord {
            t_ms,
            counts: JudgementCounts {
                n300,
                ..JudgementCounts::default()
            },
            score: i32::from(n300) * 300,
            max_combo: n300,
            combo: n300,
            hp_raw: 200,
            b4: 0,
            b25: 0,
            b28: u8::from(v2.is_some()),
            v2,
        }
    }

    fn file(records: Vec<OsgRecord>) -> OsgFile {
        let score_system = records.first().map(|r| {
            if r.v2.is_some() {
                OsgScoreSystem::V2
            } else {
                OsgScoreSystem::V1
            }
        });
        OsgFile {
            client_version: V,
            score_system,
            records,
        }
    }

    #[test]
    fn decodes_v1_record() {
        let mut bytes = Vec::new();
        bytes.extend(V.to_le_bytes());
        bytes.extend(1_i32.to_le_bytes());
        bytes.extend(1_234_i32.to_le_bytes());
        bytes.push(0);
        for c in [1u16, 2, 3, 4, 5, 6] {
            bytes.extend(c.to_le_bytes());
        }
        bytes.extend(98_765_i32.to_le_bytes());
        bytes.extend(40_u16.to_le_bytes());
        bytes.extend(12_u16.to_le_bytes());
        bytes.push(0);
        bytes.extend(187_u16.to_le_bytes());
        bytes.push(0);
        assert_eq!(bytes.len(), 8 + STRIDE_V1);
        let (osg, diags) = decode_osg(&bytes).unwrap();
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(osg.client_version, V);
        assert_eq!(osg.score_system, Some(OsgScoreSystem::V1));
        let r = &osg.records[0];
        assert_eq!(r.t_ms, 1_234);
        assert_eq!(
            r.counts,
            JudgementCounts {
                n300: 1,
                n100: 2,
                n50: 3,
                geki: 4,
                katu: 5,
                miss: 6
            }
        );
        assert_eq!(
            (r.score, r.max_combo, r.combo, r.hp_raw),
            (98_765, 40, 12, 187)
        );
        assert_eq!(r.v2, None);
    }

    #[test]
    fn decodes_v2_record() {
        let osg = file(vec![
            record(10, 1, Some([150.0, 0.0])),
            record(20, 2, Some([300.0, 0.0])),
        ]);
        let bytes = encode_osg(&osg);
        assert_eq!(bytes.len(), 8 + 2 * STRIDE_V2);
        let (decoded, diags) = decode_osg(&bytes).unwrap();
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(decoded, osg);
        assert_eq!(decoded.score_system, Some(OsgScoreSystem::V2));
        assert_eq!(decoded.records[1].v2, Some([300.0, 0.0]));
    }

    #[test]
    fn keeps_reserved_bytes_raw() {
        let mut r = record(5, 1, None);
        r.b4 = 0xaa;
        r.b25 = 1;
        r.b28 = 0x7f;
        let (decoded, _) = decode_osg(&encode_osg(&file(vec![r.clone()]))).unwrap();
        assert_eq!(decoded.records[0], r);
    }

    #[test]
    fn empty_graph_warns() {
        let bytes = encode_osg(&file(vec![]));
        assert_eq!(bytes.len(), 8);
        let (osg, diags) = decode_osg(&bytes).unwrap();
        assert!(osg.records.is_empty());
        assert_eq!(osg.score_system, None);
        assert!(diags.contains(DiagCode::OsgEmptyGraph));
    }

    #[test]
    fn unknown_client_version_is_warning() {
        let mut osg = file(vec![record(1, 1, None)]);
        osg.client_version = 20_270_101;
        let (decoded, diags) = decode_osg(&encode_osg(&osg)).unwrap();
        assert_eq!(decoded, osg);
        assert_eq!(diags.codes(), vec![DiagCode::OsgUnknownClientVersion]);
        for known in KNOWN_CLIENT_VERSIONS {
            osg.client_version = known;
            assert!(
                decode_osg(&encode_osg(&osg)).unwrap().1.is_empty(),
                "{known}"
            );
        }
    }

    #[test]
    fn flag_stride_disagree_is_warning() {
        let mut a = record(1, 1, None);
        a.b28 = 1;
        let mut b = record(2, 2, None);
        b.b28 = 1;
        let (_, diags) = decode_osg(&encode_osg(&file(vec![a, b]))).unwrap();
        // One diagnostic per kind per file, pointing at the first offending record.
        assert_eq!(diags.codes(), vec![DiagCode::OsgFlagStrideDisagree]);
        assert_eq!(diags.as_slice()[0].offset, Some(8));
        assert!(diags.as_slice()[0].detail.contains("2 records"));
        let mut v2 = record(1, 1, Some([0.0, 0.0]));
        v2.b28 = 0;
        let (_, diags) = decode_osg(&encode_osg(&file(vec![v2]))).unwrap();
        assert_eq!(diags.codes(), vec![DiagCode::OsgFlagStrideDisagree]);
        let mut reserved = record(1, 1, None);
        reserved.b4 = 3;
        let (_, diags) = decode_osg(&encode_osg(&file(vec![reserved]))).unwrap();
        assert_eq!(diags.codes(), vec![DiagCode::OsgNonzeroReserved]);
    }

    #[test]
    fn final_record_b25_is_fc_flag_not_reserved() {
        let decode = |records: Vec<OsgRecord>| decode_osg(&encode_osg(&file(records))).unwrap().1;
        let flagged = |mut r: OsgRecord, b25: u8| {
            r.b25 = b25;
            r
        };
        let silent = [
            vec![record(1, 1, None), flagged(record(2, 2, None), 1)],
            vec![flagged(record(1, 1, None), 1)],
            vec![
                record(1, 1, Some([0.0, 0.0])),
                flagged(record(2, 2, Some([0.0, 0.0])), 1),
            ],
        ];
        for records in silent {
            let diags = decode(records);
            assert!(diags.is_empty(), "{diags:?}");
        }

        let diags = decode(vec![flagged(record(1, 1, None), 1), record(2, 2, None)]);
        assert_eq!(diags.codes(), vec![DiagCode::OsgNonzeroReserved]);
        assert_eq!(diags.as_slice()[0].offset, Some(8));
        // Only the value 1 is the flag; any other final-record value stays unexplained.
        let diags = decode(vec![record(1, 1, None), flagged(record(2, 2, None), 2)]);
        assert_eq!(diags.codes(), vec![DiagCode::OsgNonzeroReserved]);
        let mut b4_on_final = flagged(record(2, 2, None), 1);
        b4_on_final.b4 = 1;
        let diags = decode(vec![record(1, 1, None), b4_on_final]);
        assert_eq!(diags.codes(), vec![DiagCode::OsgNonzeroReserved]);
        assert_eq!(diags.as_slice()[0].offset, Some((8 + STRIDE_V1) as u64));
    }

    #[test]
    fn time_decrease_is_warning() {
        let (osg, diags) = decode_osg(&encode_osg(&file(vec![
            record(10, 1, None),
            record(9, 2, None),
        ])))
        .unwrap();
        assert_eq!(osg.records.len(), 2);
        assert_eq!(diags.codes(), vec![DiagCode::OsgTimeDecreases]);
        assert_eq!(diags.as_slice()[0].offset, Some((8 + STRIDE_V1) as u64));
        let (_, diags) = decode_osg(&encode_osg(&file(vec![
            record(1, 2, None),
            record(2, 1, None),
        ])))
        .unwrap();
        assert_eq!(diags.codes(), vec![DiagCode::OsgCountDecreases]);
    }

    #[test]
    fn rejects_truncated_header() {
        assert_eq!(
            decode_osg(&[1, 2, 3, 4, 5]),
            Err(CodecError::Truncated {
                kind: FileKind::Osg,
                offset: 4,
                needed: 4
            })
        );
    }

    #[test]
    fn rejects_negative_count() {
        let mut bytes = V.to_le_bytes().to_vec();
        bytes.extend((-1_i32).to_le_bytes());
        assert_eq!(
            decode_osg(&bytes),
            Err(CodecError::InvalidCount {
                kind: FileKind::Osg,
                offset: 4,
                count: -1
            })
        );
    }

    #[test]
    fn rejects_stride_mismatch() {
        let mut bytes = encode_osg(&file(vec![record(1, 1, None), record(2, 2, None)]));
        bytes.pop();
        assert_eq!(
            decode_osg(&bytes),
            Err(CodecError::StrideMismatch {
                len: bytes.len() as u64,
                count: 2
            })
        );
        let mut empty_with_body = encode_osg(&file(vec![]));
        empty_with_body.extend([0; STRIDE_V1]);
        assert!(matches!(
            decode_osg(&empty_with_body),
            Err(CodecError::StrideMismatch { count: 0, .. })
        ));
        // 37 bytes per record divides evenly but is neither layout.
        let mut odd = V.to_le_bytes().to_vec();
        odd.extend(1_i32.to_le_bytes());
        odd.extend([0; 37]);
        assert!(matches!(
            decode_osg(&odd),
            Err(CodecError::StrideMismatch { count: 1, .. })
        ));
    }
}

#[cfg(test)]
mod props {
    use proptest::prelude::*;

    use super::*;
    use crate::testkit::encode_osg;

    fn record(v2: bool) -> impl Strategy<Value = OsgRecord> {
        let counts = any::<[u16; 6]>().prop_map(|c| JudgementCounts {
            n300: c[0],
            n100: c[1],
            n50: c[2],
            geki: c[3],
            katu: c[4],
            miss: c[5],
        });
        let floats = any::<[f64; 2]>().prop_filter("finite", |f| f.iter().all(|x| x.is_finite()));
        (
            any::<i32>(),
            counts,
            any::<i32>(),
            any::<[u16; 3]>(),
            any::<[u8; 3]>(),
            floats,
        )
            .prop_map(
                move |(t_ms, counts, score, [max_combo, combo, hp_raw], [b4, b25, b28], f)| {
                    OsgRecord {
                        t_ms,
                        counts,
                        score,
                        max_combo,
                        combo,
                        hp_raw,
                        b4,
                        b25,
                        b28,
                        v2: v2.then_some(f),
                    }
                },
            )
    }

    fn osg_file() -> impl Strategy<Value = OsgFile> {
        (any::<bool>(), any::<i32>()).prop_flat_map(|(v2, client_version)| {
            proptest::collection::vec(record(v2), 0..8).prop_map(move |records| OsgFile {
                client_version,
                score_system: (!records.is_empty()).then_some(if v2 {
                    OsgScoreSystem::V2
                } else {
                    OsgScoreSystem::V1
                }),
                records,
            })
        })
    }

    proptest! {
        #[test]
        fn roundtrip_encode_decode(osg in osg_file()) {
            let bytes = encode_osg(&osg);
            let (decoded, _) = decode_osg(&bytes).unwrap();
            prop_assert_eq!(&decoded, &osg);
            prop_assert_eq!(encode_osg(&decoded), bytes);
        }

        #[test]
        fn decode_never_panics_on_arbitrary_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
            let _ = decode_osg(&bytes);
        }
    }
}
