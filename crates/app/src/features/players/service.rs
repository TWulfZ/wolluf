//! `PlayersService` (spec 004): the players feature's only entry point for shells and other
//! features (D12), plus the `RefreshIdentity` job chained after every `SyncPlays`.

use wolluf_core::{AliasId, Keymode, ProfileId};
use wolluf_store::repo::players::{profile, profile_alias};

use super::IdentityParams;
use super::identity::{AliasList, DecisionSource, EntryRef, Identity, ProfileEntry};
use super::scope::{MergeMode, ResolvedScope};
use super::selection::Decision;
use crate::context::{AppContext, blocking_join_error};
use crate::errors::AppError;
use crate::events::AppEvent;
use crate::jobs::{Job, JobCtx, JobFuture, JobKindDto, JobSummary};

const DOMAIN_PLAYERS: &str = "players";
/// Spec 004 IPC: create and edit answers carry scopes; 7K is the MVP keymode.
const ENTRY_KEYMODE: Keymode = Keymode::K7;

pub struct PlayersService<'a> {
    ctx: &'a AppContext,
    identity: Identity,
}

impl<'a> PlayersService<'a> {
    pub fn new(ctx: &'a AppContext) -> Self {
        Self {
            ctx,
            identity: Identity {
                user: ctx.user_db().clone(),
                cache: ctx.cache_db().clone(),
                clock: ctx.clock().clone(),
                params: IdentityParams::default(),
            },
        }
    }

    async fn blocking<R: Send + 'static>(
        &self,
        f: impl FnOnce(Identity) -> Result<R, AppError> + Send + 'static,
    ) -> Result<R, AppError> {
        let identity = self.identity.clone();
        tokio::task::spawn_blocking(move || f(identity))
            .await
            .map_err(blocking_join_error)?
    }

    fn players_changed(&self) {
        self.ctx.emit(AppEvent::data_changed(&[DOMAIN_PLAYERS]));
    }

    pub async fn list_aliases(&self) -> Result<AliasList, AppError> {
        self.blocking(|i| i.list_aliases()).await
    }

    /// Cheap (a settings read and a count): 005's `setup_status.identityReady` is its negation.
    pub async fn wizard_needed(&self) -> Result<bool, AppError> {
        self.blocking(|i| i.wizard_needed()).await
    }

    /// Normally run by the chained `RefreshIdentity` job; callable directly for tests and the
    /// CLI. Emits `DataChanged{players}` only when something changed.
    pub async fn refresh(&self) -> Result<bool, AppError> {
        let changed = self.blocking(|i| i.refresh()).await?;
        if changed {
            self.players_changed();
        }
        Ok(changed)
    }

    /// `completes_wizard` also marks the wizard done (spec 004 Behaviour 4).
    pub async fn decide(
        &self,
        batch: Vec<(AliasId, Option<Decision>)>,
        completes_wizard: bool,
    ) -> Result<AliasList, AppError> {
        let source = if completes_wizard {
            DecisionSource::Wizard
        } else {
            DecisionSource::Settings
        };
        self.blocking(move |i| i.decide(batch, completes_wizard, source))
            .await?;
        self.players_changed();
        self.list_aliases().await
    }

    pub async fn create_profile(
        &self,
        label: String,
        alias_ids: Vec<AliasId>,
        merge_mode: Option<MergeMode>,
    ) -> Result<ProfileEntry, AppError> {
        let id = self
            .blocking(move |i| i.create_profile(&label, alias_ids, merge_mode))
            .await?;
        self.players_changed();
        self.entry(EntryRef::Profile(id)).await
    }

    pub async fn set_profile_aliases(
        &self,
        profile_id: ProfileId,
        alias_ids: Vec<AliasId>,
        merge_mode: Option<MergeMode>,
    ) -> Result<ProfileEntry, AppError> {
        self.blocking(move |i| i.set_profile_aliases(profile_id, alias_ids, merge_mode))
            .await?;
        self.players_changed();
        self.entry(EntryRef::Profile(profile_id)).await
    }

    pub async fn set_default(&self, profile_id: ProfileId) -> Result<(), AppError> {
        self.blocking(move |i| i.set_default(profile_id)).await?;
        self.players_changed();
        Ok(())
    }

    /// The self profile's id without resolving any scope; `None` before the first identity
    /// refresh.
    pub async fn self_profile_id(&self) -> Result<Option<ProfileId>, AppError> {
        self.blocking(|i| Ok(i.user.read(profile::self_profile)?.map(|p| p.id)))
            .await
    }

    /// The aliases in the self profile, ascending; empty without one.
    pub async fn self_alias_ids(&self) -> Result<Vec<AliasId>, AppError> {
        self.blocking(|i| {
            Ok(i.user.read(|c| {
                let Some(me) = profile::self_profile(c)? else {
                    return Ok(Vec::new());
                };
                Ok(profile_alias::list(c, me.id)?
                    .into_iter()
                    .map(|r| r.alias_id)
                    .collect())
            })?)
        })
        .await
    }

    pub async fn list_profiles(&self, keymode: Keymode) -> Result<Vec<ProfileEntry>, AppError> {
        self.blocking(move |i| i.list_profiles(keymode)).await
    }

    pub async fn resolve_scopes(
        &self,
        entry: EntryRef,
        keymode: Keymode,
        merge: Option<MergeMode>,
    ) -> Result<Vec<ResolvedScope>, AppError> {
        self.blocking(move |i| i.resolve_scopes(entry, keymode, merge))
            .await
    }

    async fn entry(&self, entry: EntryRef) -> Result<ProfileEntry, AppError> {
        self.list_profiles(ENTRY_KEYMODE)
            .await?
            .into_iter()
            .find(|e| e.entry == entry)
            .ok_or_else(|| AppError::internal("profile vanished after its write"))
    }
}

