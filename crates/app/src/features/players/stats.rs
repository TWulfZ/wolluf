//! Per-alias statistics for the identity table (§5.6 Listing, spec 004 R11). A versioned stage:
//! the cached rows carry the vkey from [`stats_vkey`].

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::fmt;

use wolluf_core::{
    AliasId, BlobSha256, ChartMd5, CoreError, Keymode, PlayId, StageId, UnixUs, VersionKey,
    VersionKeyBuilder,
};

use super::IdentityParams;

pub const ALIAS_STATS_STAGE: StageId = StageId::from_static("players.alias_stats");
/// Bump when the same plays would aggregate differently.
pub const ALIAS_STATS_VERSION: u32 = 1;

const CONFIG_TAG: &[u8] = b"wolluf.players.alias_stats.config.v1";
const INPUT_TAG: &[u8] = b"wolluf.players.alias_stats.input.v1";

/// Stable strings `k1`..`k16` and `unknown` (chart not in the catalog). The derived order
/// (keymodes by columns, then unknown) is the canonical row order. There is no non-mania bucket:
/// sync never ingests mode != 3 plays (spec 003 step 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum KeymodeBucket {
    Keys(Keymode),
    Unknown,
}

impl KeymodeBucket {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "unknown" => Some(Self::Unknown),
            _ => {
                let digits = s.strip_prefix('k')?;
                // One spelling per value: `k07` or `k+7` would alias `k7`.
                if digits.starts_with('0') || !digits.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                Keymode::new(digits.parse().ok()?).ok().map(Self::Keys)
            }
        }
    }
}

