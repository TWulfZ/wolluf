//! Data-dir resolution, the process-wide context and install registration (spec 003 T13).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use tokio::runtime::{Handle, Runtime};
use tokio::sync::broadcast;
use wolluf_core::{Clock, Game};
use wolluf_source_osu::cfg_files::{list_user_cfgs, read_user_cfg};
use wolluf_source_osu::install::Platform;
use wolluf_source_osu::paths::{is_drvfs_path, resolve_songs_dir};
use wolluf_store::repo::ledger::{GameInstall, InstallRow, game_install, install};
use wolluf_store::{DbHandle, InstanceLock, Vault, open_cache_db, open_user_db};

use crate::errors::{AppError, keys};
use crate::events::AppEvent;
use crate::features::labeling::LabelingService;
use crate::features::library::LibraryService;
use crate::features::players::PlayersService;
use crate::features::plays::PlaysService;
use crate::features::setup::SetupService;
use crate::jobs::{JobRunner, JobService};

pub use wolluf_store::repo::ledger::InstallId;

/// Read only here, so every other module sees the data dir through [`AppPaths`] (spec 003).
const DATA_DIR_ENV: &str = "WOLLUF_DATA_DIR";
const APP_NAME: &str = "wolluf";

const USER_DB: &str = "user.db";
const CACHE_DB: &str = "cache.db";
const VAULT_DIR: &str = "vault";
const BACKUPS_DIR: &str = "backups";
const LOGS_DIR: &str = "logs";
const LOCK_FILE: &str = "wolluf.lock";

/// Spec 003: a bus slower than 256 events means the receiver lags; the desktop bridge then
/// re-hydrates the tray instead of replaying (spec 005).
const EVENT_BUS_CAPACITY: usize = 256;
/// One core stays free for the UI and the async runtime (spec 003 Design).
const CORES_LEFT_FREE: usize = 1;

/// Every file wolluf owns lives under one data dir (architecture §5.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    data_dir: PathBuf,
}

impl AppPaths {
    /// Order: the explicit override (CLI `--data-dir`), then `WOLLUF_DATA_DIR`, then the
    /// platform data dir (`%LOCALAPPDATA%\wolluf\data`, `$XDG_DATA_HOME/wolluf`).
    pub fn resolve(override_dir: Option<PathBuf>) -> Result<Self, AppError> {
        let data_dir = resolve_data_dir(override_dir, std::env::var_os(DATA_DIR_ENV), || {
            directories::ProjectDirs::from("", "", APP_NAME)
                .map(|dirs| dirs.data_local_dir().to_path_buf())
        })?;
        Ok(Self { data_dir })
    }

    pub fn from_data_dir(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.data_dir.join(LOGS_DIR)
    }

    pub fn user_db(&self) -> PathBuf {
        self.data_dir.join(USER_DB)
    }

    pub fn cache_db(&self) -> PathBuf {
        self.data_dir.join(CACHE_DB)
    }

    pub fn vault_dir(&self) -> PathBuf {
        self.data_dir.join(VAULT_DIR)
    }

    pub fn backups_dir(&self) -> PathBuf {
        self.data_dir.join(BACKUPS_DIR)
    }

    pub fn lock_file(&self) -> PathBuf {
        self.data_dir.join(LOCK_FILE)
    }
}

