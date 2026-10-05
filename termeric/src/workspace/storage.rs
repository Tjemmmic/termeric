use super::{
    WORKSPACE_SCHEMA_VERSION, WorkspaceSnapshot, validate_workspace_id, validate_workspace_name,
};
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

pub const MAX_WORKSPACE_SCROLLBACK_BYTES_PER_PANE: usize = 5 * 1024 * 1024;
pub const MAX_WORKSPACE_SCROLLBACK_BYTES_TOTAL: usize = 50 * 1024 * 1024;
pub const MAX_WORKSPACE_METADATA_BYTES: usize = 5 * 1024 * 1024;

#[cfg(test)]
thread_local! {
    static TEST_WORKSPACES_BASE_DIR: std::cell::RefCell<Option<PathBuf>> = const {
        std::cell::RefCell::new(None)
    };
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSaveReport {
    pub id: String,
    pub scrollback_bytes: usize,
    pub truncated_panes: Vec<(u32, u32)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceLoadReport {
    pub workspace: WorkspaceSnapshot,
    pub warnings: Vec<String>,
    pub recovered_from_backup: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceListEntry {
    pub id: String,
    pub name: String,
    pub updated_at: String,
    pub tab_count: usize,
    pub pane_count: usize,
    /// Load failure summary for an archive that remains visible so the user
    /// can inspect/delete it instead of losing it from the manager.
    pub warning: Option<String>,
}

pub fn workspaces_base_dir() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(path) = TEST_WORKSPACES_BASE_DIR.with(|base| base.borrow().clone()) {
        return Some(path);
    }
    crate::paths::data_dir().map(|dir| dir.join("workspaces"))
}

#[cfg(test)]
pub(crate) fn set_test_workspaces_base_dir(path: Option<PathBuf>) {
    TEST_WORKSPACES_BASE_DIR.with(|base| *base.borrow_mut() = path);
}

pub fn workspace_dir(id: &str) -> Result<PathBuf> {
    validate_workspace_id(id)?;
    workspaces_base_dir()
        .map(|base| base.join(id))
        .ok_or_else(|| anyhow::anyhow!("workspace data directory is unavailable"))
}

/// The time a save records, in milliseconds and strictly after every earlier
/// one this process recorded: listings sort by it, and pruning keeps the
/// newest, so two saves in the same instant must still have an order.
pub(crate) fn save_timestamp() -> String {
    static LAST_MILLIS: std::sync::Mutex<u128> = std::sync::Mutex::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let mut last = LAST_MILLIS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *last = now.max(*last + 1);
    let millis = *last;
    crate::session::format_unix_timestamp(
        u64::try_from(millis / 1000).unwrap_or(u64::MAX),
        Some((millis % 1000) as u32),
    )
}

pub fn save_workspace(workspace: &mut WorkspaceSnapshot) -> Result<WorkspaceSaveReport> {
    let dir = workspace_dir(&workspace.id)?;
    save_workspace_to_dir(workspace, &dir)
}

pub fn save_workspace_to_dir(
    workspace: &mut WorkspaceSnapshot,
    dir: &Path,
) -> Result<WorkspaceSaveReport> {
    save_workspace_to_dir_with_backup(workspace, dir, true)
}

fn save_workspace_to_dir_with_backup(
    workspace: &mut WorkspaceSnapshot,
    dir: &Path,
    replace_backup: bool,
) -> Result<WorkspaceSaveReport> {
    workspace.validate()?;
    ensure_real_directory(dir, true, "workspace")?;
    let _writing = lock_archive(dir, ArchiveLock::Write)?;
    remove_interrupted_staging(dir)?;
    let scrollback_dir = dir.join("scrollback");
    ensure_real_directory(&scrollback_dir, true, "workspace scrollback")?;

    let mut total_bytes = 0usize;
    let mut truncated_panes = Vec::new();
    for tab in &mut workspace.tabs {
        for pane in &mut tab.panes {
            if pane.scrollback_lines.is_empty() {
                pane.scrollback_file = None;
                pane.scrollback_truncated = false;
                continue;
            }
            let remaining = MAX_WORKSPACE_SCROLLBACK_BYTES_TOTAL.saturating_sub(total_bytes);
            let limit = remaining.min(MAX_WORKSPACE_SCROLLBACK_BYTES_PER_PANE);
            let (payload, truncated) = bounded_scrollback(&pane.scrollback_lines, limit);
            if payload.is_empty() {
                pane.scrollback_file = None;
                pane.scrollback_truncated = true;
                truncated_panes.push((tab.id, pane.id));
                continue;
            }
            let filename = deduplicated_scrollback_name(&scrollback_dir, &payload)?;
            let path = scrollback_dir.join(&filename);
            if fs::symlink_metadata(&path).is_err() {
                atomic_write(&path, &payload, false)?;
            }
            pane.scrollback_file = Some(format!("scrollback/{filename}"));
            pane.scrollback_truncated = truncated;
            total_bytes += payload.len();
            if truncated {
                truncated_panes.push((tab.id, pane.id));
            }
        }
    }

    workspace.updated_at = save_timestamp();
    workspace.validate()?;
    let metadata = toml::to_string_pretty(workspace)?;
    // Loading refuses larger metadata, so writing it would replace a
    // loadable archive with one that can't be opened.
    if metadata.len() > MAX_WORKSPACE_METADATA_BYTES {
        anyhow::bail!(
            "workspace metadata would be {} bytes, over its {MAX_WORKSPACE_METADATA_BYTES}-byte limit",
            metadata.len()
        );
    }
    atomic_write(
        &dir.join("workspace.toml"),
        metadata.as_bytes(),
        replace_backup,
    )?;
    cleanup_unreferenced_scrollback(workspace, &scrollback_dir)?;

    Ok(WorkspaceSaveReport {
        id: workspace.id.clone(),
        scrollback_bytes: total_bytes,
        truncated_panes,
    })
}

pub fn load_workspace(id: &str) -> Result<WorkspaceLoadReport> {
    let dir = workspace_dir(id)?;
    ensure_real_directory(&dir, false, "workspace")?;
    let _reading = lock_archive(&dir, ArchiveLock::Read)?;
    load_scrollback(&dir, read_named_workspace_metadata(&dir)?)
}

/// The archive in any directory, whatever it is named: for tests.
#[cfg(test)]
pub fn load_workspace_from_dir(dir: &Path) -> Result<WorkspaceLoadReport> {
    load_scrollback(dir, read_workspace_metadata(dir)?)
}

/// The archive in `dir` whose metadata was just read, with each pane's saved
/// scrollback.
fn load_scrollback(
    dir: &Path,
    (mut workspace, recovered_from_backup, mut warnings): (WorkspaceSnapshot, bool, Vec<String>),
) -> Result<WorkspaceLoadReport> {
    let mut loaded_scrollback_bytes = 0usize;
    for tab in &mut workspace.tabs {
        for pane in &mut tab.panes {
            let Some(relative) = pane.scrollback_file.clone() else {
                continue;
            };
            match safe_scrollback_path(dir, &relative).and_then(|path| {
                read_bounded_scrollback(
                    &path,
                    MAX_WORKSPACE_SCROLLBACK_BYTES_TOTAL
                        .saturating_sub(loaded_scrollback_bytes)
                        .min(MAX_WORKSPACE_SCROLLBACK_BYTES_PER_PANE),
                )
            }) {
                Ok(content) => {
                    loaded_scrollback_bytes += content.len();
                    pane.scrollback_lines = content.lines().map(String::from).collect();
                }
                Err(error) => {
                    warnings.push(format!(
                        "tab {} pane {} scrollback was skipped: {error:#}",
                        tab.id, pane.id
                    ));
                    pane.scrollback_file = None;
                    pane.scrollback_lines.clear();
                }
            }
        }
    }

    Ok(WorkspaceLoadReport {
        workspace,
        warnings,
        recovered_from_backup,
    })
}

/// The archive's metadata, without reading any pane's scrollback: enough to
/// list or prune workspaces cheaply.
fn read_workspace_metadata(dir: &Path) -> Result<(WorkspaceSnapshot, bool, Vec<String>)> {
    ensure_real_directory(dir, false, "workspace")?;
    let primary = dir.join("workspace.toml");
    let backup = dir.join("workspace.toml.bak");
    let (mut workspace, recovered_from_backup) = match parse_workspace(&primary) {
        Ok(workspace) => (workspace, false),
        Err(primary_error) => match parse_workspace(&backup) {
            Ok(workspace) => (workspace, true),
            Err(backup_error) => {
                return Err(primary_error).with_context(|| {
                    format!("workspace backup could not be recovered either: {backup_error:#}")
                });
            }
        },
    };

    check_schema(&workspace)?;
    let mut warnings = Vec::new();
    recover_usable_tabs(&mut workspace, &mut warnings);
    if workspace.tabs.is_empty() {
        anyhow::bail!("workspace contains no recoverable tabs");
    }

    Ok((workspace, recovered_from_backup, warnings))
}

/// [`read_workspace_metadata`] for the archive in a directory named for its
/// ID. A directory whose metadata names another archive, such as a copy of
/// one, is not that archive: listing, pruning, or deleting by the ID inside
/// it would reach the original.
fn read_named_workspace_metadata(dir: &Path) -> Result<(WorkspaceSnapshot, bool, Vec<String>)> {
    let read = read_workspace_metadata(dir)?;
    let id = &read.0.id;
    if dir.file_name().and_then(|name| name.to_str()) != Some(id.as_str()) {
        anyhow::bail!(
            "workspace directory {} holds archive {id:?}, not its own",
            dir.display()
        );
    }
    Ok(read)
}

/// Saved workspaces, newest first. Closed-tab snapshots are listed apart, by
/// [`list_closed_tabs`]; an archive that can't be read is listed here, so it
/// can be inspected and deleted.
pub fn list_workspaces() -> Vec<WorkspaceListEntry> {
    list_archives(false)
}

/// Closed-tab snapshots, newest first.
pub fn list_closed_tabs() -> Vec<WorkspaceListEntry> {
    list_archives(true)
}

fn list_archives(closed_tabs: bool) -> Vec<WorkspaceListEntry> {
    let Some(base) = workspaces_base_dir() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(base) else {
        return Vec::new();
    };
    let mut workspaces: Vec<_> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let path = entry.path();
            match read_named_workspace_metadata(&path) {
                Ok((workspace, ..)) => {
                    (workspace.closed_tab == closed_tabs).then(|| list_entry(workspace))
                }
                Err(_) if closed_tabs => None,
                Err(error) => {
                    let id = entry.file_name().to_string_lossy().to_string();
                    validate_workspace_id(&id).ok()?;
                    Some(WorkspaceListEntry {
                        name: id.clone(),
                        id,
                        updated_at: "unreadable".into(),
                        tab_count: 0,
                        pane_count: 0,
                        warning: Some(format!("archive unavailable: {error:#}")),
                    })
                }
            }
        })
        .collect();
    // An unreadable archive has no time to sort by, so it goes after every
    // readable one rather than wherever its placeholder text would fall.
    workspaces.sort_by(|left, right| {
        left.warning
            .is_some()
            .cmp(&right.warning.is_some())
            .then_with(|| right.updated_at.cmp(&left.updated_at))
    });
    workspaces
}

