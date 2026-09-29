//! Players DTOs (spec 004 IPC). camelCase on the wire, snake_case enum values, ids as `u32`
//! (the bindings export fails on 64-bit integers, spec 005), timestamps as RFC 3339 strings.
//! Mapped from the domain types here, so shells stay mapping-free (D13).

use serde::{Deserialize, Serialize};
use wolluf_core::{AliasId, Keymode, ProfileId, UnixUs};
use wolluf_store::time::format_rfc3339_ms;

use super::identity::{AliasList, AliasRow, EntryKind, EntryRef, ProfileEntry, TopChart};
use super::names::SessionMatchKind;
use super::scope::{MergeMode, ResolvedScope};
use super::selection::{AutoMatch, Decision, MatchSource};
use crate::errors::{AppError, players_keys as keys};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum DecisionDto {
    Me,
    NotMe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum MatchSourceDto {
    CfgUsername,
    LinkedAccount,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum MatchKindDto {
    Equal,
    Prefix,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum MergeModeDto {
    Merged,
    Separate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKindDto {
    #[serde(rename = "self")]
    SelfProfile,
    Other,
    AllPlayers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct AutoMatchDto {
    pub source: MatchSourceDto,
    pub kind: MatchKindDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct KeymodeCountDto {
    /// `k1`..`k16` or `unknown` (`KeymodeBucket`).
    pub bucket: String,
    pub n: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct TopChartDto {
    pub chart_md5: String,
    pub title: Option<String>,
    pub version: Option<String>,
    pub n: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AliasRowDto {
    pub alias_id: u32,
    /// Lossy UTF-8 for display only; `aliasId` is the key the UI sends back.
    pub raw_name: String,
    pub is_empty_name: bool,
    pub normalized_length: u32,
    pub n_plays: u32,
    pub by_keymode: Vec<KeymodeCountDto>,
    pub first_played_at: Option<String>,
    pub last_played_at: Option<String>,
    pub n_online: u32,
    pub n_offline: u32,
    pub n_with_replay: u32,
    pub top_charts: Vec<TopChartDto>,
    pub auto_match: Option<AutoMatchDto>,
    pub decision: Option<DecisionDto>,
    pub selected: bool,
    pub in_self_profile: bool,
}

/// Rows arrive in the R6 order; the UI does not re-sort them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AliasListDto {
    pub selection_version: u32,
    pub cfg_username_available: bool,
    pub wizard_needed: bool,
    pub aliases: Vec<AliasRowDto>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EntryRefDto {
    Profile { id: u32 },
    AllPlayers,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ScopeDto {
    /// 64 hex chars (`ScopeHash`).
    pub scope_hash: String,
    pub alias_ids: Vec<u32>,
    pub keymode: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ProfileEntryDto {
    #[serde(rename = "ref")]
    pub entry: EntryRefDto,
    pub profile_kind: ProfileKindDto,
    /// Empty for All players: the UI shows its i18n label.
    pub label: String,
    pub is_default: bool,
    pub merge_mode: MergeModeDto,
    pub alias_ids: Vec<u32>,
    pub scopes: Vec<ScopeDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AliasDecisionInput {
    pub alias_id: u32,
    /// `null` clears the decision, handing the alias back to the auto rule (R6).
    pub decision: Option<DecisionDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DecideAliasInput {
    pub decisions: Vec<AliasDecisionInput>,
    pub completes_wizard: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CreateProfileInput {
    pub label: String,
    pub alias_ids: Vec<u32>,
    #[specta(optional)]
    pub merge_mode: Option<MergeModeDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SetProfileAliasesInput {
    pub profile_id: u32,
    pub alias_ids: Vec<u32>,
    #[specta(optional)]
    pub merge_mode: Option<MergeModeDto>,
}

impl From<Decision> for DecisionDto {
    fn from(d: Decision) -> Self {
        match d {
            Decision::Me => Self::Me,
            Decision::NotMe => Self::NotMe,
        }
    }
}

impl From<DecisionDto> for Decision {
    fn from(d: DecisionDto) -> Self {
        match d {
            DecisionDto::Me => Self::Me,
            DecisionDto::NotMe => Self::NotMe,
        }
    }
}

impl From<MatchSource> for MatchSourceDto {
    fn from(s: MatchSource) -> Self {
        match s {
            MatchSource::CfgUsername => Self::CfgUsername,
            MatchSource::LinkedAccount => Self::LinkedAccount,
        }
    }
}

impl From<SessionMatchKind> for MatchKindDto {
    fn from(k: SessionMatchKind) -> Self {
        match k {
            SessionMatchKind::Equal => Self::Equal,
            SessionMatchKind::Prefix => Self::Prefix,
        }
    }
}

impl From<AutoMatch> for AutoMatchDto {
    fn from(m: AutoMatch) -> Self {
        Self {
            source: m.source.into(),
            kind: m.kind.into(),
        }
    }
}

impl From<MergeMode> for MergeModeDto {
    fn from(m: MergeMode) -> Self {
        match m {
            MergeMode::Merged => Self::Merged,
            MergeMode::Separate => Self::Separate,
        }
    }
}

impl From<MergeModeDto> for MergeMode {
    fn from(m: MergeModeDto) -> Self {
        match m {
            MergeModeDto::Merged => Self::Merged,
            MergeModeDto::Separate => Self::Separate,
        }
    }
}

impl From<EntryKind> for ProfileKindDto {
    fn from(k: EntryKind) -> Self {
        match k {
            EntryKind::SelfProfile => Self::SelfProfile,
            EntryKind::Other => Self::Other,
            EntryKind::AllPlayers => Self::AllPlayers,
        }
    }
}

/// Row ids are SQLite `i64`s that stay tiny in practice; one past `u32` is a broken invariant,
/// not user input, hence `INTERNAL` (spec 004 IPC).
fn wire_id(kind: &str, id: i64) -> Result<u32, AppError> {
    u32::try_from(id).map_err(|_| AppError::internal(format!("{kind} id {id} does not fit u32")))
}

fn alias_ids(ids: &[AliasId]) -> Result<Vec<u32>, AppError> {
    ids.iter().map(|id| wire_id("alias", id.0)).collect()
}

fn timestamp(t: Option<UnixUs>) -> Option<String> {
    t.map(format_rfc3339_ms)
}

impl From<TopChart> for TopChartDto {
    fn from(c: TopChart) -> Self {
        Self {
            chart_md5: c.chart_md5.to_string(),
            title: c.title,
            version: c.version,
            n: c.n,
        }
    }
}

impl TryFrom<AliasRow> for AliasRowDto {
    type Error = AppError;

    fn try_from(row: AliasRow) -> Result<Self, AppError> {
        let s = row.stats;
        Ok(Self {
            alias_id: wire_id("alias", row.alias_id.0)?,
            raw_name: String::from_utf8_lossy(&row.raw_name).into_owned(),
            is_empty_name: row.raw_name.is_empty(),
            normalized_length: row.norm_len,
            n_plays: s.n_plays,
            by_keymode: s
                .by_keymode
                .iter()
                .map(|(bucket, n)| KeymodeCountDto {
                    bucket: bucket.to_string(),
                    n: *n,
                })
                .collect(),
            first_played_at: timestamp(s.first_played_at),
            last_played_at: timestamp(s.last_played_at),
            n_online: s.n_online,
            n_offline: s.n_plays.saturating_sub(s.n_online),
            n_with_replay: s.n_with_replay,
            top_charts: row.top_charts.into_iter().map(TopChartDto::from).collect(),
            auto_match: row.auto_match.map(AutoMatchDto::from),
            decision: row.decision.map(DecisionDto::from),
            selected: row.selected,
            in_self_profile: row.in_self_profile,
        })
    }
}

impl TryFrom<AliasList> for AliasListDto {
    type Error = AppError;

    fn try_from(list: AliasList) -> Result<Self, AppError> {
        Ok(Self {
            selection_version: list.selection_version,
            cfg_username_available: list.cfg_username_available,
            wizard_needed: list.wizard_needed,
            aliases: list
                .rows
                .into_iter()
                .map(AliasRowDto::try_from)
                .collect::<Result<_, _>>()?,
        })
    }
}

impl TryFrom<ResolvedScope> for ScopeDto {
    type Error = AppError;

    fn try_from(scope: ResolvedScope) -> Result<Self, AppError> {
        Ok(Self {
            scope_hash: scope.hash.to_string(),
            alias_ids: alias_ids(&scope.alias_ids)?,
            keymode: scope.keymode.columns(),
        })
    }
}

impl TryFrom<ProfileEntry> for ProfileEntryDto {
    type Error = AppError;

    fn try_from(entry: ProfileEntry) -> Result<Self, AppError> {
        Ok(Self {
            entry: match entry.entry {
                EntryRef::Profile(id) => EntryRefDto::Profile {
                    id: wire_id("profile", id.0)?,
                },
                EntryRef::AllPlayers => EntryRefDto::AllPlayers,
            },
            profile_kind: entry.kind.into(),
            label: entry.label,
            is_default: entry.is_default,
            merge_mode: entry.merge_mode.into(),
            alias_ids: alias_ids(&entry.alias_ids)?,
            scopes: entry
                .scopes
                .into_iter()
                .map(ScopeDto::try_from)
                .collect::<Result<_, _>>()?,
        })
    }
}

/// For command results: the whole list or the first id that cannot cross the wire.
pub fn profile_entries(entries: Vec<ProfileEntry>) -> Result<Vec<ProfileEntryDto>, AppError> {
    entries.into_iter().map(ProfileEntryDto::try_from).collect()
}

pub fn profile_id(id: u32) -> ProfileId {
    ProfileId(i64::from(id))
}

fn alias_ids_in(ids: Vec<u32>) -> Vec<AliasId> {
    ids.into_iter().map(|id| AliasId(i64::from(id))).collect()
}

/// A structured `INVALID_INPUT` instead of a deserialization failure, so the UI can localise it.
pub fn keymode(columns: u32) -> Result<Keymode, AppError> {
    u8::try_from(columns)
        .ok()
        .and_then(|c| Keymode::new(c).ok())
        .ok_or_else(|| {
            AppError::invalid_input()
                .with_key(keys::INVALID_KEYMODE)
                .with_arg("keymode", columns.to_string())
        })
}

impl DecideAliasInput {
    /// The batch for `PlayersService::decide`, plus `completes_wizard`.
    pub fn into_domain(self) -> (Vec<(AliasId, Option<Decision>)>, bool) {
        let batch = self
            .decisions
            .into_iter()
            .map(|d| {
                (
                    AliasId(i64::from(d.alias_id)),
                    d.decision.map(Decision::from),
                )
            })
            .collect();
        (batch, self.completes_wizard)
    }
}

impl CreateProfileInput {
    pub fn into_domain(self) -> (String, Vec<AliasId>, Option<MergeMode>) {
        (
            self.label,
            alias_ids_in(self.alias_ids),
            self.merge_mode.map(MergeMode::from),
        )
    }
}

impl SetProfileAliasesInput {
    pub fn into_domain(self) -> (ProfileId, Vec<AliasId>, Option<MergeMode>) {
        (
            profile_id(self.profile_id),
            alias_ids_in(self.alias_ids),
            self.merge_mode.map(MergeMode::from),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::collections::BTreeMap;

    use wolluf_core::{ChartMd5, ErrorCode, ScopeHash};

    use super::*;
    use crate::features::players::stats::{AliasStats, KeymodeBucket};

    const MD5_A: &str = "0123456789abcdef0123456789abcdef";
    const MD5_B: &str = "fedcba9876543210fedcba9876543210";
    /// 2026-04-25T00:00:00Z and one hour later.
    const T_FIRST: UnixUs = UnixUs(1_777_075_200_000_000);
    const T_LAST: UnixUs = UnixUs(1_777_078_800_000_000);

    fn md5(hex: &str) -> ChartMd5 {
        hex.parse().unwrap()
    }

    fn stats(alias: i64, n_plays: u32, n_online: u32) -> AliasStats {
        AliasStats {
            alias_id: AliasId(alias),
            n_plays,
            by_keymode: vec![
                (KeymodeBucket::Keys(Keymode::K7), n_plays.saturating_sub(1)),
                (KeymodeBucket::Unknown, 1),
            ],
            first_played_at: Some(T_FIRST),
            last_played_at: Some(T_LAST),
            n_online,
            n_with_replay: n_plays,
            top_charts: vec![(md5(MD5_A), 2)],
        }
    }

    fn row(alias: i64, raw: &[u8]) -> AliasRow {
        AliasRow {
            alias_id: AliasId(alias),
            raw_name: raw.to_vec(),
            norm_len: 6,
            stats: stats(alias, 3, 1),
            top_charts: vec![TopChart {
                chart_md5: md5(MD5_A),
                title: Some("Title".to_owned()),
                version: Some("7K Hard".to_owned()),
                n: 2,
            }],
            auto_match: Some(AutoMatch {
                source: MatchSource::CfgUsername,
                kind: SessionMatchKind::Prefix,
            }),
            decision: None,
            selected: true,
            in_self_profile: true,
        }
    }

    fn empty_row(alias: i64) -> AliasRow {
        AliasRow {
            alias_id: AliasId(alias),
            raw_name: Vec::new(),
            norm_len: 0,
            stats: AliasStats {
                alias_id: AliasId(alias),
                n_plays: 0,
                by_keymode: Vec::new(),
                first_played_at: None,
                last_played_at: None,
                n_online: 0,
                n_with_replay: 0,
                top_charts: Vec::new(),
            },
            top_charts: vec![TopChart {
                chart_md5: md5(MD5_B),
                title: None,
                version: None,
                n: 1,
            }],
            auto_match: None,
            decision: Some(Decision::NotMe),
            selected: false,
            in_self_profile: false,
        }
    }

    fn scope(byte: u8, ids: &[i64]) -> ResolvedScope {
        ResolvedScope {
            hash: ScopeHash([byte; 32]),
            alias_ids: ids.iter().copied().map(AliasId).collect(),
            keymode: Keymode::K7,
            label_args: BTreeMap::new(),
        }
    }

    fn entries() -> Vec<ProfileEntry> {
        vec![
            ProfileEntry {
                entry: EntryRef::Profile(ProfileId(1)),
                kind: EntryKind::SelfProfile,
                label: "Me".to_owned(),
                is_default: true,
                merge_mode: MergeMode::Merged,
                alias_ids: vec![AliasId(1)],
                scopes: vec![scope(0xab, &[1])],
            },
            ProfileEntry {
                entry: EntryRef::Profile(ProfileId(2)),
                kind: EntryKind::Other,
                label: "Rosalind".to_owned(),
                is_default: false,
                merge_mode: MergeMode::Separate,
                alias_ids: vec![AliasId(3), AliasId(4)],
                scopes: vec![scope(0x01, &[3]), scope(0x02, &[4])],
            },
            ProfileEntry {
                entry: EntryRef::AllPlayers,
                kind: EntryKind::AllPlayers,
                label: String::new(),
                is_default: false,
                merge_mode: MergeMode::Merged,
                alias_ids: vec![AliasId(1), AliasId(2), AliasId(3), AliasId(4)],
                scopes: vec![scope(0xff, &[1, 2, 3, 4])],
            },
        ]
    }

    /// Leaves the graph as the macros built it: enough to prove no field needs a BigInt, which
    /// the exporter checks per primitive whatever the serde renames.
    struct Unrenamed;

    impl specta::Format for Unrenamed {
        fn map_types(
            &'_ self,
            types: &specta::Types,
        ) -> Result<Cow<'_, specta::Types>, specta::FormatError> {
            Ok(Cow::Owned(types.clone()))
        }

        fn map_type(
            &'_ self,
            _types: &specta::Types,
            dt: &specta::datatype::DataType,
        ) -> Result<Cow<'_, specta::datatype::DataType>, specta::FormatError> {
            Ok(Cow::Owned(dt.clone()))
        }
    }

    #[test]
    fn dto_shape() {
        let types = specta::Types::default()
            .register::<AliasListDto>()
            .register::<ProfileEntryDto>()
            .register::<DecideAliasInput>()
            .register::<CreateProfileInput>()
            .register::<SetProfileAliasesInput>();
        let ts = specta_typescript::Typescript::default()
            .export(&types, Unrenamed)
            .unwrap();
        assert!(!ts.contains("bigint"), "{ts}");

        let list = AliasList {
            selection_version: 1,
            cfg_username_available: true,
            wizard_needed: true,
            rows: vec![row(1, b"TWulfZ"), empty_row(2)],
        };
        let list = AliasListDto::try_from(list).unwrap();
        let profiles = entries()
            .into_iter()
            .map(ProfileEntryDto::try_from)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let wire = serde_json::json!({ "aliasList": list, "profiles": profiles });
        assert_eq!(
            wire["aliasList"]["aliases"][0]["autoMatch"],
            serde_json::json!({ "source": "cfg_username", "kind": "prefix" })
        );
        assert_eq!(
            wire["aliasList"]["aliases"][1]["autoMatch"],
            serde_json::Value::Null
        );
        insta::assert_snapshot!(
            "players_dto_wire",
            serde_json::to_string_pretty(&wire).unwrap()
        );
    }

    #[test]
    fn ids_beyond_u32_are_internal() {
        let too_big = i64::from(u32::MAX) + 1;
        let list = AliasList {
            selection_version: 1,
            cfg_username_available: false,
            wizard_needed: false,
            rows: vec![row(too_big, b"x")],
        };
        let err = AliasListDto::try_from(list).unwrap_err();
        assert_eq!(err.code, ErrorCode::Internal);

        let mut entry = entries().remove(0);
        entry.entry = EntryRef::Profile(ProfileId(-1));
        let err = ProfileEntryDto::try_from(entry).unwrap_err();
        assert_eq!(err.code, ErrorCode::Internal);
    }

    #[test]
    fn inputs_map_to_domain() {
        let decide: DecideAliasInput = serde_json::from_str(
            r#"{"decisions":[{"aliasId":3,"decision":"me"},{"aliasId":4,"decision":"not_me"},{"aliasId":5,"decision":null}],"completesWizard":true}"#,
        )
        .unwrap();
        let (batch, completes) = decide.into_domain();
        assert!(completes);
        assert_eq!(
            batch,
            vec![
                (AliasId(3), Some(Decision::Me)),
                (AliasId(4), Some(Decision::NotMe)),
                (AliasId(5), None),
            ]
        );

        let create: CreateProfileInput =
            serde_json::from_str(r#"{"label":"Rosalind","aliasIds":[7,8]}"#).unwrap();
        assert_eq!(
            create.into_domain(),
            ("Rosalind".to_owned(), vec![AliasId(7), AliasId(8)], None)
        );

        let set: SetProfileAliasesInput =
            serde_json::from_str(r#"{"profileId":2,"aliasIds":[7],"mergeMode":"separate"}"#)
                .unwrap();
        assert_eq!(
            set.into_domain(),
            (ProfileId(2), vec![AliasId(7)], Some(MergeMode::Separate))
        );
        assert_eq!(profile_id(9), ProfileId(9));
    }

    #[test]
    fn keymode_input_is_validated() {
        assert_eq!(keymode(7).unwrap(), Keymode::K7);
        for bad in [0, 17, 300] {
            let err = keymode(bad).unwrap_err();
            assert_eq!(err.code, ErrorCode::InvalidInput, "{bad}");
            assert_eq!(err.message_key, keys::INVALID_KEYMODE, "{bad}");
            assert_eq!(err.args["keymode"], bad.to_string());
        }
    }
}