fn resolve_data_dir(
    override_dir: Option<PathBuf>,
    env_value: Option<OsString>,
    platform_default: impl FnOnce() -> Option<PathBuf>,
) -> Result<PathBuf, AppError> {
    if let Some(dir) = override_dir {
        return Ok(dir);
    }
    if let Some(dir) = env_value.filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    platform_default().ok_or_else(|| AppError::internal("no platform data directory"))
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Component-wise, so `osu!2` is not inside `osu!`. Only `app::export` may write into an osu!
/// folder (D9); a data dir there would make every store write one.
fn data_dir_inside(data_dir: &Path, root: &Path) -> bool {
    canonical(data_dir).starts_with(canonical(root))
}

fn guard_data_dir(data_dir: &Path, root: &Path) -> Result<(), AppError> {
    if data_dir_inside(data_dir, root) {
        return Err(AppError::invalid_input()
            .with_key(keys::DATA_DIR_INSIDE_OSU)
            .with_arg("dataDir", data_dir.to_string_lossy())
            .with_arg("osuDir", root.to_string_lossy()));
    }
    Ok(())
}

/// The desktop shares Tauri's runtime; the CLI and tests usually run inside one. Only when
/// `open` is called outside any runtime does the context own one.
enum RuntimeHolder {
    Shared(Handle),
    Owned(Option<Runtime>),
}

impl RuntimeHolder {
    fn current_or_owned() -> Result<Self, AppError> {
        if let Ok(handle) = Handle::try_current() {
            return Ok(Self::Shared(handle));
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("wolluf-app")
            .build()
            .map_err(|e| AppError::internal(format!("tokio runtime: {e}")))?;
        Ok(Self::Owned(Some(runtime)))
    }

    fn handle(&self) -> Handle {
        match self {
            Self::Shared(handle) => handle.clone(),
            Self::Owned(Some(runtime)) => runtime.handle().clone(),
            // Only reachable during drop, after the runtime was taken.
            Self::Owned(None) => Handle::current(),
        }
    }
}

impl Drop for RuntimeHolder {
    fn drop(&mut self) {
        // A plain drop panics when the context is dropped from async code.
        if let Self::Owned(runtime) = self
            && let Some(runtime) = runtime.take()
        {
            runtime.shutdown_background();
        }
    }
}

/// The newest cfg decides `BeatmapDirectory` (002 R-d); anything unreadable means `Songs`.
pub(crate) fn songs_dir(root: &Path) -> PathBuf {
    let beatmap_directory = list_user_cfgs(root)
        .ok()
        .and_then(|cfgs| cfgs.into_iter().next())
        .and_then(|cfg| read_user_cfg(&cfg.path).ok())
        .and_then(|(cfg, _)| cfg.beatmap_directory);
    let platform = if cfg!(windows) {
        Platform::Windows
    } else if is_drvfs_path(root) {
        Platform::Wsl
    } else {
        Platform::Linux
    };
    resolve_songs_dir(root, beatmap_directory.as_deref(), platform)
}

pub(crate) fn blocking_join_error(e: tokio::task::JoinError) -> AppError {
    AppError::internal(format!("blocking task failed: {e}"))
}

/// Everything a service needs, opened once per process. Holding it holds the instance lock.
/// [`AppContext::close`] is the shutdown contract; a plain drop only stops what it can without
/// waiting for the job worker.
pub struct AppContext {
    jobs: JobRunner,
    paths: AppPaths,
    clock: Arc<dyn Clock>,
    user: DbHandle,
    cache: DbHandle,
    events: broadcast::Sender<AppEvent>,
    install: InstallRow,
    runtime: RuntimeHolder,
    // Taken after the stores close, so a reopen never races a closing writer.
    lock: Mutex<Option<InstanceLock>>,
}

impl Drop for AppContext {
    fn drop(&mut self) {
        self.jobs.stop();
        // A cancelled job keeps store clones until its task is polled again; closing here
        // makes its late writes fail instead of outliving the context.
        self.close_stores();
    }
}

impl std::fmt::Debug for AppContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppContext")
            .field("data_dir", &self.paths.data_dir)
            .finish_non_exhaustive()
    }
}

impl AppContext {
    /// Blocking: creates the data dir layout, takes the instance lock before touching any
    /// database, migrates user.db and ensures the install row.
    pub fn open(paths: AppPaths, clock: Arc<dyn Clock>) -> Result<Self, AppError> {
        Self::open_with(paths, clock, RuntimeHolder::current_or_owned()?)
    }

    /// For a shell that owns the runtime but calls from outside it (Tauri's setup hook).
    pub fn open_in(
        paths: AppPaths,
        clock: Arc<dyn Clock>,
        handle: Handle,
    ) -> Result<Self, AppError> {
        Self::open_with(paths, clock, RuntimeHolder::Shared(handle))
    }

