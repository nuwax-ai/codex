use super::*;
use pretty_assertions::assert_eq;

#[tokio::test(flavor = "current_thread")]
async fn registration_readiness_follows_a_missing_target_from_ancestor_to_file() {
    struct MigratingBackend {
        entered: mpsc::SyncSender<()>,
        release: mpsc::Receiver<()>,
        installed: mpsc::SyncSender<PathBuf>,
        blocked_once: bool,
    }

    impl WatchBackend for MigratingBackend {
        fn watch(&mut self, path: &Path, _mode: RecursiveMode) -> notify::Result<()> {
            if !self.blocked_once {
                self.blocked_once = true;
                self.entered.send(()).unwrap();
                self.release.recv().unwrap();
            } else {
                self.installed.send(path.to_path_buf()).unwrap();
            }
            Ok(())
        }

        fn unwatch(&mut self, _path: &Path) -> notify::Result<()> {
            Ok(())
        }
    }

    let temp = tempfile::tempdir().expect("fixture directory");
    let target = temp.path().join("initially-missing.txt");
    let (entered, entered_rx) = mpsc::sync_channel(1);
    let (release, release_rx) = mpsc::sync_channel(1);
    let (installed, installed_rx) = mpsc::sync_channel(1);
    let (raw_tx, raw_rx) = tokio::sync::mpsc::unbounded_channel();
    let backend = BackendController::spawn(
        MigratingBackend {
            entered,
            release: release_rx,
            installed,
            blocked_once: false,
        },
        move |path| {
            let _ = raw_tx.send(crate::RawEvent::Ready(path.to_path_buf()));
        },
    )
    .expect("backend thread");
    let watcher = Arc::new(crate::FileWatcher {
        inner: Some(Arc::new(Mutex::new(crate::FileWatcherInner { backend }))),
        state: Arc::new(std::sync::RwLock::new(crate::WatchState::default())),
    });
    watcher.spawn_event_loop_for_test(raw_rx);
    let (subscriber, _receiver) = watcher.add_subscriber();
    let registration = Arc::new(subscriber.register_paths(vec![crate::WatchPath {
        path: target.clone(),
        recursive: false,
    }]));
    entered_rx
        .recv_timeout(Duration::from_secs(/*secs*/ 2))
        .expect("ancestor installation is blocked");
    let pending_registration = Arc::clone(&registration);
    let pending = tokio::spawn(async move { pending_registration.ready().await });
    tokio::task::yield_now().await;
    assert!(!pending.is_finished());

    std::fs::write(&target, "created during installation").expect("create target");
    watcher.send_paths_for_test(vec![target.clone()]).await;
    release.send(()).expect("release ancestor installation");
    tokio::time::timeout(Duration::from_secs(/*secs*/ 2), pending)
        .await
        .expect("readiness follows the current actual path")
        .expect("readiness task")
        .expect("target installation succeeds");
    assert_eq!(
        installed_rx
            .recv_timeout(Duration::from_secs(/*secs*/ 2))
            .expect("file is installed after ancestor migration"),
        target
    );
}

#[derive(Debug, PartialEq)]
enum Call {
    Watch(PathBuf, RecursiveMode),
    Unwatch(PathBuf),
}

#[derive(Default)]
struct FailingBackend {
    watch_failures: usize,
    unwatch_failures: usize,
    calls: Vec<Call>,
}

impl WatchBackend for FailingBackend {
    fn watch(&mut self, path: &Path, mode: RecursiveMode) -> notify::Result<()> {
        self.calls.push(Call::Watch(path.to_path_buf(), mode));
        if self.watch_failures > 0 {
            self.watch_failures -= 1;
            return Err(notify::Error::generic("injected watch failure"));
        }
        Ok(())
    }

    fn unwatch(&mut self, path: &Path) -> notify::Result<()> {
        self.calls.push(Call::Unwatch(path.to_path_buf()));
        if self.unwatch_failures > 0 {
            self.unwatch_failures -= 1;
            return Err(notify::Error::generic("injected unwatch failure"));
        }
        Ok(())
    }
}