/// The listing of the readable archive `id`, read without its scrollback.
pub fn describe_workspace(id: &str) -> Result<WorkspaceListEntry> {
    let dir = workspace_dir(id)?;
    ensure_real_directory(&dir, false, "workspace")?;
    let _reading = lock_archive(&dir, ArchiveLock::Read)?;
    let (workspace, ..) = read_named_workspace_metadata(&dir)?;
    Ok(list_entry(workspace))
}

fn list_entry(workspace: WorkspaceSnapshot) -> WorkspaceListEntry {
    WorkspaceListEntry {
        pane_count: workspace.tabs.iter().map(|tab| tab.panes.len()).sum(),
        tab_count: workspace.tabs.len(),
        id: workspace.id,
        name: workspace.name,
        updated_at: workspace.updated_at,
        warning: None,
    }
}

/// How many closed-tab snapshots are kept; older ones are deleted.
pub const MAX_CLOSED_TABS: usize = 100;

/// Delete all but the newest [`MAX_CLOSED_TABS`] closed-tab snapshots.
/// Returns how many went.
pub fn prune_closed_tabs() -> usize {
    list_closed_tabs()
        .into_iter()
        .skip(MAX_CLOSED_TABS)
        .filter(|snapshot| match delete_workspace(&snapshot.id) {
            Ok(()) => true,
            Err(error) => {
                log::warn!("Could not prune closed tab {}: {error:#}", snapshot.id);
                false
            }
        })
        .count()
}