    fn open_with(
        paths: AppPaths,
        clock: Arc<dyn Clock>,
        runtime: RuntimeHolder,
    ) -> Result<Self, AppError> {
        let data_dir = paths.data_dir();
        for dir in [data_dir.to_path_buf(), paths.backups_dir()] {
            std::fs::create_dir_all(&dir)
                .map_err(|e| AppError::internal(format!("create {}: {e}", dir.display())))?;
        }
        let lock = InstanceLock::acquire(&paths.lock_file())?;
        let user = open_user_db(&paths.user_db(), &paths.backups_dir(), clock.now())?;
        let installs = user.read(game_install::list)?;
        for known in &installs {
            guard_data_dir(data_dir, &known.root_path)?;
        }
        let cache = open_cache_db(&paths.cache_db())?;
        let vault = Vault::open(&paths.vault_dir())?;
        let now = clock.now();
        let install = user.write(move |tx| install::ensure(tx, now))?;
        let threads = std::thread::available_parallelism()
            .map_or(1, |n| n.get().saturating_sub(CORES_LEFT_FREE).max(1));
        let cpu = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|i| format!("wolluf-cpu-{i}"))
            .build()
            .map_err(|e| AppError::internal(format!("rayon pool: {e}")))?;
        let (events, _) = broadcast::channel(EVENT_BUS_CAPACITY);
        let jobs = JobRunner::start(
            &runtime.handle(),
            user.clone(),
            cache.clone(),
            vault,
            Arc::new(cpu),
            clock.clone(),
            events.clone(),
        );
        Ok(Self {
            jobs,
            paths,
            clock,
            user,
            cache,
            events,
            install,
            runtime,
            lock: Mutex::new(Some(lock)),
        })
    }

    pub fn paths(&self) -> &AppPaths {
        &self.paths
    }

    pub fn clock(&self) -> &Arc<dyn Clock> {
        &self.clock
    }

    pub fn install(&self) -> &InstallRow {
        &self.install
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AppEvent> {
        self.events.subscribe()
    }

    /// No subscriber is not an error: the CLI may run without listening.
    pub(crate) fn emit(&self, event: AppEvent) {
        let _ = self.events.send(event);
    }

    pub fn runtime(&self) -> Handle {
        self.runtime.handle()
    }

    pub fn jobs(&self) -> &JobRunner {
        &self.jobs
    }

    pub fn job_service(&self) -> JobService<'_> {
        JobService::new(self)
    }

    pub fn plays(&self) -> PlaysService<'_> {
        PlaysService::new(self)
    }

    pub fn library(&self) -> LibraryService<'_> {
        LibraryService::new(self)
    }

    pub fn labeling(&self) -> LabelingService<'_> {
        LabelingService::new(self)
    }

    pub fn players(&self) -> PlayersService<'_> {
        PlayersService::new(self)
    }

    /// Reads the system environment (and `reg.exe` on WSL) at each call; shells that need a
    /// fixed environment use `SetupService::with_env`.
    pub fn setup(&self) -> SetupService<'_> {
        SetupService::new(self)
    }

    pub(crate) fn user_db(&self) -> &DbHandle {
        &self.user
    }

    pub(crate) fn cache_db(&self) -> &DbHandle {
        &self.cache
    }

    /// Upserts `game_install` on `(game, root)`; 005's setup service calls it after
    /// validating the install. Keeps the first `detected_at`.
    pub async fn register_install(
        &self,
        root: PathBuf,
        client_version: Option<i32>,
    ) -> Result<InstallId, AppError> {
        guard_data_dir(self.paths.data_dir(), &root)?;
        let user = self.user.clone();
        let now = self.clock.now();
        let id = tokio::task::spawn_blocking(move || {
            user.write(move |tx| {
                game_install::upsert(tx, Game::OsuStable, &root, client_version, now)
            })
        })
        .await
        .map_err(blocking_join_error)??;
        Ok(id)
    }

    /// Returns once every handle under the data dir is released: the running job is cancelled
    /// and recorded, both stores are closed and the instance lock is dropped.
    pub async fn close(self) {
        self.shutdown().await;
    }

    /// [`AppContext::close`] for a shell that cannot take ownership (Tauri managed state).
    /// Every later store call fails with `INTERNAL`. Idempotent.
    pub async fn shutdown(&self) {
        self.jobs.shutdown().await;
        let (user, cache) = (self.user.clone(), self.cache.clone());
        if let Err(e) = tokio::task::spawn_blocking(move || {
            user.close();
            cache.close();
        })
        .await
        {
            tracing::error!(error = %e, "closing the stores off the runtime failed");
        }
        // No-op after the blocking task; closes here if the runtime refused it.
        self.close_stores();
        self.lock
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
    }

    fn close_stores(&self) {
        self.user.close();
        self.cache.close();
    }

    pub async fn installs(&self) -> Result<Vec<GameInstall>, AppError> {
        let user = self.user.clone();
        let list = tokio::task::spawn_blocking(move || user.read(game_install::list))
            .await
            .map_err(blocking_join_error)??;
        Ok(list)
    }
}

