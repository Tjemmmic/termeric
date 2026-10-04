use super::{
    WORKSPACE_SCHEMA_VERSION, WorkspaceSnapshot, validate_workspace_id, validate_workspace_name,
};
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::fs;
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

    workspace.updated_at = crate::session::format_unix_timestamp(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        None,
    );
    workspace.validate()?;
    let metadata = toml::to_string_pretty(workspace)?;
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
    load_workspace_from_dir(&dir)
}

pub fn load_workspace_from_dir(dir: &Path) -> Result<WorkspaceLoadReport> {
    let (mut workspace, recovered_from_backup, mut warnings) = read_workspace_metadata(dir)?;
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

pub fn list_workspaces() -> Vec<WorkspaceListEntry> {
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
            match read_workspace_metadata(&path) {
                Ok((workspace, ..)) => Some(WorkspaceListEntry {
                    pane_count: workspace.tabs.iter().map(|tab| tab.panes.len()).sum(),
                    tab_count: workspace.tabs.len(),
                    id: workspace.id,
                    name: workspace.name,
                    updated_at: workspace.updated_at,
                    warning: None,
                }),
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
    workspaces.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    workspaces
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
        .filter_map(|entry| read_workspace_metadata(&entry.path()).ok())
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
    let mut report = load_workspace_from_dir(&dir)?;
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
    fs::remove_dir_all(&dir)
        .with_context(|| format!("delete workspace directory {}", dir.display()))
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