/// Delete the automatic archives (each saved when a window closed) that
/// `keep` no longer names. Workspaces the person saved or renamed, and
/// archives that can't be read, are never touched. Returns how many went.
pub fn prune_automatic_workspaces(keep: &[String]) -> usize {
    let Some(base) = workspaces_base_dir() else {
        return 0;
    };
    let Ok(entries) = fs::read_dir(base) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| read_named_workspace_metadata(&entry.path()).ok())
        .filter(|(workspace, ..)| workspace.automatic && !keep.contains(&workspace.id))
        .filter(|(workspace, ..)| match delete_workspace(&workspace.id) {
            Ok(()) => true,
            Err(error) => {
                log::warn!(
                    "Could not prune automatic workspace {}: {error:#}",
                    workspace.id
                );
                false
            }
        })
        .count()
}

pub fn rename_workspace(id: &str, new_name: &str) -> Result<()> {
    validate_workspace_name(new_name)?;
    let dir = workspace_dir(id)?;
    let mut report = load_workspace(id)?;
    report.workspace.name = new_name.trim().to_string();
    // Naming a workspace makes it the person's; it is never pruned.
    report.workspace.automatic = false;
    // A recovered archive has a corrupt/missing primary and one known-good
    // backup. Replacing that backup with the bad primary before installing the
    // repaired metadata would leave no recovery point if the write is
    // interrupted or the new primary is later damaged.
    save_workspace_to_dir_with_backup(&mut report.workspace, &dir, !report.recovered_from_backup)?;
    Ok(())
}

pub fn delete_workspace(id: &str) -> Result<()> {
    let dir = workspace_dir(id)?;
    match fs::symlink_metadata(&dir) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            anyhow::bail!("refusing to delete a non-directory or symlinked workspace")
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!("workspace {id:?} was not found")
        }
        Err(error) => return Err(error.into()),
    }
    let _deleting = lock_archive(&dir, ArchiveLock::Write)?;
    fs::remove_dir_all(&dir)
        .with_context(|| format!("delete workspace directory {}", dir.display()))
}

#[derive(Clone, Copy)]
enum ArchiveLock {
    Read,
    Write,
}

/// Lock the archive in `dir` until the returned handle drops: shared to read
/// it, exclusive to write or delete it. Termeric processes share the data
/// directory, and two can save one archive (a closed tab or workspace both
/// reopened), so without it one save's cleanup could delete scrollback or a
/// staging file another is about to publish, and a load could read
/// metadata whose scrollback a save is replacing.
fn lock_archive(dir: &Path, lock: ArchiveLock) -> Result<fs::File> {
    let directory = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(dir)
        .with_context(|| format!("open workspace directory {}", dir.display()))?;
    match lock {
        ArchiveLock::Read => directory.lock_shared(),
        ArchiveLock::Write => directory.lock(),
    }
    .with_context(|| format!("lock workspace directory {}", dir.display()))?;
    Ok(directory)
}

/// Delete the staging files an interrupted metadata write left in `dir`.
/// Every writer holds the archive's write lock, which the caller holds, so
/// none of them is still being written.
fn remove_interrupted_staging(dir: &Path) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let staging = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(crate::safe_fs::STAGING_PREFIX));
        // A link is removed itself, never followed.
        if staging && !entry.file_type()?.is_dir() {
            fs::remove_file(entry.path()).with_context(|| {
                format!("remove interrupted staging file {}", entry.path().display())
            })?;
        }
    }
    Ok(())
}

fn parse_workspace(path: &Path) -> Result<WorkspaceSnapshot> {
    let content = crate::safe_fs::read_text(path, MAX_WORKSPACE_METADATA_BYTES)
        .with_context(|| format!("read workspace metadata {}", path.display()))?;
    toml::from_str(&content).with_context(|| format!("parse workspace metadata {}", path.display()))
}

/// Only this schema loads; older archives are not migrated.
fn check_schema(workspace: &WorkspaceSnapshot) -> Result<()> {
    if workspace.version != WORKSPACE_SCHEMA_VERSION {
        anyhow::bail!(
            "workspace schema {} is not the supported schema {WORKSPACE_SCHEMA_VERSION}",
            workspace.version
        );
    }
    Ok(())
}

fn recover_usable_tabs(workspace: &mut WorkspaceSnapshot, warnings: &mut Vec<String>) {
    let saved_active = workspace.active_tab;
    let mut seen = HashSet::new();
    let mut recovered = Vec::with_capacity(workspace.tabs.len());
    let mut recovered_active = None;
    for (source_index, tab) in workspace.tabs.drain(..).enumerate() {
        if !seen.insert(tab.id) {
            warnings.push(format!("duplicate tab {} was skipped", tab.id));
            continue;
        }
        match tab.validate() {
            Ok(()) => {
                if source_index == saved_active {
                    recovered_active = Some(recovered.len());
                }
                recovered.push(tab);
            }
            Err(error) => {
                warnings.push(format!("tab {} was skipped: {error:#}", tab.id));
            }
        }
    }
    workspace.tabs = recovered;
    workspace.active_tab = recovered_active.unwrap_or(0);
    if recovered_active.is_none() && !workspace.tabs.is_empty() {
        warnings.push(format!(
            "active tab {saved_active} was unavailable; focused the first recovered tab"
        ));
    }
}

