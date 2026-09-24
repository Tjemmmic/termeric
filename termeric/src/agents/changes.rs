//! What an agent changed in a Git working tree since it started.
//!
//! When an agent starts, Termeric records a snapshot of the whole working
//! tree, including untracked files that are not ignored, as a Git tree
//! object. Reviewing compares a fresh snapshot with it. Snapshots are built
//! from an in-memory copy of the index that is never written back, so taking
//! one changes neither the index nor any file; it only adds objects to the
//! repository's object store, where Git can collect them later.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{Context, Result, bail};
use git2::{Delta, DiffOptions, IndexAddOption, ObjectType, Oid, Repository};

/// Untracked files larger than this are left out of snapshots, so a stray
/// build artifact or disk image is neither hashed nor reviewed.
pub const MAX_SNAPSHOT_FILE_BYTES: u64 = 16 * 1024 * 1024;
/// Snapshots stop when more than this many paths differ from the index,
/// which in practice means a working tree full of files nobody ignored.
pub const MAX_SNAPSHOT_PATHS: usize = 50_000;
/// A file's diff is cut after this much text.
const MAX_FILE_DIFF_BYTES: usize = 1024 * 1024;

/// One terminal's record of where its latest agent started. It travels with
/// the terminal, and outlives the agent so its changes can be reviewed after
/// it exits.
#[derive(Debug, Default)]
pub struct ChangeTracking {
    /// The agent run the baseline belongs to: its id and process.
    captured_for: Option<(String, Option<i64>)>,
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
    /// Whether `agent` (its id and process) is a run the baseline does not
    /// belong to yet. The same process returning to the foreground, for
    /// example after being suspended, keeps its baseline.
    pub fn needs_capture(&self, agent: Option<(&str, Option<i64>)>) -> bool {
        agent.is_some_and(|(id, process)| {
            self.captured_for
                .as_ref()
                .is_none_or(|(captured, captured_process)| {
                    captured != id || *captured_process != process
                })
        })
    }

