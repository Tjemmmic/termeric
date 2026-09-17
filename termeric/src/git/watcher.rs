//! Cancellable-generation background Git state watcher.

use super::{GitRepo, GitStatus, MAX_BRANCH_NAME_BYTES, MAX_BRANCH_NAMES, filter_branches};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

type WakeCallback = Arc<dyn Fn() + Send + Sync>;

#[derive(Debug)]
struct ScanRequest {
    generation: u64,
    path: PathBuf,
}

#[derive(Debug)]
struct ScanResult {
    generation: u64,
    path: PathBuf,
    snapshot: Result<GitSnapshot, String>,
}

#[derive(Debug)]
struct GitSnapshot {
    status: Option<GitStatus>,
    /// None disables completion after a collection limit; status stays usable.
    branches: Option<Vec<String>>,
}

trait GitBackend: Send + 'static {
    fn scan(&self, path: &Path) -> Result<GitSnapshot, String>;
}

struct LibGit2Backend;

impl GitBackend for LibGit2Backend {
    fn scan(&self, path: &Path) -> Result<GitSnapshot, String> {
        let Some(repo) = GitRepo::discover(path).map_err(|error| error.message().to_string())?
        else {
            return Ok(GitSnapshot {
                status: None,
                branches: Some(Vec::new()),
            });
        };
        Ok(GitSnapshot {
            status: Some(repo.status().map_err(|error| error.message().to_string())?),
            branches: repo
                .branch_names()
                .map_err(|error| error.message().to_string())?,
        })
    }
}

/// Background Git state tracker. Repository access is owned by one worker;
/// callers only schedule scans, drain immutable snapshots, and filter cached
/// branch names.
pub struct GitWatcher {
    request_tx: Sender<ScanRequest>,
    result_rx: Receiver<ScanResult>,
    last_dir: Option<PathBuf>,
    generation: u64,
    status: Option<GitStatus>,
    cached_completions: Vec<String>,
    completions_limited: bool,
    last_poll: Instant,
    poll_interval: Duration,
    worker_failed: bool,
    scan_in_flight: bool,
    refresh_pending: bool,
    last_error: Option<String>,
    pending_error: Option<String>,
}

impl GitWatcher {
    pub fn new(wake: Option<WakeCallback>) -> Self {
        Self::with_backend(LibGit2Backend, wake)
    }

    fn with_backend(backend: impl GitBackend, wake: Option<WakeCallback>) -> Self {
        let (request_tx, request_rx) = mpsc::channel::<ScanRequest>();
        let (result_tx, result_rx) = mpsc::channel::<ScanResult>();
        std::thread::Builder::new()
            .name("termeric-git".into())
            .spawn(move || git_worker(backend, request_rx, result_tx, wake))
            .expect("failed to spawn Git worker");

        Self {
            request_tx,
            result_rx,
            last_dir: None,
            generation: 0,
            status: None,
            cached_completions: Vec::new(),
            completions_limited: false,
            last_poll: Instant::now() - Duration::from_secs(10),
            poll_interval: Duration::from_secs(2),
            worker_failed: false,
            scan_in_flight: false,
            refresh_pending: false,
            last_error: None,
            pending_error: None,
        }
    }

    /// Drain completed snapshots and schedule a scan when the cwd changes or
    /// the debounce interval expires. This method never opens a repository.
    pub fn update(&mut self, working_dir: Option<&str>) -> bool {
        let dir = working_dir.map(PathBuf::from);
        let dir_changed = self.last_dir != dir;
        let mut changed = self.drain_results();

        if dir_changed {
            self.generation = self.generation.wrapping_add(1);
            self.last_dir = dir.clone();
            changed |= self.status.take().is_some() || !self.cached_completions.is_empty();
            self.cached_completions.clear();
            self.completions_limited = false;
            self.refresh_pending = dir.is_some();
        }

        if dir_changed || self.last_poll.elapsed() >= self.poll_interval {
            self.last_poll = Instant::now();
            if !dir_changed {
                // A periodic refresh for the same cwd does not make an
                // in-flight snapshot stale. Advancing the generation here
                // would permanently starve repositories whose scan takes
                // longer than the polling interval. Keep one follow-up
                // pending; cwd changes and explicit invalidation remain the
                // cancellation boundaries.
                self.refresh_pending = dir.is_some();
            }
        }

        if self.refresh_pending
            && !self.scan_in_flight
            && let Some(path) = dir
        {
            if !self.worker_failed
                && self
                    .request_tx
                    .send(ScanRequest {
                        generation: self.generation,
                        path,
                    })
                    .is_err()
            {
                self.worker_failed = true;
                log::error!("Git background worker stopped unexpectedly");
            } else if !self.worker_failed {
                self.scan_in_flight = true;
                self.refresh_pending = false;
            }
        }
        changed
    }

