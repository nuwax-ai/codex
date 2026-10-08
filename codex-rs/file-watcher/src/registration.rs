//! RAII path registrations.

use super::FileWatcher;
use super::SubscriberId;
use super::SubscriberWatchKey;

/// RAII guard for a set of active path registrations.
pub struct WatchRegistration {
    pub(super) file_watcher: std::sync::Weak<FileWatcher>,
    pub(super) subscriber_id: SubscriberId,
    pub(super) watched_paths: Vec<SubscriberWatchKey>,
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