#[test]
fn failed_operations_do_not_claim_success_and_retry_the_current_mode() {
    let root = PathBuf::from("fixture");
    let desired = HashMap::from([(root.clone(), RecursiveMode::NonRecursive)]);
    let mut active = HashMap::new();
    let mut watcher = FailingBackend {
        watch_failures: 1,
        ..Default::default()
    };
    assert!(
        !reconcile(
            &mut watcher,
            &mut active,
            &desired,
            |_| panic!("failed watch activated"),
            |_| {}
        )
        .is_empty()
    );
    assert_eq!(active, HashMap::new());
    let activated = Mutex::new(Vec::new());
    assert!(
        reconcile(
            &mut watcher,
            &mut active,
            &desired,
            |path| activated.lock().unwrap().push(path.to_path_buf()),
            |_| {}
        )
        .is_empty()
    );
    assert_eq!(active, desired);
    assert_eq!(*activated.lock().unwrap(), vec![root.clone()]);

    watcher.unwatch_failures = 1;
    let changed = HashMap::from([(root.clone(), RecursiveMode::Recursive)]);
    assert!(
        !reconcile(
            &mut watcher,
            &mut active,
            &changed,
            |_| panic!("failed removal must not add"),
            |_| {}
        )
        .is_empty()
    );
    assert_eq!(active, desired);
    assert!(reconcile(&mut watcher, &mut active, &changed, |_| {}, |_| {}).is_empty());
    assert_eq!(active, changed);
    assert_eq!(
        watcher.calls,
        vec![
            Call::Watch(root.clone(), RecursiveMode::NonRecursive),
            Call::Watch(root.clone(), RecursiveMode::NonRecursive),
            Call::Unwatch(root.clone()),
            Call::Unwatch(root.clone()),
            Call::Watch(root, RecursiveMode::Recursive),
        ]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn blocked_unwatch_keeps_runtime_responsive_and_coalesces_updates() {
    struct BlockedBackend {
        entered: mpsc::SyncSender<()>,
        release: mpsc::Receiver<()>,
        dropped: mpsc::SyncSender<()>,
    }
    impl WatchBackend for BlockedBackend {
        fn watch(&mut self, _path: &Path, _mode: RecursiveMode) -> notify::Result<()> {
            Ok(())
        }
        fn unwatch(&mut self, _path: &Path) -> notify::Result<()> {
            self.entered.send(()).unwrap();
            self.release.recv().unwrap();
            Ok(())
        }
    }
    impl Drop for BlockedBackend {
        fn drop(&mut self) {
            let _ = self.dropped.send(());
        }
    }
    let (entered, entered_rx) = mpsc::sync_channel(1);
    let (release, release_rx) = mpsc::sync_channel(1);
    let (dropped, dropped_rx) = mpsc::sync_channel(1);
    let (active, active_rx) = mpsc::sync_channel(1);
    let backend = BackendController::spawn(
        BlockedBackend {
            entered,
            release: release_rx,
            dropped,
        },
        move |_| {
            active.send(()).unwrap();
        },
    )
    .unwrap();
    let root = PathBuf::from("initial");
    backend.update(&root, Some(RecursiveMode::NonRecursive));
    active_rx
        .recv_timeout(Duration::from_secs(/*secs*/ 2))
        .unwrap();
    backend.update(&root, /*mode*/ None);
    entered_rx
        .recv_timeout(Duration::from_secs(/*secs*/ 2))
        .unwrap();
    backend.update(&root, Some(RecursiveMode::NonRecursive));
    assert!(
        tokio::time::timeout(
            Duration::from_millis(/*millis*/ 20),
            backend.ready(&[(root.clone(), RecursiveMode::NonRecursive)]),
        )
        .await
        .is_err(),
        "in-flight unwatch cannot certify readiness"
    );
    backend.update(&root, /*mode*/ None);
    for n in 0..10_000 {
        let path = PathBuf::from(format!("pending-{n}"));
        backend.update(&path, Some(RecursiveMode::Recursive));
        backend.update(&path, /*mode*/ None);
    }
    assert_eq!(*backend.desired.lock().unwrap(), HashMap::new());
    tokio::time::timeout(
        Duration::from_secs(/*secs*/ 1),
        tokio::time::sleep(Duration::from_millis(/*millis*/ 10)),
    )
    .await
    .unwrap();
    drop(backend);
    release.send(()).unwrap();
    dropped_rx
        .recv_timeout(Duration::from_secs(/*secs*/ 2))
        .unwrap();
}

#[test]
fn worker_retries_a_failed_install_and_emits_activation_without_new_registration() {
    let (activated, receiver) = mpsc::sync_channel(1);
    let backend = BackendController::spawn(
        FailingBackend {
            watch_failures: 1,
            ..Default::default()
        },
        move |path| {
            activated.send(path.to_path_buf()).unwrap();
        },
    )
    .unwrap();
    let path = PathBuf::from("retry");
    backend.update(&path, Some(RecursiveMode::Recursive));
    assert_eq!(
        receiver
            .recv_timeout(Duration::from_secs(/*secs*/ 2))
            .unwrap(),
        path
    );
}

#[tokio::test]
async fn readiness_reports_failed_install_instead_of_claiming_success() {
    let backend = BackendController::spawn(
        FailingBackend {
            watch_failures: usize::MAX,
            ..Default::default()
        },
        |_| panic!("failed installation must not activate"),
    )
    .expect("backend thread");
    let path = PathBuf::from("missing");
    backend.update(&path, Some(RecursiveMode::Recursive));
    let result = tokio::time::timeout(
        Duration::from_secs(/*secs*/ 2),
        backend.ready(&[(path, RecursiveMode::Recursive)]),
    )
    .await
    .expect("backend failure must be reported promptly");
    assert!(
        result
            .expect_err("failed watch is not ready")
            .to_string()
            .contains("injected watch failure")
    );
}
