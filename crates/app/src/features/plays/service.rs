//! `PlaysService` (spec 003 IPC): the entry point shells and the watcher use to sync.

use std::collections::BTreeSet;

use tokio::sync::broadcast::error::RecvError;
use wolluf_core::{AliasId, ChartMd5};
use wolluf_store::repo::ledger::play;

use crate::context::{AppContext, InstallId, blocking_join_error};
use crate::errors::AppError;
use crate::events::AppEvent;
use crate::features::plays::SyncPlaysJob;
use crate::jobs::dto::{JobDto, JobId};

pub struct PlaysService<'a> {
    ctx: &'a AppContext,
}

impl<'a> PlaysService<'a> {
    pub fn new(ctx: &'a AppContext) -> Self {
        Self { ctx }
    }

    /// Charts with a play under one of `aliases`; callers pass a profile's aliases so another
    /// player's plays never count as the user's.
    pub async fn played_charts(&self, aliases: &[AliasId]) -> Result<BTreeSet<ChartMd5>, AppError> {
        let user = self.ctx.user_db().clone();
        let aliases = aliases.to_vec();
        let played =
            tokio::task::spawn_blocking(move || user.read(|c| play::charts_played_by(c, &aliases)))
                .await
                .map_err(blocking_join_error)??;
        Ok(played)
    }

    /// Returns at once with the job id (a queued sync for the same install is reused). An
    /// unknown install is `NOT_FOUND` here rather than a failed job.
    pub async fn sync(&self, install_id: InstallId) -> Result<JobId, AppError> {
        self.ensure_install(install_id).await?;
        Ok(self
            .ctx
            .jobs()
            .submit(Box::new(SyncPlaysJob::new(install_id))))
    }

    /// Runs one sync and its follow-ups (the identity refresh) to their end and returns the
    /// sync's history entry, whatever its status; `Err` only for an unknown install or broken
    /// bookkeeping.
    pub async fn sync_and_wait(&self, install_id: InstallId) -> Result<JobDto, AppError> {
        // Subscribed before submitting, so the finish event cannot slip past.
        let mut rx = self.ctx.subscribe();
        let id = self.sync(install_id).await?;
        loop {
            match rx.recv().await {
                Ok(AppEvent::JobFinished(f)) if f.job_id == id => break,
                // A lagging receiver only lost progress; the history below is authoritative.
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => {
                    return Err(AppError::internal("event bus closed during sync"));
                }
            }
        }
        self.ctx.jobs().wait_idle().await;
        self.ctx
            .jobs()
            .list(None)
            .await?
            .into_iter()
            .find(|j| j.id == id)
            .ok_or_else(|| AppError::internal(format!("job {id} missing from history")))
    }