#[cfg(test)]
mod tests {
    use wolluf_core::{ErrorCode, FixedClock, UnixUs};

    use super::*;

    const T0: UnixUs = UnixUs(1_790_637_236_636_000);

    fn clock() -> Arc<dyn wolluf_core::Clock> {
        Arc::new(FixedClock::new(T0))
    }

    #[test]
    fn explicit_override_beats_env() {
        let got = resolve_data_dir(
            Some(PathBuf::from("/explicit")),
            Some(OsString::from("/from-env")),
            || Some(PathBuf::from("/default")),
        )
        .unwrap();
        assert_eq!(got, PathBuf::from("/explicit"));
    }

    #[test]
    fn env_override_wins() {
        let got = resolve_data_dir(None, Some(OsString::from("/from-env")), || {
            Some(PathBuf::from("/default"))
        })
        .unwrap();
        assert_eq!(got, PathBuf::from("/from-env"));
        // An empty variable is "unset", as shells export it that way.
        let got = resolve_data_dir(None, Some(OsString::new()), || {
            Some(PathBuf::from("/default"))
        })
        .unwrap();
        assert_eq!(got, PathBuf::from("/default"));
        let err = resolve_data_dir(None, None, || None).unwrap_err();
        assert_eq!(err.code, ErrorCode::Internal);
    }

