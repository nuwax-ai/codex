//! RAII subscriptions and asynchronous platform-watch readiness.

use super::FileWatcher;
use super::SubscriberId;
use super::SubscriberWatchKey;
use super::backend;
use notify::RecursiveMode;
use std::path::PathBuf;

/// RAII guard for a set of active path registrations.
pub struct WatchRegistration {
    pub(super) file_watcher: std::sync::Weak<FileWatcher>,
    pub(super) subscriber_id: SubscriberId,
    pub(super) watched_paths: Vec<SubscriberWatchKey>,
}

struct RegistrationReadiness {
    backend: backend::BackendController,
    paths: Vec<(PathBuf, RecursiveMode)>,
}

impl WatchRegistration {
    /// Waits for the registered paths to be installed, or returns the first
    /// backend installation error. No watcher locks are held while waiting.
    pub async fn ready(&self) -> notify::Result<()> {
        if self.watched_paths.is_empty() {
            return Ok(());
        }
        loop {
            let Some(snapshot) = self.readiness_snapshot()? else {
                return Ok(());
            };
            let result = snapshot.backend.ready(&snapshot.paths).await;
            let Some(current) = self.readiness_snapshot()? else {
                return Ok(());
            };
            if current.paths == snapshot.paths {
                return result;
            }
        }
    }

    fn readiness_snapshot(&self) -> notify::Result<Option<RegistrationReadiness>> {
        let file_watcher = self
            .file_watcher
            .upgrade()
            .ok_or_else(|| notify::Error::generic("file watcher is no longer available"))?;
        let Some(inner) = file_watcher.inner.as_ref() else {
            return Ok(None);
        };
        let (backend, paths) = {
            let state = file_watcher
                .state
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let subscriber = state
                .subscribers
                .get(&self.subscriber_id)
                .ok_or_else(|| notify::Error::generic("file watcher subscriber was removed"))?;
            let paths = self
                .watched_paths
                .iter()
                .map(|key| {
                    let watch = subscriber.watched_paths.get(key).ok_or_else(|| {
                        notify::Error::generic("file watcher registration was removed")
                    })?;
                    let mode = if watch.actual.recursive {
                        RecursiveMode::Recursive
                    } else {
                        RecursiveMode::NonRecursive
                    };
                    Ok((watch.actual.path.clone(), mode))
                })
                .collect::<notify::Result<Vec<_>>>()?;
            let backend = inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .backend
                .clone();
            (backend, paths)
        };
        Ok(Some(RegistrationReadiness { backend, paths }))
    }
}

impl Default for WatchRegistration {
    fn default() -> Self {
        Self {
            file_watcher: std::sync::Weak::new(),
            subscriber_id: 0,
            watched_paths: Vec::new(),
        }
    }
}

impl Drop for WatchRegistration {
    fn drop(&mut self) {
        if let Some(file_watcher) = self.file_watcher.upgrade() {
            file_watcher.unregister_paths(self.subscriber_id, &self.watched_paths);
        }
    }
}
