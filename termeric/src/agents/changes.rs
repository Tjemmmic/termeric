//! What an agent changed in a Git working tree since it started.
//!
//! When an agent starts, Termeric records a snapshot of the whole working
//! tree, including untracked files that are not ignored, as a Git tree
//! object. Reviewing compares a fresh snapshot with it. Snapshots are built
//! from an in-memory copy of the index that is never written back, so taking
//! one changes neither the index nor any file; it only adds objects to the
//! repository's object store, where Git can collect them later.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use git2::{Delta, DiffOptions, IndexAddOption, ObjectType, Oid, Repository};

/// Untracked files larger than this are left out of snapshots, so a stray
/// build artifact or disk image is neither hashed nor reviewed.
pub const MAX_SNAPSHOT_FILE_BYTES: u64 = 16 * 1024 * 1024;
/// Working trees with more paths than this are not snapshotted.
pub const MAX_SNAPSHOT_PATHS: usize = 50_000;
/// A file's diff is cut after this much text.
const MAX_FILE_DIFF_BYTES: usize = 1024 * 1024;

/// One terminal's record of where its latest agent started. It travels with
/// the terminal, and outlives the agent so its changes can be reviewed after
/// it exits.
#[derive(Debug, Default)]
pub struct ChangeTracking {
    agent_running: bool,
    baseline: BaselineState,
}

#[derive(Debug, Default)]
enum BaselineState {
    #[default]
    None,
    Capturing(std::sync::mpsc::Receiver<Result<Option<Baseline>, String>>),
    Ready(Baseline),
    Unavailable(String),
}

impl ChangeTracking {
    /// Whether an agent that just appeared needs a baseline.
    pub fn needs_capture(&self, agent_running: bool) -> bool {
        agent_running && !self.agent_running
    }

    pub fn set_agent_running(&mut self, agent_running: bool) {
        self.agent_running = agent_running;
    }

    /// Snapshot `directory` on a worker thread, replacing the previous
    /// agent's baseline.
    pub fn start_capture(&mut self, directory: Option<PathBuf>) {
        let Some(directory) = directory else {
            self.baseline =
                BaselineState::Unavailable("the agent's working directory is unknown".into());
            return;
        };
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let spawned = std::thread::Builder::new()
            .name("termeric-agent-baseline".into())
            .spawn(move || {
                let _ = sender.send(capture(&directory).map_err(|error| format!("{error:#}")));
            });
        self.baseline = match spawned {
            Ok(_) => BaselineState::Capturing(receiver),
            Err(error) => BaselineState::Unavailable(error.to_string()),
        };
    }

    /// Collect a finished capture. Returns whether the state changed.
    pub fn poll(&mut self) -> bool {
        let BaselineState::Capturing(receiver) = &self.baseline else {
            return false;
        };
        self.baseline = match receiver.try_recv() {
            Ok(Ok(Some(baseline))) => BaselineState::Ready(baseline),
            Ok(Ok(None)) => BaselineState::Unavailable("not in a Git repository".into()),
            Ok(Err(error)) => BaselineState::Unavailable(error),
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                BaselineState::Unavailable("the snapshot stopped unexpectedly".into())
            }
        };
        true
    }

    pub fn baseline(&self) -> Option<&Baseline> {
        match &self.baseline {
            BaselineState::Ready(baseline) => Some(baseline),
            _ => None,
        }
    }

    /// Why there is no baseline, if one was attempted.
    pub fn unavailable_reason(&self) -> Option<&str> {
        match &self.baseline {
            BaselineState::Unavailable(reason) => Some(reason),
            _ => None,
        }
    }
}

/// The working tree as it was when an agent started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Baseline {
    pub repo_root: PathBuf,
    tree: Oid,
}

/// How a file differs from the baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    /// Relative to the repository root.
    pub path: PathBuf,
    pub kind: ChangeKind,
    pub added_lines: usize,
    pub removed_lines: usize,
    pub binary: bool,
    /// The index already holds the file exactly as it is now.
    pub staged: bool,
}