    fn drain_results(&mut self) -> bool {
        let mut changed = false;
        while let Ok(result) = self.result_rx.try_recv() {
            self.scan_in_flight = false;
            if result.generation != self.generation || Some(&result.path) != self.last_dir.as_ref()
            {
                log::trace!(
                    "Discarding stale Git result generation {} for {}",
                    result.generation,
                    result.path.display()
                );
                continue;
            }
            match result.snapshot {
                Ok(snapshot) => {
                    let limited = snapshot.branches.is_none();
                    let branches = snapshot.branches.unwrap_or_default();
                    changed |= self.status != snapshot.status
                        || self.cached_completions != branches
                        || self.completions_limited != limited;
                    self.status = snapshot.status;
                    self.cached_completions = branches;
                    self.completions_limited = limited;
                    if limited {
                        self.report_error(format!(
                            "{}: branch completion disabled: collection exceeds {} names or {} MiB of name text; use shell completion. Git status remains available",
                            result.path.display(), MAX_BRANCH_NAMES, MAX_BRANCH_NAME_BYTES / (1024 * 1024),
                        ));
                    } else {
                        self.last_error = None;
                        self.pending_error = None;
                    }
                }
                Err(error) => {
                    self.report_error(format!("{}: {error}", result.path.display()));
                    changed |= self.status.take().is_some() || !self.cached_completions.is_empty();
                    self.cached_completions.clear();
                    self.completions_limited = false;
                }
            }
        }
        changed
    }

    fn report_error(&mut self, message: String) {
        if self.last_error.as_deref() != Some(&message) {
            self.pending_error = Some(message.clone());
        }
        self.last_error = Some(message);
    }

    pub fn completions_limited(&self) -> bool {
        self.completions_limited
    }

    pub fn next_poll_deadline(&self) -> Instant {
        self.last_poll + self.poll_interval
    }

    pub fn status(&self) -> Option<&GitStatus> {
        self.status.as_ref()
    }

    pub fn branch(&self) -> Option<&str> {
        self.status.as_ref()?.branch.as_deref()
    }

    pub fn is_dirty(&self) -> bool {
        self.status.as_ref().is_some_and(|status| status.dirty)
    }

    /// Filter the immutable branch snapshot already published by the worker.
    pub fn complete_branch(&self, partial: &str) -> Vec<String> {
        filter_branches(&self.cached_completions, partial)
    }

    pub fn take_error(&mut self) -> Option<String> {
        self.pending_error.take()
    }

    /// Cancel any in-flight generation and force a debounced refresh.
    pub fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.last_poll = Instant::now() - self.poll_interval;
        self.refresh_pending = self.last_dir.is_some();
    }
}