/// Stats, self-profile bootstrap and auto reconcile after a sync (spec 004 Behaviour 1).
#[derive(Debug, Default)]
pub struct RefreshIdentityJob;

impl RefreshIdentityJob {
    pub const DEDUPE_KEY: &'static str = "players.refresh";
}

impl Job for RefreshIdentityJob {
    fn kind(&self) -> JobKindDto {
        JobKindDto::RefreshIdentity
    }

    fn dedupe_key(&self) -> String {
        Self::DEDUPE_KEY.to_owned()
    }

    fn params(&self) -> serde_json::Value {
        serde_json::json!({})
    }

    fn run(self: Box<Self>, ctx: JobCtx) -> JobFuture {
        Box::pin(async move {
            let identity = Identity {
                user: ctx.user.clone(),
                cache: ctx.cache.clone(),
                clock: ctx.clock.clone(),
                params: IdentityParams::default(),
            };
            let changed = tokio::task::spawn_blocking(move || identity.refresh())
                .await
                .map_err(blocking_join_error)??;
            Ok(JobSummary {
                summary: None,
                changed: if changed {
                    vec![DOMAIN_PLAYERS]
                } else {
                    Vec::new()
                },
                follow_ups: Vec::new(),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use wolluf_core::ErrorCode;
    use wolluf_source_osu::testkit::FakeInstall;
    use wolluf_store::repo::ledger::feedback_event;
    use wolluf_store::repo::players::{
        self as repo, AliasOrigin, NewProfile, ProfileKind, profile, profile_alias,
    };

    use super::*;
    use crate::features::players::identity::EntryKind;
    use crate::features::players::names::SessionMatchKind;
    use crate::features::players::selection::{AutoMatch, MatchSource};
    use crate::features::players::testkit::{
        PILOT_ALIASES, PILOT_CFG_USERNAME, pilot_cfg, pilot_cfg_path, pilot_install, pilot_scores,
    };
    use crate::features::plays::testkit::{Fixture, scores_db};

    const TWULFZ: &str = "TWulfZ";

    /// Syncs (which chains `RefreshIdentity`) and waits for the refresh too.
    async fn pilot(cfg: Option<&str>) -> Fixture {
        let f = Fixture::new(&pilot_install(cfg)).await;
        f.sync().await;
        f
    }

    fn ids(f: &Fixture) -> BTreeMap<String, AliasId> {
        f.ctx
            .user_db()
            .read(wolluf_store::repo::ledger::alias::list)
            .unwrap()
            .into_iter()
            .map(|a| (String::from_utf8(a.raw_name).unwrap(), a.id))
            .collect()
    }

    fn id(f: &Fixture, name: &str) -> AliasId {
        ids(f)[name]
    }

    fn self_rows(f: &Fixture) -> BTreeMap<String, AliasOrigin> {
        let names: BTreeMap<AliasId, String> = ids(f).into_iter().map(|(n, i)| (i, n)).collect();
        let me = f
            .ctx
            .user_db()
            .read(profile::self_profile)
            .unwrap()
            .unwrap();
        f.ctx
            .user_db()
            .read(|c| profile_alias::list(c, me.id))
            .unwrap()
            .into_iter()
            .map(|r| (names[&r.alias_id].clone(), r.origin))
            .collect()
    }

    fn auto(names: &[&str]) -> BTreeMap<String, AliasOrigin> {
        names
            .iter()
            .map(|n| ((*n).to_owned(), AliasOrigin::Auto))
            .collect()
    }

    fn feedback(f: &Fixture) -> Vec<wolluf_store::repo::ledger::FeedbackEvent> {
        f.ctx.user_db().read(feedback_event::list).unwrap()
    }

    fn set_cfg(f: &Fixture, username: &str) {
        f.write(pilot_cfg_path(), &pilot_cfg(username));
    }

    fn self_hash(entries: &[ProfileEntry]) -> Option<wolluf_core::ScopeHash> {
        entries
            .iter()
            .find(|e| e.kind == EntryKind::SelfProfile)
            .and_then(|e| e.scopes.first())
            .map(|s| s.hash)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn self_profile_id_and_aliases_are_the_stored_self() {
        let empty = Fixture::new(&FakeInstall::new()).await;
        assert_eq!(empty.ctx.players().self_profile_id().await.unwrap(), None);
        assert!(
            empty
                .ctx
                .players()
                .self_alias_ids()
                .await
                .unwrap()
                .is_empty()
        );

        let f = pilot(Some(PILOT_CFG_USERNAME)).await;
        let me = f
            .ctx
            .user_db()
            .read(profile::self_profile)
            .unwrap()
            .unwrap();
        assert_eq!(
            f.ctx.players().self_profile_id().await.unwrap(),
            Some(me.id)
        );
        let mut want = vec![id(&f, TWULFZ), id(&f, PILOT_CFG_USERNAME)];
        want.sort();
        assert_eq!(f.ctx.players().self_alias_ids().await.unwrap(), want);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn first_refresh_bootstraps_self_with_auto_aliases() {
        let f = pilot(Some(PILOT_CFG_USERNAME)).await;
        let me = f
            .ctx
            .user_db()
            .read(profile::self_profile)
            .unwrap()
            .unwrap();
        assert_eq!(me.label, "Me");
        assert!(me.is_default);
        assert_eq!(me.merge_mode, repo::MergeMode::Merged);
        assert_eq!(self_rows(&f), auto(&[TWULFZ, PILOT_CFG_USERNAME]));

        let list = f.ctx.players().list_aliases().await.unwrap();
        assert!(list.cfg_username_available);
        assert!(list.wizard_needed);
        let names: BTreeMap<AliasId, String> = ids(&f).into_iter().map(|(n, i)| (i, n)).collect();
        let order: Vec<(&str, bool)> = list
            .rows
            .iter()
            .map(|r| (names[&r.alias_id].as_str(), r.selected))
            .collect();
        let expected: Vec<(&str, bool)> = [TWULFZ, PILOT_CFG_USERNAME]
            .into_iter()
            .map(|n| (n, true))
            .chain(
                PILOT_ALIASES
                    .iter()
                    .map(|(n, _)| *n)
                    .filter(|n| *n != TWULFZ && *n != PILOT_CFG_USERNAME)
                    .map(|n| (n, false)),
            )
            .collect();
        assert_eq!(order, expected);
        let twulfz = &list.rows[0];
        assert_eq!(
            twulfz.auto_match,
            Some(AutoMatch {
                source: MatchSource::CfgUsername,
                kind: SessionMatchKind::Prefix
            })
        );
        assert!(twulfz.in_self_profile);
        let buckets: Vec<String> = twulfz
            .stats
            .by_keymode
            .iter()
            .map(|(b, _)| b.to_string())
            .collect();
        assert_eq!(buckets, ["k7", "unknown"]);
        assert!(
            twulfz.top_charts.iter().any(|c| c.title.is_some()),
            "catalog titles joined: {:?}",
            twulfz.top_charts
        );
        assert!(list.rows[2..].iter().all(|r| !r.in_self_profile));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cfg_change_moves_auto_rows_only() {
        let f = pilot(Some(PILOT_CFG_USERNAME)).await;
        let players = f.ctx.players();
        players
            .decide(vec![(id(&f, ""), Some(Decision::Me))], false)
            .await
            .unwrap();
        set_cfg(&f, "Wulfy");
        assert!(players.refresh().await.unwrap());
        let mut expected = auto(&["Wulf"]);
        expected.insert(String::new(), AliasOrigin::User);
        assert_eq!(self_rows(&f), expected);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn refresh_is_idempotent() {
        let f = pilot(Some(PILOT_CFG_USERNAME)).await;
        let rows = self_rows(&f);
        let events = feedback(&f).len();
        assert!(!f.ctx.players().refresh().await.unwrap(), "nothing changed");
        assert_eq!(self_rows(&f), rows);
        assert_eq!(feedback(&f).len(), events);
        let profiles = f.ctx.user_db().read(profile::list).unwrap();
        assert_eq!(profiles.len(), 1, "one self profile, however often it runs");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn wizard_needed_until_completed() {
        let empty = Fixture::new(&FakeInstall::new()).await;
        empty.sync().await;
        assert!(
            !empty.ctx.players().wizard_needed().await.unwrap(),
            "no plays yet"
        );

        let f = pilot(Some(PILOT_CFG_USERNAME)).await;
        let players = f.ctx.players();
        assert!(players.wizard_needed().await.unwrap());
        let list = players
            .decide(vec![(id(&f, "W"), Some(Decision::NotMe))], false)
            .await
            .unwrap();
        assert!(
            list.wizard_needed,
            "a settings edit does not complete the wizard"
        );
        let list = players
            .decide(vec![(id(&f, TWULFZ), Some(Decision::Me))], true)
            .await
            .unwrap();
        assert!(!list.wizard_needed);
        assert!(!players.wizard_needed().await.unwrap());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn missing_cfg_sets_flag() {
        let f = pilot(None).await;
        let list = f.ctx.players().list_aliases().await.unwrap();
        assert!(!list.cfg_username_available);
        assert!(
            list.rows
                .iter()
                .all(|r| !r.selected && r.auto_match.is_none())
        );
        assert!(
            self_rows(&f).is_empty(),
            "the self profile exists, without names"
        );
        let plays: Vec<u32> = list.rows.iter().map(|r| r.stats.n_plays).collect();
        assert!(plays.windows(2).all(|w| w[0] >= w[1]), "{plays:?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn wizard_needed_is_cheap_and_matches_dto() {
        let f = Fixture::new(&pilot_install(Some(PILOT_CFG_USERNAME))).await;
        let players = f.ctx.players();
        let check = || async {
            let list = players.list_aliases().await.unwrap();
            assert_eq!(players.wizard_needed().await.unwrap(), list.wizard_needed);
            list.wizard_needed
        };
        assert!(!check().await, "before the first sync");
        f.sync().await;
        assert!(check().await);
        players
            .decide(vec![(id(&f, TWULFZ), Some(Decision::Me))], true)
            .await
            .unwrap();
        assert!(!check().await);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn not_me_survives_refresh() {
        let f = pilot(Some(PILOT_CFG_USERNAME)).await;
        let twulfz = id(&f, TWULFZ);
        f.ctx
            .players()
            .decide(vec![(twulfz, Some(Decision::NotMe))], true)
            .await
            .unwrap();
        f.write("scores.db", &scores_db(&pilot_scores(3)));
        f.sync().await;
        assert_eq!(self_rows(&f), auto(&[PILOT_CFG_USERNAME]));
        let list = f.ctx.players().list_aliases().await.unwrap();
        let row = list.rows.iter().find(|r| r.alias_id == twulfz).unwrap();
        assert_eq!(row.decision, Some(Decision::NotMe));
        assert!(!row.selected);
        assert!(
            row.auto_match.is_some(),
            "the chip stays, the decision wins"
        );
        assert_eq!(row.stats.n_plays, 33, "the new plays were counted");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn me_on_empty_name_adds_user_origin() {
        let f = pilot(Some(PILOT_CFG_USERNAME)).await;
        let list = f
            .ctx
            .players()
            .decide(vec![(id(&f, ""), Some(Decision::Me))], false)
            .await
            .unwrap();
        assert_eq!(self_rows(&f)[""], AliasOrigin::User);
        let row = list.rows.iter().find(|r| r.alias_id == id(&f, "")).unwrap();
        assert!(row.selected && row.in_self_profile);
        let events = feedback(&f);
        assert_eq!(events.len(), 1);
        let e = &events[0];
        assert_eq!(e.kind, "identity_decision");
        assert_eq!(
            e.payload,
            serde_json::json!({"decision": "me", "via": "settings"})
        );
        assert_eq!(
            e.subject,
            serde_json::json!({"alias": {"game": "osu_stable", "raw_name_b64": ""}})
        );
        let entries = f.ctx.players().list_profiles(Keymode::K7).await.unwrap();
        assert_eq!(
            e.context["scope_hash"],
            serde_json::json!(self_hash(&entries).unwrap().to_string()),
            "the event records the self scope after the change"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn clear_restores_proposal() {
        let f = pilot(Some(PILOT_CFG_USERNAME)).await;
        let players = f.ctx.players();
        let (twulfz, w) = (id(&f, TWULFZ), id(&f, "W"));
        let baseline = players.list_aliases().await.unwrap();
        players
            .decide(
                vec![(twulfz, Some(Decision::NotMe)), (w, Some(Decision::Me))],
                false,
            )
            .await
            .unwrap();
        let cleared = players
            .decide(vec![(twulfz, None), (w, None)], false)
            .await
            .unwrap();
        assert_eq!(self_rows(&f), auto(&[TWULFZ, PILOT_CFG_USERNAME]));
        let selected = |l: &AliasList| -> Vec<(AliasId, bool)> {
            l.rows.iter().map(|r| (r.alias_id, r.selected)).collect()
        };
        assert_eq!(selected(&cleared), selected(&baseline));
        assert!(cleared.rows.iter().all(|r| r.decision.is_none()));
        let cleared_events = feedback(&f)
            .iter()
            .filter(|e| e.payload["decision"].is_null())
            .count();
        assert_eq!(cleared_events, 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn self_scope_hash_changes_with_alias_set() {
        let f = pilot(Some(PILOT_CFG_USERNAME)).await;
        let players = f.ctx.players();
        let before = self_hash(&players.list_profiles(Keymode::K7).await.unwrap()).unwrap();
        players
            .decide(vec![(id(&f, "W"), Some(Decision::Me))], false)
            .await
            .unwrap();
        let after = self_hash(&players.list_profiles(Keymode::K7).await.unwrap()).unwrap();
        assert_ne!(before, after);
        players
            .decide(vec![(id(&f, "W"), None)], false)
            .await
            .unwrap();
        let back = self_hash(&players.list_profiles(Keymode::K7).await.unwrap()).unwrap();
        assert_eq!(back, before, "the hash depends on the alias set only");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn decide_rejects_duplicates_and_unknown_aliases() {
        let f = pilot(Some(PILOT_CFG_USERNAME)).await;
        let players = f.ctx.players();
        let w = id(&f, "W");
        let dup = players
            .decide(vec![(w, Some(Decision::Me)), (w, None)], false)
            .await
            .unwrap_err();
        assert_eq!(dup.code, ErrorCode::InvalidInput);
        let unknown = players
            .decide(vec![(AliasId(9_999), Some(Decision::Me))], false)
            .await
            .unwrap_err();
        assert_eq!(unknown.code, ErrorCode::NotFound);
        assert!(feedback(&f).is_empty(), "a refused batch writes nothing");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn profile_invariants() {
        let f = pilot(Some(PILOT_CFG_USERNAME)).await;
        let players = f.ctx.players();
        let (rosalind, kovacs, twulfz) = (id(&f, "Rosalind"), id(&f, "Kovacs"), id(&f, TWULFZ));

        let other = players
            .create_profile("  Rosalind  ".to_owned(), vec![rosalind], None)
            .await
            .unwrap();
        assert_eq!(other.kind, EntryKind::Other);
        assert_eq!(other.label, "Rosalind");
        assert!(!other.is_default);
        let EntryRef::Profile(other_id) = other.entry else {
            panic!("persisted profile expected");
        };

        let code = |r: Result<ProfileEntry, AppError>| r.unwrap_err().code;
        assert_eq!(
            code(players.create_profile("x".into(), vec![twulfz], None).await),
            ErrorCode::Conflict,
            "an auto self alias cannot join another profile"
        );
        let err = players
            .decide(vec![(rosalind, Some(Decision::Me))], false)
            .await
            .unwrap_err();
        assert_eq!(
            err.code,
            ErrorCode::Conflict,
            "nor the reverse, via decide(me)"
        );
        assert_eq!(err.args["profileIds"], other_id.0.to_string());
        for label in ["", "   ", &"x".repeat(65)] {
            assert_eq!(
                code(
                    players
                        .create_profile(label.to_owned(), vec![kovacs], None)
                        .await
                ),
                ErrorCode::InvalidInput,
                "{label:?}"
            );
        }
        let longest = players
            .create_profile("x".repeat(64), vec![kovacs], None)
            .await
            .unwrap();
        assert_eq!(longest.label.chars().count(), 64);
        assert_eq!(
            code(players.create_profile("k".into(), vec![], None).await),
            ErrorCode::InvalidInput
        );
        assert_eq!(
            code(
                players
                    .create_profile("k".into(), vec![kovacs, kovacs], None)
                    .await
            ),
            ErrorCode::InvalidInput
        );
        assert_eq!(
            code(
                players
                    .create_profile("k".into(), vec![AliasId(9_999)], None)
                    .await
            ),
            ErrorCode::NotFound
        );
        assert_eq!(
            code(players.set_profile_aliases(other_id, vec![], None).await),
            ErrorCode::InvalidInput
        );
        assert_eq!(
            code(
                players
                    .set_profile_aliases(ProfileId(9_999), vec![kovacs], None)
                    .await
            ),
            ErrorCode::NotFound
        );

        players.set_default(other_id).await.unwrap();
        let profiles = f.ctx.user_db().read(profile::list).unwrap();
        let defaults: Vec<ProfileId> = profiles
            .iter()
            .filter(|p| p.is_default)
            .map(|p| p.id)
            .collect();
        assert_eq!(defaults, vec![other_id], "exactly one default");
        assert_eq!(
            players
                .set_default(ProfileId(9_999))
                .await
                .unwrap_err()
                .code,
            ErrorCode::NotFound
        );

        let second_self = f
            .ctx
            .user_db()
            .write(|tx| {
                profile::insert(
                    tx,
                    &NewProfile {
                        kind: ProfileKind::SelfProfile,
                        label: "Me too".into(),
                        is_default: false,
                        merge_mode: repo::MergeMode::Merged,
                        created_at: wolluf_core::UnixUs(0),
                    },
                )
            })
            .map_err(AppError::from)
            .unwrap_err();
        assert_eq!(second_self.code, ErrorCode::Conflict);

        let entries = players.list_profiles(Keymode::K7).await.unwrap();
        let all = entries.last().unwrap();
        assert_eq!(all.kind, EntryKind::AllPlayers);
        assert!(!all.is_default);
        assert_eq!(
            profiles.len(),
            entries.len() - 1,
            "All players is never persisted"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn self_set_aliases_maps_to_decisions() {
        let f = pilot(Some(PILOT_CFG_USERNAME)).await;
        let players = f.ctx.players();
        let me = f
            .ctx
            .user_db()
            .read(profile::self_profile)
            .unwrap()
            .unwrap();
        let (twulfz, empty, garbage) = (id(&f, TWULFZ), id(&f, ""), id(&f, PILOT_CFG_USERNAME));
        let entry = players
            .set_profile_aliases(me.id, vec![twulfz, empty], Some(MergeMode::Separate))
            .await
            .unwrap();
        assert_eq!(entry.merge_mode, MergeMode::Separate);
        let decisions = f.ctx.user_db().read(repo::identity_decision::list).unwrap();
        let decided: BTreeMap<AliasId, repo::IdentityDecision> = decisions
            .into_iter()
            .map(|(a, d)| (a, d.decision))
            .collect();
        assert_eq!(
            decided,
            BTreeMap::from([
                (empty, repo::IdentityDecision::Me),
                (garbage, repo::IdentityDecision::NotMe),
            ])
        );
        let mut expected = auto(&[TWULFZ]);
        expected.insert(String::new(), AliasOrigin::User);
        assert_eq!(self_rows(&f), expected);
        assert!(
            feedback(&f)
                .iter()
                .all(|e| e.payload["via"] == "profile_edit")
        );
        assert_eq!(entry.scopes.len(), 2, "separate: one scope per alias");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn list_profiles_includes_all_players_with_scopes() {
        let f = pilot(Some(PILOT_CFG_USERNAME)).await;
        let players = f.ctx.players();
        let (rosalind, kovacs) = (id(&f, "Rosalind"), id(&f, "Kovacs"));
        players
            .create_profile(
                "Others".into(),
                vec![kovacs, rosalind],
                Some(MergeMode::Separate),
            )
            .await
            .unwrap();
        let entries = players.list_profiles(Keymode::K7).await.unwrap();
        let kinds: Vec<EntryKind> = entries.iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            [
                EntryKind::SelfProfile,
                EntryKind::Other,
                EntryKind::AllPlayers
            ]
        );
        assert_eq!(entries[0].scopes.len(), 1, "self is merged");
        assert_eq!(entries[1].scopes.len(), 2, "separate over 2 aliases");
        let all = &entries[2];
        assert_eq!(all.entry, EntryRef::AllPlayers);
        assert_eq!(all.alias_ids.len(), PILOT_ALIASES.len());
        assert_eq!(all.scopes.len(), 1);
        assert_eq!(all.scopes[0].alias_ids.len(), PILOT_ALIASES.len());
        assert!(all.scopes.iter().all(|s| s.keymode == Keymode::K7));

        let merged = players
            .resolve_scopes(entries[1].entry, Keymode::K7, Some(MergeMode::Merged))
            .await
            .unwrap();
        assert_eq!(
            merged.len(),
            1,
            "the URL override wins over the persisted mode"
        );
        let split = players
            .resolve_scopes(EntryRef::AllPlayers, Keymode::K4, Some(MergeMode::Separate))
            .await
            .unwrap();
        assert_eq!(split.len(), PILOT_ALIASES.len());
    }
}