/// Snapshot the working tree containing `directory`, or `None` when it is
/// not inside a Git working tree.
pub fn capture(directory: &Path) -> Result<Option<Baseline>> {
    let repo = match Repository::discover(directory) {
        Ok(repo) => repo,
        Err(error) if error.code() == git2::ErrorCode::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let Some(root) = repo.workdir().map(Path::to_path_buf) else {
        return Ok(None);
    };
    let tree = snapshot(&repo)?;
    Ok(Some(Baseline {
        repo_root: root,
        tree,
    }))
}

/// The working tree as a tree object, without touching the on-disk index.
fn snapshot(repo: &Repository) -> Result<Oid> {
    let root = repo
        .workdir()
        .context("the repository has no working tree")?
        .to_path_buf();
    // `repo.index()` loads the index into memory; nothing here writes it
    // back, so the real index is unchanged.
    let mut index = repo.index()?;
    let mut paths = 0usize;
    let mut too_many = false;
    index
        .add_all(
            ["*"],
            IndexAddOption::DEFAULT,
            Some(&mut |path: &Path, _: &[u8]| {
                paths += 1;
                if paths > MAX_SNAPSHOT_PATHS {
                    too_many = true;
                    return -1;
                }
                let large = fs::symlink_metadata(root.join(path)).is_ok_and(|metadata| {
                    metadata.is_file() && metadata.len() > MAX_SNAPSHOT_FILE_BYTES
                });
                i32::from(large)
            }),
        )
        .or_else(|error| {
            if too_many {
                bail!("the working tree has more than {MAX_SNAPSHOT_PATHS} files to compare")
            }
            Err(error.into())
        })?;
    // Files deleted from the working tree leave the snapshot too.
    index.update_all(["*"], None)?;
    Ok(index.write_tree()?)
}

fn open(baseline: &Baseline) -> Result<Repository> {
    Repository::open(&baseline.repo_root)
        .with_context(|| format!("open the repository at {}", baseline.repo_root.display()))
}

fn diff_since<'repo>(
    repo: &'repo Repository,
    baseline: &Baseline,
    path: Option<&Path>,
) -> Result<git2::Diff<'repo>> {
    diff_between(repo, baseline, snapshot(repo)?, path)
}

fn diff_between<'repo>(
    repo: &'repo Repository,
    baseline: &Baseline,
    current: Oid,
    path: Option<&Path>,
) -> Result<git2::Diff<'repo>> {
    let old = repo.find_tree(baseline.tree)?;
    let new = repo.find_tree(current)?;
    let mut options = DiffOptions::new();
    if let Some(path) = path {
        options.pathspec(path).disable_pathspec_match(true);
    }
    Ok(repo.diff_tree_to_tree(Some(&old), Some(&new), Some(&mut options))?)
}

/// Every file that differs from the baseline, in path order.
pub fn changes_since(baseline: &Baseline) -> Result<Vec<ChangedFile>> {
    let repo = open(baseline)?;
    let current = snapshot(&repo)?;
    let diff = diff_between(&repo, baseline, current, None)?;
    let current = repo.find_tree(current)?;
    // Loaded fresh: the snapshot's in-memory changes are not the real index.
    let mut staging = git2::Index::open(&repo.path().join("index"))?;
    staging.read(true)?;
    let mut files = Vec::new();
    for (index, delta) in diff.deltas().enumerate() {
        let kind = match delta.status() {
            Delta::Added => ChangeKind::Added,
            Delta::Deleted => ChangeKind::Deleted,
            _ => ChangeKind::Modified,
        };
        let path = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .context("a changed file has no path")?
            .to_path_buf();
        let patch = git2::Patch::from_diff(&diff, index)?;
        let (_, added_lines, removed_lines) = patch
            .as_ref()
            .map(git2::Patch::line_stats)
            .transpose()?
            .unwrap_or((0, 0, 0));
        let now = current.get_path(&path).ok().map(|entry| entry.id());
        let staged = staging.get_path(&path, 0).map(|entry| entry.id) == now;
        files.push(ChangedFile {
            path,
            kind,
            added_lines,
            removed_lines,
            binary: delta.flags().is_binary(),
            staged,
        });
    }
    Ok(files)
}

/// The unified diff of one file since the baseline.
pub fn file_diff(baseline: &Baseline, path: &Path) -> Result<String> {
    let repo = open(baseline)?;
    let diff = diff_since(&repo, baseline, Some(path))?;
    let mut text = String::new();
    let mut truncated = false;
    diff.print(git2::DiffFormat::Patch, |_, _, line| {
        if text.len() >= MAX_FILE_DIFF_BYTES {
            truncated = true;
            return false;
        }
        if matches!(line.origin(), '+' | '-' | ' ') {
            text.push(line.origin());
        }
        text.push_str(&String::from_utf8_lossy(line.content()));
        true
    })
    .or_else(|error| if truncated { Ok(()) } else { Err(error) })?;
    Ok(text)
}

