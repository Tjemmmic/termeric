//! Process details from `/proc`.

use std::path::PathBuf;

pub(super) fn working_directory(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

/// The field after the name in `/proc/PID/stat`. The name may itself hold
/// spaces and parentheses, so the fields are read after its last closing
/// parenthesis.
pub(super) fn parent(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields = &stat[stat.rfind(')')? + 1..];
    fields.split_whitespace().nth(1)?.parse().ok()
}

/// The last entry of the main thread's children, which the kernel lists in
/// the order they were started.
pub(super) fn newest_child(pid: u32) -> Option<u32> {
    let children = std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children")).ok()?;
    children.split_whitespace().last()?.parse().ok()
}

pub(super) fn name(pid: u32) -> Option<String> {
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    Some(comm.trim().to_owned())
}

pub(super) fn arguments(pid: u32) -> Option<Vec<String>> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    Some(
        raw.split(|byte| *byte == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part).into_owned())
            .collect(),
    )
}