fn bounded_scrollback(lines: &[String], limit: usize) -> (Vec<u8>, bool) {
    let mut start = lines.len();
    let mut bytes = 0usize;
    for (index, line) in lines.iter().enumerate().rev() {
        let needed = line.len().saturating_add(1);
        if bytes.saturating_add(needed) > limit {
            break;
        }
        bytes += needed;
        start = index;
    }
    let mut payload = Vec::with_capacity(bytes);
    for line in &lines[start..] {
        payload.extend_from_slice(line.as_bytes());
        payload.push(b'\n');
    }
    (payload, start > 0)
}

fn deduplicated_scrollback_name(directory: &Path, payload: &[u8]) -> Result<String> {
    let hash = termeric_core::checksum::fnv1a64(payload);
    for collision in 0..=u16::MAX {
        let suffix = if collision == 0 {
            String::new()
        } else {
            format!("-{collision}")
        };
        let filename = format!("{hash:016x}-{}{}.txt", payload.len(), suffix);
        let path = directory.join(&filename);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                anyhow::bail!(
                    "workspace scrollback object {} is not a regular file",
                    path.display()
                )
            }
            Ok(_)
                if crate::safe_fs::read_bytes(&path, payload.len())
                    .is_ok_and(|existing| existing == payload) =>
            {
                return Ok(filename);
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(filename),
            Err(error) => return Err(error.into()),
        }
    }
    anyhow::bail!("could not allocate a collision-free scrollback filename")
}

fn safe_scrollback_path(directory: &Path, relative: &str) -> Result<PathBuf> {
    let path = Path::new(relative);
    if path.components().count() != 2
        || !path.starts_with("scrollback")
        || path.extension().and_then(|extension| extension.to_str()) != Some("txt")
    {
        anyhow::bail!("unsafe workspace scrollback path {relative:?}");
    }
    let scrollback = directory.join("scrollback");
    ensure_real_directory(&scrollback, false, "workspace scrollback")?;
    // The read itself refuses a symlink or anything but a regular file.
    Ok(directory.join(path))
}

fn atomic_write(path: &Path, payload: &[u8], keep_backup: bool) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("atomic write target has no parent"))?;
    ensure_real_directory(parent, true, "atomic-write parent")?;
    if keep_backup {
        match crate::safe_fs::read_text(path, MAX_WORKSPACE_METADATA_BYTES) {
            Ok(previous) => {
                crate::safe_fs::atomic_replace(&path.with_extension("toml.bak"), |file| {
                    file.write_all(previous.as_bytes())
                })
                .with_context(|| format!("keep a backup of {}", path.display()))?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("read {} for its backup", path.display()));
            }
        }
    }
    crate::safe_fs::atomic_replace(path, |file| file.write_all(payload))
        .with_context(|| format!("write {}", path.display()))
}

fn ensure_real_directory(path: &Path, create: bool, label: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            anyhow::bail!("{label} path must be a real, non-symlink directory")
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && create => {
            fs::create_dir_all(path)
                .with_context(|| format!("create {label} directory {}", path.display()))
        }
        Err(error) => {
            Err(error).with_context(|| format!("inspect {label} directory {}", path.display()))
        }
    }
}

fn read_bounded_scrollback(path: &Path, limit: usize) -> Result<String> {
    if limit == 0 {
        anyhow::bail!("workspace scrollback exceeds the total byte limit");
    }
    Ok(crate::safe_fs::read_text(path, limit)?)
}