/// Stage `paths` as they are now, including deletions.
pub fn stage(baseline: &Baseline, paths: &[PathBuf]) -> Result<()> {
    let repo = open(baseline)?;
    let mut index = repo.index()?;
    for path in paths {
        if baseline.repo_root.join(path).symlink_metadata().is_ok() {
            index.add_path(path)?;
        } else {
            index.remove_path(path)?;
        }
    }
    index.write()?;
    Ok(())
}

/// Put `path` back as it was in the baseline: restore its content and mode,
/// or remove it if it did not exist then. Only the working tree changes.
pub fn revert(baseline: &Baseline, path: &Path) -> Result<()> {
    if path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        bail!("{} is not a path inside the repository", path.display());
    }
    let repo = open(baseline)?;
    let target = baseline.repo_root.join(path);
    let tree = repo.find_tree(baseline.tree)?;
    let entry = match tree.get_path(path) {
        Ok(entry) => Some(entry),
        Err(error) if error.code() == git2::ErrorCode::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    if target
        .symlink_metadata()
        .is_ok_and(|metadata| !metadata.is_dir())
    {
        fs::remove_file(&target).with_context(|| format!("remove {}", target.display()))?;
    }
    let Some(entry) = entry else {
        return Ok(());
    };
    if entry.kind() != Some(ObjectType::Blob) {
        bail!("{} was not a file when the agent started", path.display());
    }
    let blob = repo.find_blob(entry.id())?;
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    match entry.filemode() {
        0o120000 => {
            #[cfg(unix)]
            std::os::unix::fs::symlink(
                String::from_utf8_lossy(blob.content()).into_owned(),
                &target,
            )?;
            #[cfg(not(unix))]
            bail!(
                "{} was a symbolic link, which cannot be restored here",
                path.display()
            );
        }
        mode => {
            fs::write(&target, blob.content())
                .with_context(|| format!("write {}", target.display()))?;
            #[cfg(unix)]
            if mode == 0o100755 {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&target, fs::Permissions::from_mode(0o755))?;
            }
            #[cfg(not(unix))]
            let _ = mode;
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A repository with one commit and some pre-existing, uncommitted work.
    fn repository() -> (tempfile::TempDir, Repository) {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        let root = temp.path();
        fs::write(root.join("kept.txt"), "one\ntwo\nthree\n").unwrap();
        fs::write(root.join("script.sh"), "echo hi\n").unwrap();
        fs::set_permissions(root.join("script.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(root.join(".gitignore"), "target/\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_all(["*"], IndexAddOption::DEFAULT, None).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let signature = git2::Signature::now("Test", "test@example.com").unwrap();
        repo.commit(Some("HEAD"), &signature, &signature, "start", &tree, &[])
            .unwrap();
        drop(tree);
        // Work the person had in progress before the agent started.
        fs::write(root.join("notes.txt"), "mine\n").unwrap();
        fs::create_dir(root.join("target")).unwrap();
        fs::write(root.join("target/out.bin"), "ignored").unwrap();
        (temp, repo)
    }

    fn index_bytes(root: &Path) -> Vec<u8> {
        fs::read(root.join(".git/index")).unwrap()
    }

    #[test]
    fn only_what_changed_after_the_snapshot_is_reported() {
        let (temp, _repo) = repository();
        let root = temp.path();
        let index_before = index_bytes(root);
        let baseline = capture(&root.join("target")).unwrap().unwrap();
        assert_eq!(
            baseline.repo_root.canonicalize().unwrap(),
            root.canonicalize().unwrap()
        );
        assert_eq!(index_bytes(root), index_before, "the index is untouched");
        assert!(changes_since(&baseline).unwrap().is_empty());

        fs::write(root.join("kept.txt"), "one\nTWO\nthree\nfour\n").unwrap();
        fs::write(root.join("new.rs"), "fn main() {}\n").unwrap();
        fs::remove_file(root.join("notes.txt")).unwrap();
        fs::write(root.join("target/more.bin"), "still ignored").unwrap();
        let changes = changes_since(&baseline).unwrap();
        assert_eq!(
            index_bytes(root),
            index_before,
            "reviewing leaves the index alone"
        );
        let summary: Vec<_> = changes
            .iter()
            .map(|file| {
                (
                    file.path.to_str().unwrap(),
                    file.kind,
                    file.added_lines,
                    file.removed_lines,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("kept.txt", ChangeKind::Modified, 2, 1),
                ("new.rs", ChangeKind::Added, 1, 0),
                ("notes.txt", ChangeKind::Deleted, 0, 1),
            ]
        );
        let diff = file_diff(&baseline, Path::new("kept.txt")).unwrap();
        assert!(diff.contains("-two\n") && diff.contains("+TWO\n") && diff.contains("+four\n"));
        assert!(!diff.contains("new.rs"), "one file's diff only");
    }

    #[test]
    fn reverting_restores_content_mode_and_absence() {
        let (temp, _repo) = repository();
        let root = temp.path();
        let baseline = capture(root).unwrap().unwrap();
        fs::write(root.join("script.sh"), "rm -rf /\n").unwrap();
        fs::set_permissions(root.join("script.sh"), fs::Permissions::from_mode(0o644)).unwrap();
        fs::remove_file(root.join("notes.txt")).unwrap();
        fs::create_dir_all(root.join("src/deep")).unwrap();
        fs::write(root.join("src/deep/new.rs"), "added").unwrap();

        for path in ["script.sh", "notes.txt", "src/deep/new.rs"] {
            revert(&baseline, Path::new(path)).unwrap();
        }
        assert_eq!(
            fs::read_to_string(root.join("script.sh")).unwrap(),
            "echo hi\n"
        );
        assert_eq!(
            fs::metadata(root.join("script.sh"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        assert_eq!(
            fs::read_to_string(root.join("notes.txt")).unwrap(),
            "mine\n"
        );
        assert!(!root.join("src/deep/new.rs").exists());
        assert!(changes_since(&baseline).unwrap().is_empty());

        for escape in ["../outside", "/etc/passwd", "a/../../b"] {
            assert!(revert(&baseline, Path::new(escape)).is_err(), "{escape}");
        }
    }

    #[test]
    fn staging_records_edits_and_deletions_in_the_index() {
        let (temp, repo) = repository();
        let root = temp.path();
        let baseline = capture(root).unwrap().unwrap();
        fs::write(root.join("kept.txt"), "changed\n").unwrap();
        fs::remove_file(root.join("script.sh")).unwrap();
        let staged = |baseline: &Baseline| {
            changes_since(baseline)
                .unwrap()
                .into_iter()
                .map(|file| (file.path.to_string_lossy().into_owned(), file.staged))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            staged(&baseline),
            [("kept.txt".into(), false), ("script.sh".into(), false)]
        );
        stage(&baseline, &["kept.txt".into(), "script.sh".into()]).unwrap();
        assert_eq!(
            staged(&baseline),
            [("kept.txt".into(), true), ("script.sh".into(), true)]
        );
        let mut index = repo.index().unwrap();
        index.read(true).unwrap();
        let kept = index.get_path(Path::new("kept.txt"), 0).unwrap();
        assert_eq!(repo.find_blob(kept.id).unwrap().content(), b"changed\n");
        assert!(index.get_path(Path::new("script.sh"), 0).is_none());
    }

    #[test]
    fn each_agent_start_captures_one_baseline_that_outlives_the_agent() {
        let (temp, _repo) = repository();
        let mut tracking = ChangeTracking::default();
        assert!(!tracking.needs_capture(false));
        assert!(tracking.needs_capture(true));
        tracking.set_agent_running(true);
        tracking.start_capture(Some(temp.path().to_path_buf()));
        assert_eq!(tracking.baseline(), None, "not until the snapshot finishes");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !tracking.poll() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let first = tracking.baseline().unwrap().clone();
        assert!(!tracking.needs_capture(true), "still the same agent");

        // The agent exits: its baseline stays for review.
        tracking.set_agent_running(false);
        assert_eq!(tracking.baseline(), Some(&first));
        // A later agent gets its own.
        assert!(tracking.needs_capture(true));
        tracking.start_capture(None);
        assert_eq!(tracking.baseline(), None);
        assert_eq!(
            tracking.unavailable_reason(),
            Some("the agent's working directory is unknown")
        );
    }

    #[test]
    fn outside_a_repository_there_is_no_baseline() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(capture(temp.path()).unwrap(), None);
    }

    #[test]
    fn large_untracked_files_are_left_out() {
        let (temp, _repo) = repository();
        let root = temp.path();
        let baseline = capture(root).unwrap().unwrap();
        let big = fs::File::create(root.join("disk.img")).unwrap();
        big.set_len(MAX_SNAPSHOT_FILE_BYTES + 1).unwrap();
        fs::write(root.join("small.txt"), "x").unwrap();
        let paths: Vec<_> = changes_since(&baseline)
            .unwrap()
            .into_iter()
            .map(|file| file.path)
            .collect();
        assert_eq!(paths, [PathBuf::from("small.txt")]);
    }
}