    #[test]
    fn paths_layout() {
        let paths = AppPaths::from_data_dir(PathBuf::from("/d"));
        assert_eq!(paths.logs_dir(), Path::new("/d/logs"));
        assert_eq!(paths.user_db(), Path::new("/d/user.db"));
        assert_eq!(paths.cache_db(), Path::new("/d/cache.db"));
        assert_eq!(paths.vault_dir(), Path::new("/d/vault"));
        assert_eq!(paths.backups_dir(), Path::new("/d/backups"));
        assert_eq!(paths.lock_file(), Path::new("/d/wolluf.lock"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn open_creates_layout_and_install_row() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let ctx = AppContext::open(AppPaths::from_data_dir(data.clone()), clock()).unwrap();
        for f in ["user.db", "cache.db", "vault", "backups", "wolluf.lock"] {
            assert!(data.join(f).exists(), "{f}");
        }
        let first = ctx.install().clone();
        drop(ctx);
        let ctx = AppContext::open(AppPaths::from_data_dir(data), clock()).unwrap();
        assert_eq!(ctx.install(), &first, "install row is created once");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn second_context_same_dir_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_data_dir(dir.path().join("data"));
        let _first = AppContext::open(paths.clone(), clock()).unwrap();
        let err = AppContext::open(paths, clock()).err().unwrap();
        assert_eq!(err.code, ErrorCode::Conflict);
        assert_eq!(err.message_key, "error.instance_running");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn data_dir_inside_osu_root_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let osu = dir.path().join("osu!");
        std::fs::create_dir_all(&osu).unwrap();
        let ctx = AppContext::open(AppPaths::from_data_dir(osu.join("wolluf")), clock()).unwrap();
        let err = ctx.register_install(osu.clone(), Some(20260924)).await;
        let err = err.unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
        assert_eq!(err.message_key, "error.data_dir_inside_osu");
        // Equal paths are rejected too: the data dir would be the osu! folder itself.
        drop(ctx);
        let ctx = AppContext::open(AppPaths::from_data_dir(osu.clone()), clock()).unwrap();
        let err = ctx.register_install(osu, None).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn register_install_upserts_and_guards() {
        let dir = tempfile::tempdir().unwrap();
        let osu = dir.path().join("osu!");
        std::fs::create_dir_all(&osu).unwrap();
        let ctx =
            AppContext::open(AppPaths::from_data_dir(dir.path().join("data")), clock()).unwrap();
        let a = ctx.register_install(osu.clone(), None).await.unwrap();
        let b = ctx
            .register_install(osu.clone(), Some(20260924))
            .await
            .unwrap();
        assert_eq!(a, b, "same root upserts");
        let installs = ctx.installs().await.unwrap();
        assert_eq!(installs.len(), 1);
        assert_eq!(installs[0].client_version, Some(20260924));
        // A sibling whose name merely shares a prefix is not "inside" (component-wise check).
        let sibling = dir.path().join("osu!2");
        std::fs::create_dir_all(&sibling).unwrap();
        let c = ctx.register_install(sibling, None).await.unwrap();
        assert_ne!(a, c);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn open_rejects_data_dir_under_registered_root() {
        let dir = tempfile::tempdir().unwrap();
        let osu = dir.path().join("osu!");
        std::fs::create_dir_all(&osu).unwrap();
        let data = dir.path().join("data");
        let ctx = AppContext::open(AppPaths::from_data_dir(data.clone()), clock()).unwrap();
        ctx.register_install(osu.clone(), None).await.unwrap();
        ctx.close().await;
        // Moving the data dir into a registered install is caught at the next start.
        let moved = osu.join("wolluf");
        std::fs::rename(&data, &moved).unwrap();
        let err = AppContext::open(AppPaths::from_data_dir(moved), clock())
            .err()
            .unwrap();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    /// This process's descriptors that resolve under `dir`; nextest runs each test in its own
    /// process, and the tempdir is unique either way.
    #[cfg(target_os = "linux")]
    fn open_fds_under(dir: &Path) -> Vec<PathBuf> {
        let dir = canonical(dir);
        std::fs::read_dir("/proc/self/fd")
            .unwrap()
            .filter_map(|e| std::fs::read_link(e.ok()?.path()).ok())
            .filter(|target| target.starts_with(&dir))
            .collect()
    }

    // current_thread: the job worker is never polled unless something awaits it, the worst case
    // for a release that depends on the worker noticing shutdown.
    #[tokio::test(flavor = "current_thread")]
    async fn close_releases_every_handle_under_data_dir() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let ctx = AppContext::open(AppPaths::from_data_dir(data.clone()), clock()).unwrap();
        ctx.register_install(dir.path().join("osu!"), None)
            .await
            .unwrap();
        #[cfg(target_os = "linux")]
        assert!(
            !open_fds_under(&data).is_empty(),
            "the probe sees an open context"
        );
        ctx.close().await;
        #[cfg(target_os = "linux")]
        assert_eq!(open_fds_under(&data), Vec::<PathBuf>::new());
        // Windows refuses both while any handle under the dir is open.
        for f in ["user.db", "cache.db", "wolluf.lock"] {
            std::fs::remove_file(data.join(f)).unwrap();
        }
        std::fs::rename(&data, dir.path().join("moved")).unwrap();
    }

    #[test]
    fn close_on_owned_runtime_releases_every_handle() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let ctx = AppContext::open(AppPaths::from_data_dir(data.clone()), clock()).unwrap();
        ctx.runtime().block_on(ctx.close());
        #[cfg(target_os = "linux")]
        assert_eq!(open_fds_under(&data), Vec::<PathBuf>::new());
        std::fs::rename(&data, dir.path().join("moved")).unwrap();
    }
}