/// Delete every file in the scrollback folder that `workspace` does not name,
/// including staging files left by an interrupted write. The caller holds
/// the archive's write lock, so no other save is publishing one.
fn cleanup_unreferenced_scrollback(workspace: &WorkspaceSnapshot, directory: &Path) -> Result<()> {
    let referenced: HashSet<&str> = workspace
        .tabs
        .iter()
        .flat_map(|tab| &tab.panes)
        .filter_map(|pane| pane.scrollback_file.as_deref())
        .filter_map(|path| path.strip_prefix("scrollback/"))
        .collect();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() || !file_type.is_file() {
            anyhow::bail!(
                "workspace scrollback directory contains a non-regular object: {}",
                entry.path().display()
            );
        }
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| !referenced.contains(name))
        {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::model::tests::sample_workspace;

    #[test]
    fn archive_round_trip_preserves_layout_focus_metadata_and_deduplicates_scrollback() {
        let temp = tempfile::tempdir().unwrap();
        let mut workspace = sample_workspace();
        workspace.tabs[0].panes[1].scrollback_lines = vec!["history-0".into()];
        let report = save_workspace_to_dir(&mut workspace, temp.path()).unwrap();
        assert!(report.truncated_panes.is_empty());
        let files: Vec<_> = fs::read_dir(temp.path().join("scrollback"))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(
            files.len(),
            2,
            "identical pane history should share one file"
        );

        let loaded = load_workspace_from_dir(temp.path()).unwrap();
        assert!(!loaded.recovered_from_backup);
        assert!(loaded.warnings.is_empty());
        assert_eq!(loaded.workspace.active_tab, 0);
        assert_eq!(loaded.workspace.tabs[0].focused_pane, 2);
        assert_eq!(loaded.workspace.tabs[0].layout, workspace.tabs[0].layout);
        assert_eq!(
            loaded.workspace.tabs[0].panes[0].scrollback_lines,
            vec!["history-0"]
        );
    }

    #[test]
    fn corrupt_primary_recovers_backup_and_interrupted_temp_is_ignored() {
        let temp = tempfile::tempdir().unwrap();
        let mut workspace = sample_workspace();
        save_workspace_to_dir(&mut workspace, temp.path()).unwrap();
        workspace.name = "second save".into();
        save_workspace_to_dir(&mut workspace, temp.path()).unwrap();
        fs::write(temp.path().join("workspace.tmp"), "interrupted").unwrap();
        fs::write(temp.path().join("workspace.toml"), "not = [toml").unwrap();

        let loaded = load_workspace_from_dir(temp.path()).unwrap();
        assert!(loaded.recovered_from_backup);
        assert_eq!(loaded.workspace.name, "Asymmetric roots");
    }

    #[test]
    fn rename_after_backup_recovery_preserves_the_last_known_good_copy() {
        let temp = tempfile::tempdir().unwrap();
        set_test_workspaces_base_dir(Some(temp.path().to_path_buf()));
        let result = std::panic::catch_unwind(|| {
            let mut workspace = sample_workspace();
            save_workspace(&mut workspace).unwrap();
            workspace.name = "newer primary".into();
            save_workspace(&mut workspace).unwrap();

            let dir = workspace_dir(&workspace.id).unwrap();
            fs::write(dir.join("workspace.toml"), "not = [toml").unwrap();
            rename_workspace(&workspace.id, "Recovered rename").unwrap();
            let renamed = load_workspace(&workspace.id).unwrap();
            assert!(!renamed.recovered_from_backup);
            assert_eq!(renamed.workspace.name, "Recovered rename");

            // Damage the newly installed primary too. The rename must not have
            // replaced the only known-good backup with the first corrupt file.
            fs::write(dir.join("workspace.toml"), "also = [broken").unwrap();
            let recovered = load_workspace(&workspace.id).unwrap();
            assert!(recovered.recovered_from_backup);
            assert_eq!(recovered.workspace.name, "Asymmetric roots");
        });
        set_test_workspaces_base_dir(None);
        result.unwrap();
    }

    #[test]
    fn workspace_and_atomic_temporary_symlinks_are_never_followed() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let linked_workspace = temp.path().join("linked-workspace");
        symlink(&outside, &linked_workspace).unwrap();
        let mut workspace = sample_workspace();
        let error = save_workspace_to_dir(&mut workspace, &linked_workspace).unwrap_err();
        assert!(format!("{error:#}").contains("non-symlink directory"));
        assert!(!outside.join("workspace.toml").exists());

        let archive = temp.path().join("archive");
        fs::create_dir(&archive).unwrap();
        let sentinel = temp.path().join("sentinel");
        fs::write(&sentinel, "do-not-replace").unwrap();
        // Staging files get fresh private names, so a link planted at a
        // predictable temporary name is never written through.
        symlink(&sentinel, archive.join("workspace.tmp")).unwrap();
        save_workspace_to_dir(&mut workspace, &archive).unwrap();
        assert_eq!(fs::read_to_string(&sentinel).unwrap(), "do-not-replace");
        assert!(
            fs::symlink_metadata(archive.join("workspace.toml"))
                .unwrap()
                .is_file()
        );

        // A link at the final name fails the save closed: its backup can't be
        // read, and the link's target is never written.
        let final_target = temp.path().join("final-target");
        fs::write(&final_target, "do-not-replace").unwrap();
        fs::remove_file(archive.join("workspace.toml")).unwrap();
        symlink(&final_target, archive.join("workspace.toml")).unwrap();
        assert!(save_workspace_to_dir(&mut workspace, &archive).is_err());
        assert_eq!(fs::read_to_string(&final_target).unwrap(), "do-not-replace");
    }

    #[test]
    fn metadata_and_scrollback_symlinks_fail_closed_without_disclosing_targets() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let mut workspace = sample_workspace();
        save_workspace_to_dir(&mut workspace, temp.path()).unwrap();
        let relative = workspace.tabs[0].panes[0].scrollback_file.clone().unwrap();
        let object = temp.path().join(relative);
        fs::remove_file(&object).unwrap();
        let sentinel = temp.path().join("private-sentinel");
        fs::write(&sentinel, "must-not-enter-terminal-history").unwrap();
        symlink(&sentinel, &object).unwrap();

        let loaded = load_workspace_from_dir(temp.path()).unwrap();
        assert!(
            loaded.workspace.tabs[0].panes[0]
                .scrollback_lines
                .is_empty()
        );
        assert!(loaded.warnings.iter().any(|warning| {
            warning.contains("scrollback was skipped") && !warning.contains("private-sentinel")
        }));

        fs::remove_file(temp.path().join("workspace.toml")).unwrap();
        symlink(&sentinel, temp.path().join("workspace.toml")).unwrap();
        assert!(load_workspace_from_dir(temp.path()).is_err());
    }

    #[test]
    fn hostile_scrollback_paths_never_read_outside_the_archive() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("archive");
        fs::write(temp.path().join("x.txt"), "outside-secret").unwrap();
        let mut workspace = sample_workspace();
        workspace.tabs.push(workspace.tabs[0].clone());
        workspace.tabs[1].id = 8;
        workspace.tabs[1].pinned = false;
        save_workspace_to_dir(&mut workspace, &archive).unwrap();
        fs::create_dir(archive.join("scrollback/nested")).unwrap();
        fs::write(archive.join("scrollback/nested/x.txt"), "outside-secret").unwrap();
        let metadata = fs::read_to_string(archive.join("workspace.toml")).unwrap();

        for hostile in [
            "../x.txt",
            "scrollback/../x.txt",
            "scrollback/../../x.txt",
            "/tmp/x.txt",
            "/scrollback/x.txt",
            "scrollback/nested/x.txt",
            "x.txt",
            "scrollback/x.toml",
        ] {
            assert!(
                safe_scrollback_path(&archive, hostile).is_err(),
                "accepted {hostile:?}"
            );
            let mut encoded: toml::Value = toml::from_str(&metadata).unwrap();
            encoded["tabs"][0]["panes"][0]["scrollback_file"] = toml::Value::String(hostile.into());
            fs::write(
                archive.join("workspace.toml"),
                toml::to_string_pretty(&encoded).unwrap(),
            )
            .unwrap();

            let loaded = load_workspace_from_dir(&archive).unwrap();
            let ids: Vec<_> = loaded.workspace.tabs.iter().map(|tab| tab.id).collect();
            assert_eq!(ids, [8], "{hostile:?}");
            assert!(
                loaded
                    .warnings
                    .iter()
                    .any(|warning| warning.contains("unsafe scrollback path")),
                "{hostile:?}: {:?}",
                loaded.warnings
            );
            let history = loaded
                .workspace
                .tabs
                .iter()
                .flat_map(|tab| &tab.panes)
                .flat_map(|pane| &pane.scrollback_lines)
                .any(|line| line.contains("outside-secret"));
            assert!(!history, "{hostile:?} read a file outside the archive");
        }
    }

    #[test]
    fn oversized_metadata_is_refused_and_the_previous_archive_still_loads() {
        let temp = tempfile::tempdir().unwrap();
        let mut workspace = sample_workspace();
        save_workspace_to_dir(&mut workspace, temp.path()).unwrap();

        let mut oversized = workspace.clone();
        oversized.name = "oversized".into();
        oversized.tabs[0].panes[0].working_directory =
            Some("/".repeat(MAX_WORKSPACE_METADATA_BYTES));
        let error = save_workspace_to_dir(&mut oversized, temp.path()).unwrap_err();
        assert!(format!("{error:#}").contains("byte limit"), "{error:#}");

        let loaded = load_workspace_from_dir(temp.path()).unwrap();
        assert!(!loaded.recovered_from_backup);
        assert_eq!(loaded.workspace.name, workspace.name);
        assert_eq!(
            loaded.workspace.tabs[0].panes[0].scrollback_lines,
            ["history-0"]
        );
    }

    #[test]
    fn a_save_cleans_staging_files_an_interrupted_write_left() {
        let temp = tempfile::tempdir().unwrap();
        let mut workspace = sample_workspace();
        save_workspace_to_dir(&mut workspace, temp.path()).unwrap();
        let outside = temp.path().join("outside");
        fs::write(&outside, "keep").unwrap();
        let leftovers = [
            temp.path().join(".termeric-interrupted"),
            temp.path().join("scrollback/.termeric-interrupted"),
        ];
        for leftover in &leftovers {
            fs::write(leftover, "partial").unwrap();
        }
        std::os::unix::fs::symlink(&outside, temp.path().join(".termeric-linked")).unwrap();

        save_workspace_to_dir(&mut workspace, temp.path()).unwrap();
        for leftover in leftovers
            .iter()
            .chain([&temp.path().join(".termeric-linked")])
        {
            assert!(
                fs::symlink_metadata(leftover).is_err(),
                "{}",
                leftover.display()
            );
        }
        assert_eq!(fs::read_to_string(&outside).unwrap(), "keep");
        assert!(
            load_workspace_from_dir(temp.path())
                .unwrap()
                .warnings
                .is_empty()
        );
    }

    #[test]
    fn a_save_waits_while_another_holds_the_archive() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("workspace-locked");
        let mut workspace = sample_workspace();
        save_workspace_to_dir(&mut workspace, &dir).unwrap();

        // Another process's save or load holds the archive.
        let held = lock_archive(&dir, ArchiveLock::Read).unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let saving = {
            let dir = dir.clone();
            std::thread::spawn(move || {
                let result = save_workspace_to_dir(&mut workspace, &dir);
                sender.send(()).unwrap();
                result
            })
        };
        assert!(
            receiver
                .recv_timeout(std::time::Duration::from_millis(200))
                .is_err(),
            "the save wrote the archive while it was locked"
        );
        drop(held);
        receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        saving.join().unwrap().unwrap();
    }

    #[test]
    fn delete_refuses_a_symlinked_workspace_before_touching_its_target() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().join("workspaces");
        let outside = temp.path().join("outside");
        fs::create_dir(&base).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep"), "present").unwrap();
        symlink(&outside, base.join("workspace-linked")).unwrap();
        set_test_workspaces_base_dir(Some(base));
        let result = std::panic::catch_unwind(|| {
            let error = delete_workspace("workspace-linked").unwrap_err();
            assert!(format!("{error:#}").contains("symlinked workspace"));
            assert_eq!(fs::read_to_string(outside.join("keep")).unwrap(), "present");
        });
        set_test_workspaces_base_dir(None);
        result.unwrap();
    }

    #[test]
    fn scrollback_reader_enforces_per_pane_and_remaining_total_budget() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("object.txt");
        fs::write(&path, "12345").unwrap();
        assert_eq!(read_bounded_scrollback(&path, 5).unwrap(), "12345");
        assert!(read_bounded_scrollback(&path, 4).is_err());
        assert!(read_bounded_scrollback(&path, 0).is_err());
    }

    #[test]
    fn unreadable_archives_remain_listed_for_deletion() {
        let temp = tempfile::tempdir().unwrap();
        set_test_workspaces_base_dir(Some(temp.path().to_path_buf()));
        let result = std::panic::catch_unwind(|| {
            let broken = temp.path().join("workspace-broken");
            fs::create_dir_all(&broken).unwrap();
            fs::write(broken.join("workspace.toml"), "not = [toml").unwrap();
            let entries = list_workspaces();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].id, "workspace-broken");
            assert!(entries[0].warning.is_some());
            delete_workspace("workspace-broken").unwrap();
            assert!(list_workspaces().is_empty());
        });
        set_test_workspaces_base_dir(None);
        result.unwrap();
    }

    #[test]
    fn unreadable_archives_are_listed_after_readable_ones() {
        let temp = tempfile::tempdir().unwrap();
        set_test_workspaces_base_dir(Some(temp.path().to_path_buf()));
        let result = std::panic::catch_unwind(|| {
            for id in ["workspace-broken-b", "workspace-broken-a"] {
                let broken = temp.path().join(id);
                fs::create_dir_all(&broken).unwrap();
                fs::write(broken.join("workspace.toml"), "not = [toml").unwrap();
            }
            for (id, updated_at) in [
                ("workspace-older", "2026-01-01T00:00:00Z"),
                ("workspace-newer", "2026-02-01T00:00:00Z"),
            ] {
                let mut workspace = sample_workspace();
                workspace.id = id.into();
                save_workspace(&mut workspace).unwrap();
                // Saving stamps the archive with the current time; set the
                // times apart so the order between them is known.
                let path = temp.path().join(id).join("workspace.toml");
                let mut encoded: toml::Value =
                    toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
                encoded["updated_at"] = toml::Value::String(updated_at.into());
                fs::write(&path, toml::to_string_pretty(&encoded).unwrap()).unwrap();
            }
            let order: Vec<_> = list_workspaces()
                .into_iter()
                .map(|entry| entry.id)
                .collect();
            assert_eq!(order[..2], ["workspace-newer", "workspace-older"]);
            let mut unreadable = order[2..].to_vec();
            unreadable.sort();
            assert_eq!(unreadable, ["workspace-broken-a", "workspace-broken-b"]);
        });
        set_test_workspaces_base_dir(None);
        result.unwrap();
    }

    #[test]
    fn missing_scrollback_and_corrupt_tab_recover_healthy_components() {
        let temp = tempfile::tempdir().unwrap();
        let mut workspace = sample_workspace();
        workspace.tabs.push(workspace.tabs[0].clone());
        workspace.tabs[1].id = 8;
        save_workspace_to_dir(&mut workspace, temp.path()).unwrap();
        let missing = workspace.tabs[0].panes[0].scrollback_file.clone().unwrap();
        fs::remove_file(temp.path().join(missing)).unwrap();

        let mut encoded: toml::Value =
            toml::from_str(&fs::read_to_string(temp.path().join("workspace.toml")).unwrap())
                .unwrap();
        encoded["tabs"][1]["focused_pane"] = toml::Value::Integer(999);
        fs::write(
            temp.path().join("workspace.toml"),
            toml::to_string_pretty(&encoded).unwrap(),
        )
        .unwrap();

        let loaded = load_workspace_from_dir(temp.path()).unwrap();
        assert_eq!(loaded.workspace.tabs.len(), 1);
        assert!(
            loaded
                .warnings
                .iter()
                .any(|warning| warning.contains("tab 8"))
        );
        assert!(
            loaded
                .warnings
                .iter()
                .any(|warning| warning.contains("scrollback was skipped"))
        );
    }

    #[test]
    fn corrupt_preceding_tab_does_not_retarget_a_healthy_active_tab() {
        let temp = tempfile::tempdir().unwrap();
        let mut workspace = sample_workspace();
        for id in [8, 9] {
            let mut tab = workspace.tabs[0].clone();
            tab.id = id;
            workspace.tabs.push(tab);
        }
        workspace.active_tab = 1;
        save_workspace_to_dir(&mut workspace, temp.path()).unwrap();

        let mut encoded: toml::Value =
            toml::from_str(&fs::read_to_string(temp.path().join("workspace.toml")).unwrap())
                .unwrap();
        encoded["tabs"][0]["focused_pane"] = toml::Value::Integer(999);
        fs::write(
            temp.path().join("workspace.toml"),
            toml::to_string_pretty(&encoded).unwrap(),
        )
        .unwrap();

        let loaded = load_workspace_from_dir(temp.path()).unwrap();
        assert_eq!(
            loaded
                .workspace
                .tabs
                .iter()
                .map(|tab| tab.id)
                .collect::<Vec<_>>(),
            [8, 9]
        );
        assert_eq!(loaded.workspace.active_tab, 0);
        assert_eq!(
            loaded.workspace.tabs[loaded.workspace.active_tab].id, 8,
            "focus must follow the surviving source tab instead of its stale list index"
        );
        assert!(
            loaded
                .warnings
                .iter()
                .all(|warning| !warning.contains("active tab"))
        );
    }

    #[test]
    fn save_times_are_readable_and_strictly_increasing() {
        let stamps: Vec<_> = (0..50).map(|_| save_timestamp()).collect();
        assert!(
            stamps.windows(2).all(|pair| pair[0] < pair[1]),
            "{stamps:?}"
        );
        for stamp in &stamps {
            assert_eq!(stamp.len(), "2026-01-01T00:00:00.000Z".len(), "{stamp}");
            assert!(
                crate::session::parse_utc_timestamp(stamp).is_some(),
                "{stamp}"
            );
        }
    }

    #[test]
    fn only_the_newest_closed_tabs_are_kept_and_workspaces_never_pruned() {
        let temp = tempfile::tempdir().unwrap();
        set_test_workspaces_base_dir(Some(temp.path().to_path_buf()));
        let save = |index: usize, closed_tab: bool| {
            let mut workspace = sample_workspace();
            workspace.id = format!("workspace-{index:04}");
            workspace.closed_tab = closed_tab;
            // Saving stamps the time, so each save is newer than the last.
            save_workspace(&mut workspace).unwrap();
        };
        save(0, false);
        for index in 1..=MAX_CLOSED_TABS + 2 {
            save(index, true);
        }
        let pruned = prune_closed_tabs();
        let closed = list_closed_tabs();
        let workspaces = list_workspaces();
        set_test_workspaces_base_dir(None);
        assert_eq!(pruned, 2);
        assert_eq!(closed.len(), MAX_CLOSED_TABS);
        assert_eq!(closed.last().unwrap().id, "workspace-0003");
        assert_eq!(workspaces.len(), 1);
        assert_eq!(workspaces[0].id, "workspace-0000");
    }

    #[test]
    fn a_copied_archive_is_never_taken_for_its_original() {
        let temp = tempfile::tempdir().unwrap();
        set_test_workspaces_base_dir(Some(temp.path().to_path_buf()));
        let result = std::panic::catch_unwind(|| {
            for index in 0..MAX_CLOSED_TABS {
                let mut workspace = sample_workspace();
                workspace.id = format!("workspace-{index:04}");
                workspace.closed_tab = true;
                save_workspace(&mut workspace).unwrap();
            }
            // Copy the oldest snapshot, which pruning would reach first.
            let original = temp.path().join("workspace-0000");
            let copy = temp.path().join("workspace-0000copy");
            fs::create_dir(&copy).unwrap();
            for name in ["workspace.toml", "workspace.toml.bak"] {
                if original.join(name).exists() {
                    fs::copy(original.join(name), copy.join(name)).unwrap();
                }
            }

            assert_eq!(list_closed_tabs().len(), MAX_CLOSED_TABS);
            assert_eq!(prune_closed_tabs(), 0);
            assert!(original.join("workspace.toml").exists());
            assert!(load_workspace("workspace-0000").is_ok());
            let error = load_workspace("workspace-0000copy").unwrap_err();
            assert!(format!("{error:#}").contains("not its own"), "{error:#}");
            // It stays visible, under its own name, so it can be deleted.
            let listed = list_workspaces();
            assert_eq!(listed.len(), 1);
            assert_eq!(listed[0].id, "workspace-0000copy");
            assert!(listed[0].warning.is_some());
            delete_workspace("workspace-0000copy").unwrap();
            assert!(original.join("workspace.toml").exists());
        });
        set_test_workspaces_base_dir(None);
        result.unwrap();
    }

    #[test]
    fn a_stale_copy_of_an_automatic_archive_never_prunes_the_kept_original() {
        let temp = tempfile::tempdir().unwrap();
        set_test_workspaces_base_dir(Some(temp.path().to_path_buf()));
        let result = std::panic::catch_unwind(|| {
            let mut workspace = sample_workspace();
            workspace.id = "workspace-automatic".into();
            workspace.automatic = true;
            save_workspace(&mut workspace).unwrap();
            let original = temp.path().join("workspace-automatic");
            let copy = temp.path().join("workspace-automaticcopy");
            fs::create_dir(&copy).unwrap();
            fs::copy(original.join("workspace.toml"), copy.join("workspace.toml")).unwrap();
            // Renaming makes the original the person's; the copy still
            // says it is automatic.
            rename_workspace("workspace-automatic", "Mine now").unwrap();

            assert_eq!(prune_automatic_workspaces(&[]), 0);
            assert!(original.join("workspace.toml").exists());
            assert!(copy.join("workspace.toml").exists());
        });
        set_test_workspaces_base_dir(None);
        result.unwrap();
    }

    #[test]
    fn only_the_current_schema_loads() {
        let temp = tempfile::tempdir().unwrap();
        for version in [WORKSPACE_SCHEMA_VERSION - 1, WORKSPACE_SCHEMA_VERSION + 1] {
            let mut workspace = sample_workspace();
            workspace.version = version;
            fs::write(
                temp.path().join("workspace.toml"),
                toml::to_string_pretty(&workspace).unwrap(),
            )
            .unwrap();
            assert!(load_workspace_from_dir(temp.path()).is_err());
        }
    }

    #[test]
    fn list_rename_delete_and_truncation_are_explicit() {
        let temp = tempfile::tempdir().unwrap();
        set_test_workspaces_base_dir(Some(temp.path().to_path_buf()));
        let result = std::panic::catch_unwind(|| {
            let mut workspace = sample_workspace();
            workspace.tabs[0].panes[0].scrollback_lines =
                vec!["x".repeat(MAX_WORKSPACE_SCROLLBACK_BYTES_PER_PANE)];
            let report = save_workspace(&mut workspace).unwrap();
            assert_eq!(report.truncated_panes, vec![(7, 0)]);
            assert_eq!(list_workspaces()[0].name, "Asymmetric roots");
            rename_workspace("workspace-1", "Renamed roots").unwrap();
            assert_eq!(
                load_workspace("workspace-1").unwrap().workspace.name,
                "Renamed roots"
            );
            assert!(rename_workspace("workspace-1", "../bad\nname").is_err());
            delete_workspace("workspace-1").unwrap();
            assert!(list_workspaces().is_empty());
        });
        set_test_workspaces_base_dir(None);
        result.unwrap();
    }

    #[test]
    fn only_unrestorable_automatic_archives_are_pruned() {
        let temp = tempfile::tempdir().unwrap();
        set_test_workspaces_base_dir(Some(temp.path().to_path_buf()));
        let result = std::panic::catch_unwind(|| {
            for (id, automatic) in [
                ("workspace-old", true),
                ("workspace-last", true),
                ("workspace-renamed", true),
                ("workspace-named", false),
            ] {
                let mut workspace = sample_workspace();
                workspace.id = id.into();
                workspace.automatic = automatic;
                save_workspace(&mut workspace).unwrap();
            }
            rename_workspace("workspace-renamed", "Mine now").unwrap();

            assert_eq!(prune_automatic_workspaces(&["workspace-last".into()]), 1);
            let mut kept: Vec<_> = list_workspaces()
                .into_iter()
                .map(|entry| entry.id)
                .collect();
            kept.sort();
            assert_eq!(
                kept,
                ["workspace-last", "workspace-named", "workspace-renamed"]
            );
            assert_eq!(prune_automatic_workspaces(&["workspace-last".into()]), 0);
        });
        set_test_workspaces_base_dir(None);
        result.unwrap();
    }

    #[test]
    fn listing_reads_metadata_without_touching_scrollback() {
        let temp = tempfile::tempdir().unwrap();
        set_test_workspaces_base_dir(Some(temp.path().to_path_buf()));
        let result = std::panic::catch_unwind(|| {
            let mut workspace = sample_workspace();
            workspace.tabs[0].panes[0].scrollback_lines = vec!["history".into()];
            save_workspace(&mut workspace).unwrap();
            let scrollback = workspace_dir(&workspace.id).unwrap().join(
                workspace.tabs[0].panes[0]
                    .scrollback_file
                    .as_deref()
                    .expect("saved scrollback"),
            );
            // Loading notices a broken scrollback file; listing never opens one.
            fs::remove_file(&scrollback).unwrap();
            fs::create_dir(&scrollback).unwrap();
            assert!(!load_workspace(&workspace.id).unwrap().warnings.is_empty());
            let listed = list_workspaces();
            assert_eq!(listed.len(), 1);
            assert!(listed[0].warning.is_none());
            assert_eq!(listed[0].pane_count, workspace.tabs[0].panes.len());
        });
        set_test_workspaces_base_dir(None);
        result.unwrap();
    }

    #[test]
    fn bounded_scrollback_keeps_the_most_recent_complete_lines() {
        let (payload, truncated) = bounded_scrollback(
            &["old".into(), "middle".into(), "new".into()],
            "middle\nnew\n".len(),
        );
        assert!(truncated);
        assert_eq!(String::from_utf8(payload).unwrap(), "middle\nnew\n");
    }
}