impl fmt::Display for KeymodeBucket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Keys(keymode) => write!(f, "k{}", keymode.columns()),
            Self::Unknown => f.write_str("unknown"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayFact {
    pub play_id: PlayId,
    pub alias_id: AliasId,
    pub played_at: UnixUs,
    pub chart_md5: ChartMd5,
    pub keymode: KeymodeBucket,
    pub has_online_id: bool,
    pub has_replay: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasStats {
    pub alias_id: AliasId,
    pub n_plays: u32,
    pub by_keymode: Vec<(KeymodeBucket, u32)>,
    pub first_played_at: Option<UnixUs>,
    pub last_played_at: Option<UnixUs>,
    pub n_online: u32,
    pub n_with_replay: u32,
    pub top_charts: Vec<(ChartMd5, u32)>,
}

#[derive(Default)]
struct Acc {
    n_plays: u32,
    by_keymode: BTreeMap<KeymodeBucket, u32>,
    first: Option<UnixUs>,
    last: Option<UnixUs>,
    n_online: u32,
    n_with_replay: u32,
    charts: BTreeMap<ChartMd5, u32>,
}

/// One row per listed alias, sorted by alias id, including aliases without plays. Plays of
/// unlisted aliases are ignored. `BTreeMap` only, so the output never depends on hash order (D3).
pub fn compute(
    aliases: &[AliasId],
    plays: &[PlayFact],
    params: &IdentityParams,
) -> Vec<AliasStats> {
    let mut accs: BTreeMap<AliasId, Acc> = aliases.iter().map(|id| (*id, Acc::default())).collect();
    for play in plays {
        let Some(acc) = accs.get_mut(&play.alias_id) else {
            continue;
        };
        acc.n_plays = acc.n_plays.saturating_add(1);
        bump(acc.by_keymode.entry(play.keymode).or_default());
        acc.first = Some(acc.first.map_or(play.played_at, |t| t.min(play.played_at)));
        acc.last = Some(acc.last.map_or(play.played_at, |t| t.max(play.played_at)));
        acc.n_online = acc.n_online.saturating_add(u32::from(play.has_online_id));
        acc.n_with_replay = acc.n_with_replay.saturating_add(u32::from(play.has_replay));
        bump(acc.charts.entry(play.chart_md5).or_default());
    }

    let top_n = usize::try_from(params.top_charts_n).unwrap_or(usize::MAX);
    accs.into_iter()
        .map(|(alias_id, acc)| {
            let mut top_charts: Vec<(ChartMd5, u32)> = acc.charts.into_iter().collect();
            top_charts.sort_by_key(|(md5, n)| (Reverse(*n), *md5));
            top_charts.truncate(top_n);
            AliasStats {
                alias_id,
                n_plays: acc.n_plays,
                by_keymode: acc.by_keymode.into_iter().collect(),
                first_played_at: acc.first,
                last_played_at: acc.last,
                n_online: acc.n_online,
                n_with_replay: acc.n_with_replay,
                top_charts,
            }
        })
        .collect()
}

fn bump(count: &mut u32) {
    *count = count.saturating_add(1);
}

/// Config = the params that change the aggregation (`top_charts_n`; `min_norm_len` only
/// affects selection). Input = the play set plus the osu!.db snapshot, which decides the
/// keymode buckets through the catalog (spec 004 Design).
pub fn stats_vkey(
    params: &IdentityParams,
    play_ids: &[PlayId],
    osu_db_snapshot: Option<BlobSha256>,
) -> Result<VersionKey, CoreError> {
    let mut config = blake3::Hasher::new();
    config.update(CONFIG_TAG);
    config.update(&params.top_charts_n.to_le_bytes());

    let mut ids: Vec<PlayId> = play_ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    let mut input = blake3::Hasher::new();
    input.update(INPUT_TAG);
    input.update(&u64::try_from(ids.len()).unwrap_or(u64::MAX).to_le_bytes());
    for id in &ids {
        input.update(&id.0);
    }
    match osu_db_snapshot {
        Some(sha) => input.update(&[1]).update(&sha.0),
        None => input.update(&[0]),
    };

    VersionKeyBuilder::new(ALIAS_STATS_STAGE, ALIAS_STATS_VERSION)
        .config(*config.finalize().as_bytes())
        .input(*input.finalize().as_bytes())
        .finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn md5(byte: u8) -> ChartMd5 {
        ChartMd5([byte; 16])
    }

    fn play(n: u8, alias: i64, at: i64, chart: u8, keymode: KeymodeBucket) -> PlayFact {
        PlayFact {
            play_id: PlayId([n; 32]),
            alias_id: AliasId(alias),
            played_at: UnixUs(at),
            chart_md5: md5(chart),
            keymode,
            has_online_id: false,
            has_replay: false,
        }
    }

    fn keys(n: u8) -> KeymodeBucket {
        KeymodeBucket::Keys(Keymode::new(n).unwrap())
    }

    #[test]
    fn aggregates_table() {
        let online = |mut p: PlayFact| {
            p.has_online_id = true;
            p
        };
        let replay = |mut p: PlayFact| {
            p.has_replay = true;
            p
        };
        let plays = vec![
            online(replay(play(1, 1, 500, 0xbb, keys(7)))),
            replay(play(2, 1, 100, 0xaa, keys(7))),
            online(play(3, 1, 900, 0xbb, keys(7))),
            replay(play(4, 1, 300, 0xaa, keys(4))),
            replay(play(5, 1, 700, 0xdd, KeymodeBucket::Unknown)),
            play(6, 1, 200, 0xcc, KeymodeBucket::Unknown),
            // Not a listed alias: ignored, never an extra row.
            play(7, 99, 50, 0xaa, keys(7)),
        ];
        let params = IdentityParams {
            top_charts_n: 3,
            ..IdentityParams::default()
        };
        let stats = compute(&[AliasId(2), AliasId(1)], &plays, &params);

        assert_eq!(
            stats,
            vec![
                AliasStats {
                    alias_id: AliasId(1),
                    n_plays: 6,
                    by_keymode: vec![(keys(4), 1), (keys(7), 3), (KeymodeBucket::Unknown, 2)],
                    first_played_at: Some(UnixUs(100)),
                    last_played_at: Some(UnixUs(900)),
                    n_online: 2,
                    n_with_replay: 4,
                    // aa and bb tie at 2 and cc and dd at 1: ties break by md5 ascending.
                    top_charts: vec![(md5(0xaa), 2), (md5(0xbb), 2), (md5(0xcc), 1)],
                },
                AliasStats {
                    alias_id: AliasId(2),
                    n_plays: 0,
                    by_keymode: vec![],
                    first_played_at: None,
                    last_played_at: None,
                    n_online: 0,
                    n_with_replay: 0,
                    top_charts: vec![],
                },
            ]
        );
    }

    #[test]
    fn keymode_bucket_strings() {
        let all: Vec<String> = (1..=16)
            .map(keys)
            .chain([KeymodeBucket::Unknown])
            .map(|b| b.to_string())
            .collect();
        assert_eq!(all.first().map(String::as_str), Some("k1"));
        assert_eq!(all[6], "k7");
        assert_eq!(all[15], "k16");
        assert_eq!(&all[16..], ["unknown"]);
        for text in &all {
            assert_eq!(
                KeymodeBucket::parse(text).map(|b| b.to_string()).as_ref(),
                Some(text)
            );
        }
        for bad in [
            "k0",
            "k17",
            "K7",
            "k07",
            "k+7",
            "k",
            "",
            "7",
            "k256",
            "non_mania",
        ] {
            assert_eq!(KeymodeBucket::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn stats_vkey_changes_with_inputs() {
        let params = IdentityParams::default();
        let ids = [PlayId([1; 32]), PlayId([2; 32])];
        let sha = Some(BlobSha256([9; 32]));
        let base = stats_vkey(&params, &ids, sha).unwrap();

        let reversed = [ids[1], ids[0]];
        assert_eq!(stats_vkey(&params, &reversed, sha).unwrap(), base);
        let selection_only = IdentityParams {
            min_norm_len: 7,
            ..params
        };
        assert_eq!(stats_vkey(&selection_only, &ids, sha).unwrap(), base);

        let more_charts = IdentityParams {
            top_charts_n: 6,
            ..params
        };
        assert_ne!(stats_vkey(&more_charts, &ids, sha).unwrap(), base);
        assert_ne!(stats_vkey(&params, &ids[..1], sha).unwrap(), base);
        assert_ne!(stats_vkey(&params, &ids, None).unwrap(), base);
        assert_ne!(
            stats_vkey(&params, &ids, Some(BlobSha256([8; 32]))).unwrap(),
            base
        );
        assert_ne!(
            stats_vkey(&params, &[], None).unwrap(),
            stats_vkey(&params, &[], Some(BlobSha256([0; 32]))).unwrap()
        );
    }
}