    /// For callers without an async runtime (the CLI's plain `main`). Fails with `INTERNAL`
    /// when called from inside a runtime, where blocking would stall its worker.
    pub fn sync_blocking(&self, install_id: InstallId) -> Result<JobDto, AppError> {
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(AppError::internal(
                "sync_blocking called inside an async runtime; use sync_and_wait",
            ));
        }
        self.ctx.runtime().block_on(self.sync_and_wait(install_id))
    }

    async fn ensure_install(&self, install_id: InstallId) -> Result<(), AppError> {
        let user = self.ctx.user_db().clone();
        let found = tokio::task::spawn_blocking(move || {
            user.read(|c| wolluf_store::repo::ledger::game_install::get(c, install_id))
        })
        .await
        .map_err(blocking_join_error)??;
        match found {
            Some(_) => Ok(()),
            None => Err(AppError::not_found().with_arg("installId", install_id.0.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use wolluf_core::{ErrorCode, FixedClock};
    use wolluf_source_osu::testkit::{FakeInstall, ScoreBuilder};

    use super::*;
    use crate::context::AppPaths;
    use crate::features::plays::testkit::{Fixture, T0, md5_hex, scores_db};
    use crate::jobs::dto::{JobStartDto, JobStatusDto, JobSummaryDto, SyncPlaysStartDto};

    fn one_play() -> FakeInstall {
        FakeInstall::new().scores_db(scores_db(&[ScoreBuilder::mania(
            &md5_hex(b"svc"),
            "TWulfZ",
            1,
        )]))
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn sync_and_wait_returns_finished_summary() {
        let f = Fixture::new(&one_play()).await;
        let job = f.ctx.plays().sync_and_wait(f.install).await.unwrap();
        assert_eq!(job.status, JobStatusDto::Ok);
        let Some(JobSummaryDto::SyncPlays(s)) = job.summary else {
            panic!("no summary: {job:?}");
        };
        assert_eq!(s.plays_new, 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn unknown_install_is_not_found_and_blocking_refuses_runtime() {
        let f = Fixture::new(&one_play()).await;
        let unknown = InstallId(f.install.0 + 1);
        let err = f.ctx.plays().sync(unknown).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound);
        let err = f.ctx.plays().sync_blocking(f.install).unwrap_err();
        assert_eq!(err.code, ErrorCode::Internal, "blocking inside a runtime");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn job_service_start_maps_sync_plays() {
        let f = Fixture::new(&one_play()).await;
        let mut rx = f.ctx.subscribe();
        let request = JobStartDto::SyncPlays(SyncPlaysStartDto {
            install_id: u32::try_from(f.install.0).unwrap(),
        });
        let id = f.ctx.job_service().start(request).await.unwrap();
        loop {
            if let AppEvent::JobFinished(fin) = rx.recv().await.unwrap()
                && fin.job_id == id
            {
                break;
            }
        }
        let listed = f.ctx.job_service().list(None).await.unwrap();
        let job = listed.iter().find(|j| j.id == id).unwrap();
        assert_eq!(job.kind, crate::jobs::JobKindDto::SyncPlays);
        assert_eq!(job.status, JobStatusDto::Ok);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn played_charts_are_scoped_to_the_aliases() {
        let install = FakeInstall::new().scores_db(scores_db(&[
            ScoreBuilder::mania(&md5_hex(b"mine"), "TWulfZ", 1),
            ScoreBuilder::mania(&md5_hex(b"theirs"), "Kovacs", 2),
        ]));
        let f = Fixture::new(&install).await;
        f.ctx.plays().sync_and_wait(f.install).await.unwrap();
        let aliases = f
            .ctx
            .user_db()
            .read(wolluf_store::repo::ledger::alias::list)
            .unwrap();
        let alias = |name: &str| {
            aliases
                .iter()
                .find(|a| a.raw_name == name.as_bytes())
                .unwrap()
                .id
        };
        let md5 = |b: &[u8]| -> wolluf_core::ChartMd5 { md5_hex(b).parse().unwrap() };
        let plays = f.ctx.plays();
        assert_eq!(
            plays.played_charts(&[alias("TWulfZ")]).await.unwrap(),
            [md5(b"mine")].into()
        );
        assert_eq!(
            plays
                .played_charts(&[alias("TWulfZ"), alias("Kovacs")])
                .await
                .unwrap(),
            [md5(b"mine"), md5(b"theirs")].into()
        );
        assert!(plays.played_charts(&[]).await.unwrap().is_empty());
    }

    #[test]
    fn sync_blocking_outside_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("osu!");
        for (rel, bytes) in one_play().files() {
            crate::features::plays::testkit::write_file(&root, &rel, &bytes);
        }
        let ctx = AppContext::open(
            AppPaths::from_data_dir(dir.path().join("data")),
            Arc::new(FixedClock::new(T0)),
        )
        .unwrap();
        let install = ctx
            .runtime()
            .block_on(ctx.register_install(root, None))
            .unwrap();
        let job = ctx.plays().sync_blocking(install).unwrap();
        assert_eq!(job.status, JobStatusDto::Ok);
    }
}