fn git_worker(
    backend: impl GitBackend,
    request_rx: Receiver<ScanRequest>,
    result_tx: Sender<ScanResult>,
    wake: Option<WakeCallback>,
) {
    while let Ok(request) = request_rx.recv() {
        // The watcher permits only one queued/in-flight scan. Cwd churn is
        // retained as one replaceable pending generation on the caller side.
        let snapshot = backend.scan(&request.path);
        if result_tx
            .send(ScanResult {
                generation: request.generation,
                path: request.path,
                snapshot,
            })
            .is_err()
        {
            break;
        }
        if let Some(wake) = &wake {
            wake();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct ControlledBackend {
        started: Sender<PathBuf>,
        release: Mutex<Receiver<()>>,
    }

    impl GitBackend for ControlledBackend {
        fn scan(&self, path: &Path) -> Result<GitSnapshot, String> {
            self.started.send(path.to_path_buf()).unwrap();
            self.release.lock().unwrap().recv().unwrap();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            Ok(GitSnapshot {
                status: Some(GitStatus {
                    branch: Some(name.clone()),
                    ..GitStatus::default()
                }),
                branches: Some(vec![name]),
            })
        }
    }

    #[test]
    fn slow_scans_never_block_and_stale_cwd_results_are_discarded() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (wake_tx, wake_rx) = mpsc::channel();
        let wake = Arc::new(move || {
            let _ = wake_tx.send(());
        });
        let mut watcher = GitWatcher::with_backend(
            ControlledBackend {
                started: started_tx,
                release: Mutex::new(release_rx),
            },
            Some(wake),
        );

        let started_at = Instant::now();
        assert!(!watcher.update(Some("/tmp/old")));
        assert!(started_at.elapsed() < Duration::from_millis(20));
        assert_eq!(
            started_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            PathBuf::from("/tmp/old")
        );

        assert!(!watcher.update(Some("/tmp/new")));
        release_tx.send(()).unwrap();
        wake_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(!watcher.update(Some("/tmp/new")));
        assert!(watcher.status().is_none());

        assert_eq!(
            started_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            PathBuf::from("/tmp/new")
        );
        release_tx.send(()).unwrap();
        wake_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(watcher.update(Some("/tmp/new")));
        assert_eq!(watcher.branch(), Some("new"));
        assert_eq!(watcher.complete_branch("n"), vec!["new"]);
    }

    #[test]
    fn slow_same_cwd_scan_survives_poll_deadline_and_queues_one_followup() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (wake_tx, wake_rx) = mpsc::channel();
        let wake = Arc::new(move || {
            let _ = wake_tx.send(());
        });
        let mut watcher = GitWatcher::with_backend(
            ControlledBackend {
                started: started_tx,
                release: Mutex::new(release_rx),
            },
            Some(wake),
        );

        assert!(!watcher.update(Some("/tmp/slow")));
        assert_eq!(
            started_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            PathBuf::from("/tmp/slow")
        );
        let generation = watcher.generation;

        watcher.last_poll = Instant::now() - watcher.poll_interval;
        assert!(!watcher.update(Some("/tmp/slow")));
        assert_eq!(watcher.generation, generation);
        assert!(watcher.scan_in_flight);
        assert!(watcher.refresh_pending);

        release_tx.send(()).unwrap();
        wake_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(watcher.update(Some("/tmp/slow")));
        assert_eq!(watcher.branch(), Some("slow"));

        // The overdue refresh is coalesced into exactly one follow-up scan.
        assert_eq!(
            started_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            PathBuf::from("/tmp/slow")
        );
        assert!(started_rx.try_recv().is_err());
        release_tx.send(()).unwrap();
        wake_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(!watcher.update(Some("/tmp/slow")));
    }

    struct FailingBackend;

    #[test]
    fn completion_limits_preserve_status_warn_once_and_recover() {
        let mut watcher = GitWatcher::with_backend(FailingBackend, None);
        let (tx, rx) = mpsc::channel();
        watcher.result_rx = rx;
        watcher.last_dir = Some(PathBuf::from("/tmp/limited-repository"));
        let status = GitStatus {
            branch: Some("main".into()),
            dirty: true,
            ahead: 2,
            behind: 1,
        };
        let publish = |branches| {
            tx.send(ScanResult {
                generation: 0,
                path: PathBuf::from("/tmp/limited-repository"),
                snapshot: Ok(GitSnapshot {
                    status: Some(status.clone()),
                    branches,
                }),
            })
            .unwrap();
        };
        publish(Some(vec!["main".into()]));
        assert!(watcher.drain_results());
        assert_eq!(watcher.complete_branch(""), ["main"]);
        for repetition in 0..2 {
            publish(None);
            assert_eq!(watcher.drain_results(), repetition == 0);
            assert_eq!(watcher.status(), Some(&status));
            assert!(watcher.completions_limited());
            assert!(watcher.complete_branch("").is_empty());
            assert_eq!(watcher.cached_completions.capacity(), 0);
            let notice = watcher.take_error();
            assert_eq!(notice.is_some(), repetition == 0);
            if let Some(notice) = notice {
                assert!(notice.contains("/tmp/limited-repository"));
                assert!(notice.contains("16384 names or 4 MiB"));
                assert!(notice.contains("use shell completion"));
            }
        }
        publish(Some(vec!["main".into(), "new".into()]));
        assert!(watcher.drain_results());
        assert!(!watcher.completions_limited());
        assert_eq!(watcher.complete_branch("new"), ["new"]);
        assert!(watcher.take_error().is_none());
        publish(None);
        assert!(watcher.drain_results());
        assert!(watcher.take_error().is_some());

        // A new generation rejects old cap failures just like stale successes.
        watcher.generation = 1;
        watcher.completions_limited = false;
        watcher.cached_completions = vec!["current".into()];
        publish(None);
        assert!(!watcher.drain_results());
        assert!(!watcher.completions_limited());
        assert_eq!(watcher.complete_branch(""), ["current"]);
        assert!(watcher.take_error().is_none());
    }

    #[test]
    fn real_backend_limits_packed_refs_without_losing_status_and_recovers() {
        use std::fmt::Write;
        let directory = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(directory.path()).unwrap();
        let tree_id = repo.index().unwrap().write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let signature =
            git2::Signature::new("Fixture", "fixture@example.invalid", &git2::Time::new(1, 0))
                .unwrap();
        let commit = repo
            .commit(None, &signature, &signature, "fixture", &tree, &[])
            .unwrap();
        let mut refs = String::from("# pack-refs with: sorted\n");
        for i in 0..=MAX_BRANCH_NAMES {
            writeln!(refs, "{commit} refs/heads/branch-{i:05}").unwrap();
        }
        let packed_refs = repo.path().join("packed-refs");
        std::fs::write(&packed_refs, refs).unwrap();
        repo.set_head("refs/heads/branch-00000").unwrap();
        drop(tree);
        drop(repo);

        let snapshot = LibGit2Backend.scan(directory.path()).unwrap();
        let status = snapshot.status.unwrap();
        assert_eq!(status.branch.as_deref(), Some("branch-00000"));
        assert!(!status.dirty);
        assert!(snapshot.branches.is_none());

        // The real asynchronous publication path preserves status too.
        let (wake_tx, wake_rx) = mpsc::channel();
        let mut watcher = GitWatcher::with_backend(
            LibGit2Backend,
            Some(Arc::new(move || {
                let _ = wake_tx.send(());
            })),
        );
        let cwd = directory.path().to_str().unwrap();
        watcher.update(Some(cwd));
        wake_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(watcher.update(Some(cwd)));
        assert_eq!(watcher.status(), Some(&status));
        assert!(watcher.completions_limited());
        assert!(watcher.complete_branch("").is_empty());
        assert!(watcher.take_error().is_some());

        // Exceed bytes before reaching the name-count limit. Nested names are
        // valid packed refs without requiring oversized filesystem components.
        let mut refs = format!("# pack-refs with: sorted\n{commit} refs/heads/branch-00000\n");
        let prefix = "part/".repeat(210);
        for i in 0..4096 {
            writeln!(refs, "{commit} refs/heads/{prefix}branch-{i:05}").unwrap();
        }
        std::fs::write(&packed_refs, refs).unwrap();
        let byte_limited = LibGit2Backend.scan(directory.path()).unwrap();
        assert_eq!(byte_limited.status, Some(status.clone()));
        assert!(byte_limited.branches.is_none());

        std::fs::write(&packed_refs, format!("{commit} refs/heads/branch-00000\n")).unwrap();
        let recovered = LibGit2Backend.scan(directory.path()).unwrap();
        assert_eq!(recovered.status, Some(status.clone()));
        assert_eq!(recovered.branches.unwrap(), ["branch-00000"]);
        watcher.invalidate();
        watcher.update(Some(cwd));
        wake_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(watcher.update(Some(cwd)));
        assert_eq!(watcher.status(), Some(&status));
        assert!(!watcher.completions_limited());
        assert_eq!(watcher.complete_branch(""), ["branch-00000"]);
        assert!(watcher.take_error().is_none());
    }

    impl GitBackend for FailingBackend {
        fn scan(&self, _path: &Path) -> Result<GitSnapshot, String> {
            Err("injected repository failure".into())
        }
    }

    #[test]
    fn worker_failure_is_published_once_with_repository_context() {
        let (wake_tx, wake_rx) = mpsc::channel();
        let mut watcher = GitWatcher::with_backend(
            FailingBackend,
            Some(Arc::new(move || {
                let _ = wake_tx.send(());
            })),
        );
        watcher.update(Some("/tmp/failing-repository"));
        wake_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        watcher.update(Some("/tmp/failing-repository"));
        let error = watcher.take_error().unwrap();
        assert!(error.contains("/tmp/failing-repository"));
        assert!(error.contains("injected repository failure"));
        assert!(watcher.take_error().is_none());
    }
}
