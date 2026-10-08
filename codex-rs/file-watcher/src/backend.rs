//! Serializes backend operations off async runtimes and coalesces desired state.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Duration;

use notify::RecommendedWatcher;
use notify::RecursiveMode;
use notify::Watcher;
use tokio::sync::watch;
use tracing::warn;

/// Synchronous watch operations owned exclusively by the backend thread.
/// Implementations may block; callers must only submit desired-state updates.
pub(super) trait WatchBackend: Send + 'static {
    fn watch(&mut self, path: &Path, mode: RecursiveMode) -> notify::Result<()>;
    fn unwatch(&mut self, path: &Path) -> notify::Result<()>;
}

impl WatchBackend for RecommendedWatcher {
    fn watch(&mut self, path: &Path, mode: RecursiveMode) -> notify::Result<()> {
        Watcher::watch(self, path, mode)
    }

    fn unwatch(&mut self, path: &Path) -> notify::Result<()> {
        Watcher::unwatch(self, path)
    }
}

#[derive(Clone)]
enum WatchStatus {
    Installed(RecursiveMode),
    Failed(String),
}

#[derive(Clone)]
pub(super) struct BackendController {
    pub(super) desired: Arc<Mutex<HashMap<PathBuf, RecursiveMode>>>,
    wake: mpsc::SyncSender<()>,
    readiness: watch::Receiver<HashMap<PathBuf, WatchStatus>>,
}

impl BackendController {
    pub(super) fn spawn(
        mut watcher: impl WatchBackend,
        activated: impl Fn(&Path) + Send + 'static,
    ) -> std::io::Result<Self> {
        let desired = Arc::new(Mutex::new(HashMap::new()));
        let backend_desired = Arc::clone(&desired);
        // Only one wake token is pending. Commands do not retain stale paths
        // or grow without bound when a platform backend stops responding.
        let (wake, receiver) = mpsc::sync_channel(1);
        let (readiness_tx, readiness) = watch::channel(HashMap::new());
        std::thread::Builder::new()
            .name("file-watcher-backend".to_owned())
            .spawn(move || {
                let mut active = HashMap::new();
                let mut retry = false;
                let mut retry_delay = Duration::from_millis(/*millis*/ 100);
                loop {
                    if retry {
                        match receiver.recv_timeout(retry_delay) {
                            Ok(()) => retry_delay = Duration::from_millis(/*millis*/ 100),
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                    } else if receiver.recv().is_err() {
                        break;
                    }
                    let next = backend_desired
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone();
                    let failures = reconcile(
                        &mut watcher,
                        &mut active,
                        &next,
                        |path| {
                            readiness_tx.send_modify(|status| {
                                status
                                    .insert(path.to_path_buf(), WatchStatus::Installed(next[path]));
                            });
                            if backend_desired
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .get(path)
                                == next.get(path)
                            {
                                activated(path);
                            }
                        },
                        |path| {
                            // An in-flight unwatch is no longer a readiness proof
                            // for a newly added registration of the same path.
                            readiness_tx.send_modify(|status| {
                                status.remove(path);
                            });
                        },
                    );
                    let mut status = active
                        .iter()
                        .map(|(path, mode)| (path.clone(), WatchStatus::Installed(*mode)))
                        .collect::<HashMap<_, _>>();
                    retry = !failures.is_empty();
                    status.extend(
                        failures
                            .into_iter()
                            .map(|(path, error)| (path, WatchStatus::Failed(error))),
                    );
                    readiness_tx.send_replace(status);
                    if retry {
                        retry_delay = (retry_delay * 2).min(Duration::from_secs(/*secs*/ 5));
                    } else {
                        retry_delay = Duration::from_millis(/*millis*/ 100);
                    }
                }
                // Drop the platform watcher on its own thread too.
            })?;
        Ok(Self {
            desired,
            wake,
            readiness,
        })
    }

    pub(super) async fn ready(&self, paths: &[(PathBuf, RecursiveMode)]) -> notify::Result<()> {
        let mut readiness = self.readiness.clone();
        loop {
            if readiness.has_changed().is_err() {
                return Err(notify::Error::generic(
                    "file watcher backend is unavailable",
                ));
            }
            let desired = {
                let desired = self
                    .desired
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some((path, _)) = paths.iter().find(|(path, _)| !desired.contains_key(path))
                {
                    return Err(
                        notify::Error::generic("watch registration changed").add_path(path.clone())
                    );
                }
                desired.clone()
            };
            {
                let status = readiness.borrow_and_update();
                let mut installed = true;
                for (path, mode) in paths {
                    match status.get(path) {
                        Some(WatchStatus::Installed(active_mode))
                            if Some(active_mode) == desired.get(path)
                                && (active_mode == mode
                                    || *active_mode == RecursiveMode::Recursive) => {}
                        Some(WatchStatus::Failed(error)) => {
                            return Err(notify::Error::generic(error));
                        }
                        Some(WatchStatus::Installed(_)) | None => installed = false,
                    }
                }
                if installed {
                    return Ok(());
                }
            }
            readiness
                .changed()
                .await
                .map_err(|_| notify::Error::generic("file watcher backend is unavailable"))?;
        }
    }

    pub(super) fn update(&self, path: &Path, mode: Option<RecursiveMode>) {
        let mut desired = self
            .desired
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(mode) = mode {
            desired.insert(path.to_path_buf(), mode);
        } else {
            desired.remove(path);
        }
        drop(desired);
        match self.wake.try_send(()) {
            Ok(()) | Err(mpsc::TrySendError::Full(())) => {}
            Err(mpsc::TrySendError::Disconnected(())) => {
                warn!("file watcher backend is unavailable");
            }
        }
    }
}

fn reconcile(
    watcher: &mut impl WatchBackend,
    active: &mut HashMap<PathBuf, RecursiveMode>,
    desired: &HashMap<PathBuf, RecursiveMode>,
    activated: impl Fn(&Path),
    deactivating: impl Fn(&Path),
) -> HashMap<PathBuf, String> {
    let mut failures = HashMap::new();
    active.retain(|path, mode| {
        if desired.get(path).is_some_and(|next| *next == *mode) {
            return true;
        }
        deactivating(path);
        match watcher.unwatch(path) {
            Ok(()) => false,
            Err(error) if matches!(error.kind, notify::ErrorKind::WatchNotFound) => false,
            Err(error) => {
                warn!("failed to unwatch {}: {error}", path.display());
                failures.insert(path.clone(), error.to_string());
                true
            }
        }
    });
    for (path, mode) in desired {
        if active.contains_key(path) {
            continue;
        }
        match watcher.watch(path, *mode) {
            Ok(()) => {
                active.insert(path.clone(), *mode);
                // Changes between logical subscription and backend readiness
                // must not leave a reader with a stale cache. This is a coarse
                // invalidation, not a fabricated specific filesystem operation.
                activated(path);
            }
            Err(error) => {
                warn!("failed to watch {}: {error}", path.display());
                failures.insert(path.clone(), error.to_string());
            }
        }
    }
    failures
}

#[cfg(test)]
#[path = "backend_tests.rs"]
mod tests;
