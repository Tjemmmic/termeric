//! Cancellable-generation background Git state watcher.

use super::{GitRepo, GitStatus, filter_branches};
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
    branches: Vec<String>,
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
                branches: Vec::new(),
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
                    changed |= self.status != snapshot.status
                        || self.cached_completions != snapshot.branches;
                    self.status = snapshot.status;
                    self.cached_completions = snapshot.branches;
                    self.last_error = None;
                }
                Err(error) => {
                    let message = format!("{}: {error}", result.path.display());
                    if self.last_error.as_deref() != Some(&message) {
                        self.pending_error = Some(message.clone());
                    }
                    self.last_error = Some(message);
                    changed |= self.status.take().is_some() || !self.cached_completions.is_empty();
                    self.cached_completions.clear();
                }
            }
        }
        changed
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
                branches: vec![name],
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