    /// Snapshot `directory` on a worker thread for `agent`, replacing the
    /// previous agent's baseline.
    pub fn start_capture(&mut self, agent: (&str, Option<i64>), directory: Option<PathBuf>) {
        self.captured_for = Some((agent.0.to_owned(), agent.1));
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
    /// Untracked files too large to snapshot. Their content is unknown, so
    /// they are never listed or reverted.
    left_out: Arc<BTreeSet<PathBuf>>,
    /// What Git ignored when the snapshot was taken, files and whole
    /// directories. The snapshot does not hold them, so a file here that
    /// looks new may be the person's own, and is never removed.
    ignored: Arc<BTreeSet<PathBuf>>,
    /// When the snapshot was taken, to tell the directories the agent made.
    captured_at: SystemTime,
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

/// A working-tree snapshot.
struct Snapshot {
    tree: Oid,
    /// Untracked files too large to include.
    left_out: BTreeSet<PathBuf>,
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
    let captured_at = SystemTime::now();
    let snapshot = snapshot(&repo)?;
    Ok(Some(Baseline {
        repo_root: root,
        tree: snapshot.tree,
        left_out: Arc::new(snapshot.left_out),
        ignored: Arc::new(ignored_paths(&repo)?),
        captured_at,
    }))
}

/// The files and outermost directories Git ignores in the working tree.
fn ignored_paths(repo: &Repository) -> Result<BTreeSet<PathBuf>> {
    let mut options = git2::StatusOptions::new();
    options
        .include_ignored(true)
        .recurse_ignored_dirs(false)
        .include_untracked(false)
        .exclude_submodules(true);
    let statuses = repo.statuses(Some(&mut options))?;
    Ok(statuses
        .iter()
        .filter(|entry| entry.status().is_ignored())
        .map(|entry| {
            let bytes = entry.path_bytes();
            let bytes = bytes.strip_suffix(b"/").unwrap_or(bytes);
            PathBuf::from(<std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(bytes))
        })
        .collect())
}

/// The working tree as a tree object, without touching the on-disk index.
fn snapshot(repo: &Repository) -> Result<Snapshot> {
    let root = repo
        .workdir()
        .context("the repository has no working tree")?
        .to_path_buf();
    // `repo.index()` loads the index into memory; nothing here writes it
    // back, so the real index is unchanged.
    let mut index = repo.index()?;
    let mut added = 0usize;
    let mut too_many = false;
    let mut large = BTreeSet::new();
    index
        .add_all(
            ["*"],
            IndexAddOption::DEFAULT,
            // 0 adds the path, a positive value skips it, a negative one stops.
            Some(&mut |path: &Path, _: &[u8]| {
                added += 1;
                if added > MAX_SNAPSHOT_PATHS {
                    too_many = true;
                    return -1;
                }
                match fs::symlink_metadata(root.join(path)) {
                    // A directory here is a nested repository Git cannot add.
                    Ok(metadata) if metadata.is_dir() => 1,
                    Ok(metadata) if metadata.is_file() && metadata.len() > MAX_SNAPSHOT_FILE_BYTES => {
                        large.insert(path.to_path_buf());
                        1
                    }
                    _ => 0,
                }
            }),
        )
        .or_else(|error| {
            if too_many {
                bail!(
                    "more than {MAX_SNAPSHOT_PATHS} files differ from the index, too many to compare"
                )
            }
            Err(error.into())
        })?;
    // Files deleted from the working tree leave the snapshot too; tracked
    // files, however large, are brought up to date here.
    index.update_all(["*"], None)?;
    let left_out = large
        .into_iter()
        .filter(|path| index.get_path(path, 0).is_none())
        .collect();
    Ok(Snapshot {
        tree: index.write_tree()?,
        left_out,
    })
}

fn open(baseline: &Baseline) -> Result<Repository> {
    Repository::open(&baseline.repo_root)
        .with_context(|| format!("open the repository at {}", baseline.repo_root.display()))
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

/// The path a delta is about.
fn delta_path(delta: &git2::DiffDelta<'_>) -> Result<PathBuf> {
    Ok(delta
        .new_file()
        .path()
        .or_else(|| delta.old_file().path())
        .context("a changed file has no path")?
        .to_path_buf())
}

/// Every file that differs from the baseline, in path order. Untracked files
/// too large to snapshot, then or now, are left out.
pub fn changes_since(baseline: &Baseline) -> Result<Vec<ChangedFile>> {
    let repo = open(baseline)?;
    let current = snapshot(&repo)?;
    let diff = diff_between(&repo, baseline, current.tree, None)?;
    let current_tree = repo.find_tree(current.tree)?;
    let head_tree = repo.head().ok().and_then(|head| head.peel_to_tree().ok());
    // Loaded fresh: the snapshot's in-memory changes are not the real index.
    let mut staging = git2::Index::open(&repo.path().join("index"))?;
    staging.read(true)?;
    let mut files = Vec::new();
    for (index, delta) in diff.deltas().enumerate() {
        let path = delta_path(&delta)?;
        if baseline.left_out.contains(&path) || current.left_out.contains(&path) {
            continue;
        }
        let kind = match delta.status() {
            Delta::Added => ChangeKind::Added,
            Delta::Deleted => ChangeKind::Deleted,
            _ => ChangeKind::Modified,
        };
        let patch = git2::Patch::from_diff(&diff, index)?;
        let (_, added_lines, removed_lines) = patch
            .as_ref()
            .map(git2::Patch::line_stats)
            .transpose()?
            .unwrap_or((0, 0, 0));
        let now = current_tree
            .get_path(&path)
            .ok()
            .map(|entry| (entry.id(), entry.filemode() as u32));
        let in_index = staging
            .get_path(&path, 0)
            .map(|entry| (entry.id, entry.mode));
        let in_head = head_tree
            .as_ref()
            .is_some_and(|tree| tree.get_path(&path).is_ok());
        // A deleted file Git never knew about has nothing to stage.
        let staged = in_index == now && (in_index.is_some() || in_head);
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
    let current = snapshot(&repo)?;
    let diff = diff_between(&repo, baseline, current.tree, Some(path))?;
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

/// Stage `paths` as they are now, including deletions. A path that is now a
/// directory, or that is reached through a symbolic link or a file, is
/// staged as removed, as Git would; its files are listed separately. A
/// submodule is staged at the commit it is on now.
pub fn stage(baseline: &Baseline, paths: &[PathBuf]) -> Result<()> {
    let repo = open(baseline)?;
    let mut index = repo.index()?;
    let tree = repo.find_tree(baseline.tree)?;
    for path in paths {
        check_relative(path)?;
        let present = if blocked_ancestor(&baseline.repo_root, path)?.is_some() {
            None
        } else {
            fs::symlink_metadata(baseline.repo_root.join(path)).ok()
        };
        let gitlink = index
            .get_path(path, 0)
            .is_some_and(|entry| entry.mode == GITLINK_MODE)
            || tree
                .get_path(path)
                .is_ok_and(|entry| entry.filemode() == GITLINK_MODE as i32);
        match present {
            Some(metadata) if !metadata.is_dir() || gitlink => index.add_path(path)?,
            _ => index.remove_path(path)?,
        }
    }
    index.write()?;
    Ok(())
}

/// Git's file mode for a submodule's commit.
const GITLINK_MODE: u32 = 0o160000;

/// Refuse a path that is not a plain relative path.
fn check_relative(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        bail!("{} is not a path inside the repository", path.display());
    }
    Ok(())
}

/// The first ancestor of `path` below `root` that is not a real directory,
/// and whether it is a symbolic link (else a file). Missing ancestors are
/// not in the way.
fn blocked_ancestor(root: &Path, path: &Path) -> Result<Option<(PathBuf, bool)>> {
    let mut parent = root.to_path_buf();
    let parts: Vec<_> = path.components().collect();
    for part in &parts[..parts.len().saturating_sub(1)] {
        parent.push(part);
        match fs::symlink_metadata(&parent) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Ok(Some((parent, true)));
            }
            Ok(metadata) if !metadata.is_dir() => return Ok(Some((parent, false))),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(None)
}

/// Refuse a path that is not a plain relative path inside the working tree,
/// or that leads through a symbolic link or a file.
fn check_path_inside(root: &Path, path: &Path) -> Result<()> {
    check_relative(path)?;
    match blocked_ancestor(root, path)? {
        Some((_, true)) => bail!(
            "{} leads through a symbolic link; restore it by hand",
            path.display()
        ),
        Some((parent, false)) => bail!(
            "{} is inside {}, which is now a file; revert that first",
            path.display(),
            parent.strip_prefix(root).unwrap_or(&parent).display()
        ),
        None => Ok(()),
    }
}

/// File systems stamp times from a coarse clock that can lag the precise
/// one by a few milliseconds.
const TIMESTAMP_GRANULARITY: std::time::Duration = std::time::Duration::from_millis(20);

/// Whether the file system records that the entry was created after
/// `moment`. Without a recorded creation time, the answer is no: a change
/// time says nothing about when a directory appeared.
fn created_after(metadata: &fs::Metadata, moment: SystemTime) -> bool {
    let moment = moment
        .checked_sub(TIMESTAMP_GRANULARITY)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    metadata.created().is_ok_and(|created| created >= moment)
}

/// Remove the directories above `path` that the agent created and left
/// empty, so a file that became a directory can be put back.
fn remove_emptied_parents(baseline: &Baseline, tree: &git2::Tree<'_>, path: &Path) {
    let mut relative = path.parent();
    while let Some(parent) = relative.filter(|parent| !parent.as_os_str().is_empty()) {
        let absolute = baseline.repo_root.join(parent);
        let recorded = tree
            .get_path(parent)
            .is_ok_and(|entry| entry.kind() == Some(ObjectType::Tree));
        let new = fs::symlink_metadata(&absolute).is_ok_and(|metadata| {
            metadata.is_dir() && created_after(&metadata, baseline.captured_at)
        });
        // `remove_dir` removes only an empty directory.
        if recorded || !new || fs::remove_dir(&absolute).is_err() {
            return;
        }
        relative = parent.parent();
    }
}

/// Put `path` back as it was in the baseline: restore its content and mode
/// through Git's own checkout (line-ending and other filters included), or
/// remove it if it did not exist then. Only the working tree changes. Every
/// check happens before anything is touched.
///
/// The checks refuse links and files already in the way. An agent racing the
/// revert could still swap a directory for a link between them and the
/// rename, but it runs as the same person and could write there itself, so
/// that race grants it nothing.
pub fn revert(baseline: &Baseline, path: &Path) -> Result<()> {
    check_path_inside(&baseline.repo_root, path)?;
    if baseline.left_out.contains(path) {
        bail!(
            "{} was too large to record when the agent started",
            path.display()
        );
    }
    let repo = open(baseline)?;
    let target = baseline.repo_root.join(path);
    let tree = repo.find_tree(baseline.tree)?;
    let entry = match tree.get_path(path) {
        Ok(entry) => Some(entry),
        Err(error) if error.code() == git2::ErrorCode::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    match entry.as_ref().and_then(git2::TreeEntry::kind) {
        Some(ObjectType::Commit) => bail!(
            "{} is a submodule; put it back with `git submodule update`",
            path.display()
        ),
        Some(ObjectType::Blob) | None => {}
        Some(_) => bail!("{} was a directory when the agent started", path.display()),
    }
    let current = fs::symlink_metadata(&target).ok();
    // An empty directory where the file was is replaced; one with files in
    // it is refused, since those files are listed and reverted on their own.
    let empty_directory = match &current {
        Some(metadata) if metadata.is_dir() => {
            if entry.is_none()
                || fs::read_dir(&target)
                    .map(|mut entries| entries.next().is_some())
                    .unwrap_or(true)
            {
                bail!(
                    "{} is now a directory; revert the files in it first",
                    path.display()
                );
            }
            true
        }
        _ => false,
    };
    let Some(entry) = entry else {
        if current.is_some() {
            // Git ignored it then, so the snapshot does not know whether it
            // was there, or what it held.
            if baseline
                .ignored
                .iter()
                .any(|ignored| path.starts_with(ignored))
            {
                bail!(
                    "{} was already here when the agent started, though Git ignored it; remove it by hand if you mean to",
                    path.display()
                );
            }
            fs::remove_file(&target).with_context(|| format!("remove {}", target.display()))?;
            remove_emptied_parents(baseline, &tree, path);
        }
        return Ok(());
    };
    // Check out a tree holding only this file into a private directory, so
    // Git applies its filters and file mode without a pathspec that could
    // match anything else, then move the result into place.
    let empty = repo.find_tree(repo.treebuilder(None)?.write()?)?;
    let mut single = git2::build::TreeUpdateBuilder::new();
    single.upsert(
        path,
        entry.id(),
        match entry.filemode() {
            0o100755 => git2::FileMode::BlobExecutable,
            0o120000 => git2::FileMode::Link,
            _ => git2::FileMode::Blob,
        },
    );
    let single = repo.find_tree(single.create_updated(&repo, &empty)?)?;
    let staging_dir = tempfile::Builder::new()
        .prefix("termeric-revert-")
        .tempdir_in(repo.path())
        .context("create a private checkout directory")?;
    let mut checkout = git2::build::CheckoutBuilder::new();
    checkout
        .force()
        .update_index(false)
        .target_dir(staging_dir.path());
    repo.checkout_tree(single.as_object(), Some(&mut checkout))
        .with_context(|| format!("restore {}", path.display()))?;
    let restored = staging_dir.path().join(path);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    if empty_directory {
        fs::remove_dir(&target).with_context(|| format!("replace {}", target.display()))?;
    }
    if let Err(error) = fs::rename(&restored, &target) {
        // A worktree whose Git directory is on another file system.
        if error.raw_os_error() != Some(libc::EXDEV) {
            return Err(error).with_context(|| format!("replace {}", target.display()));
        }
        if current.is_some() && !empty_directory {
            fs::remove_file(&target)?;
        }
        if fs::symlink_metadata(&restored)?.file_type().is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(fs::read_link(&restored)?, &target)?;
        } else {
            fs::copy(&restored, &target)?;
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
    fn a_baseline_belongs_to_one_agent_run_and_outlives_it() {
        let (temp, _repo) = repository();
        let mut tracking = ChangeTracking::default();
        let run = ("claude-code", Some(4242_i64));
        assert!(!tracking.needs_capture(None));
        assert!(tracking.needs_capture(Some(run)));
        tracking.start_capture(run, Some(temp.path().to_path_buf()));
        assert_eq!(tracking.baseline(), None, "not until the snapshot finishes");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !tracking.poll() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let first = tracking.baseline().unwrap().clone();
        assert!(!tracking.needs_capture(Some(run)), "still the same run");
        // Suspended (no agent in front), then resumed: the same process keeps
        // its baseline, so earlier edits stay reviewable.
        assert!(!tracking.needs_capture(None));
        assert!(!tracking.needs_capture(Some(run)));
        assert_eq!(tracking.baseline(), Some(&first));
        // A new process, or another agent, gets its own.
        assert!(tracking.needs_capture(Some(("claude-code", Some(5000)))));
        assert!(tracking.needs_capture(Some(("codex", Some(4242)))));
        tracking.start_capture(("codex", Some(4242)), None);
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
    fn large_untracked_files_are_neither_listed_nor_reverted() {
        let (temp, _repo) = repository();
        let root = temp.path();
        // Existing before the agent: its content was never recorded.
        let before = fs::File::create(root.join("data.db")).unwrap();
        before.set_len(MAX_SNAPSHOT_FILE_BYTES + 1).unwrap();
        let baseline = capture(root).unwrap().unwrap();
        fs::write(root.join("data.db"), "shrunk by the agent").unwrap();
        let big = fs::File::create(root.join("disk.img")).unwrap();
        big.set_len(MAX_SNAPSHOT_FILE_BYTES + 1).unwrap();
        fs::write(root.join("small.txt"), "x").unwrap();
        let paths: Vec<_> = changes_since(&baseline)
            .unwrap()
            .into_iter()
            .map(|file| file.path)
            .collect();
        assert_eq!(paths, [PathBuf::from("small.txt")]);
        assert!(revert(&baseline, Path::new("data.db")).is_err());
        assert_eq!(
            fs::read_to_string(root.join("data.db")).unwrap(),
            "shrunk by the agent",
            "an unrecorded file is never deleted"
        );
    }

    #[test]
    fn revert_never_writes_through_a_symlinked_directory() {
        let (temp, _repo) = repository();
        let root = temp.path();
        fs::create_dir(root.join("dir")).unwrap();
        fs::write(root.join("dir/file.txt"), "inside").unwrap();
        let baseline = capture(root).unwrap().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("file.txt"), "PRECIOUS").unwrap();
        fs::remove_dir_all(root.join("dir")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("dir")).unwrap();
        let error = revert(&baseline, Path::new("dir/file.txt")).unwrap_err();
        assert!(error.to_string().contains("symbolic link"), "{error}");
        assert_eq!(
            fs::read_to_string(outside.path().join("file.txt")).unwrap(),
            "PRECIOUS"
        );
    }

    #[test]
    fn revert_applies_line_endings_and_treats_glob_characters_literally() {
        let (temp, repo) = repository();
        let root = temp.path();
        fs::write(root.join(".gitattributes"), "*.txt text eol=crlf\n").unwrap();
        fs::write(root.join("crlf.txt"), "a\r\nb\r\n").unwrap();
        fs::write(root.join("f[1].txt"), "bracket\r\n").unwrap();
        fs::write(root.join("f1.txt"), "other file\r\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_all(["*"], IndexAddOption::DEFAULT, None).unwrap();
        index.write().unwrap();
        let baseline = capture(root).unwrap().unwrap();
        fs::write(root.join("crlf.txt"), "changed\r\n").unwrap();
        fs::write(root.join("f[1].txt"), "changed\r\n").unwrap();
        fs::write(root.join("f1.txt"), "edited too\r\n").unwrap();
        revert(&baseline, Path::new("crlf.txt")).unwrap();
        revert(&baseline, Path::new("f[1].txt")).unwrap();
        assert_eq!(fs::read(root.join("crlf.txt")).unwrap(), b"a\r\nb\r\n");
        assert_eq!(fs::read(root.join("f[1].txt")).unwrap(), b"bracket\r\n");
        assert_eq!(
            fs::read(root.join("f1.txt")).unwrap(),
            b"edited too\r\n",
            "a pattern-like name touches only itself"
        );
        let remaining: Vec<_> = changes_since(&baseline)
            .unwrap()
            .into_iter()
            .map(|file| file.path)
            .collect();
        assert_eq!(remaining, [PathBuf::from("f1.txt")]);
        let leftovers: Vec<_> = fs::read_dir(root.join(".git"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("termeric-revert")
            })
            .collect();
        assert!(leftovers.is_empty(), "the private checkout is cleaned up");
    }

    #[test]
    fn directories_and_files_swapping_places_fail_before_changing_anything() {
        let (temp, _repo) = repository();
        let root = temp.path();
        fs::create_dir(root.join("was-dir")).unwrap();
        fs::write(root.join("was-dir/inner.txt"), "x").unwrap();
        let baseline = capture(root).unwrap().unwrap();
        // A file became a directory, and a directory became a file.
        fs::remove_file(root.join("kept.txt")).unwrap();
        fs::create_dir(root.join("kept.txt")).unwrap();
        fs::write(root.join("kept.txt/new.txt"), "y").unwrap();
        fs::remove_dir_all(root.join("was-dir")).unwrap();
        fs::write(root.join("was-dir"), "now a file").unwrap();

        assert!(revert(&baseline, Path::new("kept.txt")).is_err());
        assert!(root.join("kept.txt/new.txt").exists());
        assert!(revert(&baseline, Path::new("was-dir")).is_err());
        assert_eq!(
            fs::read_to_string(root.join("was-dir")).unwrap(),
            "now a file",
            "nothing is deleted before the refusal"
        );
        let error = revert(&baseline, Path::new("was-dir/inner.txt")).unwrap_err();
        assert!(error.to_string().contains("now a file"), "{error}");
        // Staging copes with both shapes.
        let paths: Vec<_> = changes_since(&baseline)
            .unwrap()
            .into_iter()
            .map(|file| file.path)
            .collect();
        stage(&baseline, &paths).unwrap();
    }

    #[test]
    fn a_file_that_became_a_directory_can_be_put_back() {
        let (temp, _repo) = repository();
        let root = temp.path();
        let baseline = capture(root).unwrap().unwrap();
        fs::remove_file(root.join("kept.txt")).unwrap();
        fs::create_dir(root.join("kept.txt")).unwrap();
        fs::write(root.join("kept.txt/new.txt"), "y").unwrap();
        revert(&baseline, Path::new("kept.txt/new.txt")).unwrap();
        assert!(
            !root.join("kept.txt").exists(),
            "the emptied directory goes too"
        );
        revert(&baseline, Path::new("kept.txt")).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("kept.txt")).unwrap(),
            "one\ntwo\nthree\n"
        );
        // An empty directory in the file's place is replaced.
        fs::remove_file(root.join("kept.txt")).unwrap();
        fs::create_dir(root.join("kept.txt")).unwrap();
        revert(&baseline, Path::new("kept.txt")).unwrap();
        assert!(root.join("kept.txt").is_file());
    }

    #[test]
    fn a_file_git_ignored_when_the_agent_started_is_never_deleted() {
        let (temp, _repo) = repository();
        let root = temp.path();
        fs::write(root.join(".gitignore"), "target/\n.env\n").unwrap();
        fs::write(root.join(".env"), "SECRET=1\n").unwrap();
        let baseline = capture(root).unwrap().unwrap();
        // The agent stops ignoring it, so it now looks new.
        fs::write(root.join(".gitignore"), "target/\n").unwrap();
        let listed = changes_since(&baseline).unwrap();
        assert!(
            listed
                .iter()
                .any(|file| file.path == Path::new(".env") && file.kind == ChangeKind::Added)
        );
        let error = revert(&baseline, Path::new(".env")).unwrap_err();
        assert!(error.to_string().contains("already here"), "{error}");
        assert_eq!(fs::read_to_string(root.join(".env")).unwrap(), "SECRET=1\n");
        // Rewritten as a new file, as `sed -i` does, it is still theirs.
        fs::remove_file(root.join(".env")).unwrap();
        fs::write(root.join(".env"), "SECRET=2\n").unwrap();
        assert!(revert(&baseline, Path::new(".env")).is_err());
        assert!(root.join(".env").exists());
        // So is anything in a directory Git ignored.
        fs::write(root.join("target/built.bin"), "x").unwrap();
        fs::write(root.join(".gitignore"), "").unwrap();
        assert!(revert(&baseline, Path::new("target/built.bin")).is_err());
        // A file the agent created is still removed.
        fs::write(root.join("made.txt"), "x").unwrap();
        revert(&baseline, Path::new("made.txt")).unwrap();
        assert!(!root.join("made.txt").exists());
    }

    #[test]
    fn staging_never_reads_through_a_linked_directory() {
        let (temp, repo) = repository();
        let root = temp.path();
        fs::create_dir(root.join("dir")).unwrap();
        fs::write(root.join("dir/file.txt"), "inside\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("dir/file.txt")).unwrap();
        index.write().unwrap();
        let baseline = capture(root).unwrap().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("file.txt"), "outside\n").unwrap();
        fs::remove_dir_all(root.join("dir")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("dir")).unwrap();
        stage(&baseline, &[PathBuf::from("dir/file.txt")]).unwrap();
        let mut index = repo.index().unwrap();
        index.read(true).unwrap();
        assert!(
            index.get_path(Path::new("dir/file.txt"), 0).is_none(),
            "staged as removed, like Git, rather than with the linked file's content"
        );
    }

    #[test]
    fn a_submodule_is_staged_at_its_new_commit_never_removed() {
        let (temp, repo) = repository();
        let root = temp.path();
        // A submodule: a nested repository recorded as a gitlink.
        let sub = Repository::init(root.join("sub")).unwrap();
        let signature = git2::Signature::now("Test", "test@example.com").unwrap();
        let commit = |message: &str| {
            let mut index = sub.index().unwrap();
            fs::write(root.join("sub/file"), message).unwrap();
            index.add_path(Path::new("file")).unwrap();
            index.write().unwrap();
            let tree = sub.find_tree(index.write_tree().unwrap()).unwrap();
            let parents: Vec<_> = sub
                .head()
                .ok()
                .and_then(|head| head.peel_to_commit().ok())
                .into_iter()
                .collect();
            let parents: Vec<_> = parents.iter().collect();
            sub.commit(
                Some("HEAD"),
                &signature,
                &signature,
                message,
                &tree,
                &parents,
            )
            .unwrap()
        };
        let first = commit("one");
        let mut index = repo.index().unwrap();
        index
            .add(&git2::IndexEntry {
                ctime: git2::IndexTime::new(0, 0),
                mtime: git2::IndexTime::new(0, 0),
                dev: 0,
                ino: 0,
                mode: GITLINK_MODE,
                uid: 0,
                gid: 0,
                file_size: 0,
                id: first,
                flags: 3,
                flags_extended: 0,
                path: b"sub".to_vec(),
            })
            .unwrap();
        index.write().unwrap();
        let baseline = capture(root).unwrap().unwrap();
        let second = commit("two");
        stage(&baseline, &[PathBuf::from("sub")]).unwrap();
        let mut index = repo.index().unwrap();
        index.read(true).unwrap();
        let entry = index.get_path(Path::new("sub"), 0).unwrap();
        assert_eq!((entry.mode, entry.id), (GITLINK_MODE, second));
        let error = revert(&baseline, Path::new("sub")).unwrap_err();
        assert!(error.to_string().contains("submodule"), "{error}");
    }

    #[test]
    fn nested_repositories_do_not_stop_the_snapshot() {
        let (temp, _repo) = repository();
        let root = temp.path();
        Repository::init(root.join("inner")).unwrap();
        fs::write(root.join("inner/file"), "x").unwrap();
        let baseline = capture(root).unwrap().unwrap();
        fs::write(root.join("kept.txt"), "changed\n").unwrap();
        assert_eq!(changes_since(&baseline).unwrap().len(), 1);
    }

    #[test]
    fn staged_means_the_index_matches_content_and_mode() {
        let (temp, repo) = repository();
        let root = temp.path();
        let baseline = capture(root).unwrap().unwrap();
        fs::set_permissions(root.join("kept.txt"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::remove_file(root.join("notes.txt")).unwrap();
        let staged = |baseline: &Baseline| {
            changes_since(baseline)
                .unwrap()
                .into_iter()
                .map(|file| (file.path.to_string_lossy().into_owned(), file.staged))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            staged(&baseline),
            [("kept.txt".into(), false), ("notes.txt".into(), false)],
            "a mode change is not staged, and an untracked deletion has nothing to stage"
        );
        stage(&baseline, &["kept.txt".into()]).unwrap();
        let mut index = repo.index().unwrap();
        index.read(true).unwrap();
        assert_eq!(
            index.get_path(Path::new("kept.txt"), 0).unwrap().mode,
            0o100755
        );
        assert_eq!(staged(&baseline)[0], ("kept.txt".into(), true));
    }
}
