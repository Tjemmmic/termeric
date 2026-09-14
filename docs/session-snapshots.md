# Session snapshot contract

Termeric sessions are bounded, versioned snapshots. Restoring one starts a new
shell with saved metadata and replays saved text history; it never resurrects a
process, PTY, tab set, or pane tree. Use the separate complete workspace format
for topology-preserving relaunches; neither format resurrects processes.

## Capture policy

Every save entry point uses `Tab::save_as_session()` and one stable capture of
the terminal selected at the start of that save:

- a normal tab captures its single live terminal;
- a split tab captures only its currently focused pane;
- changing focus changes the source of the next save;
- cwd, last foreground command, and scrollback always come from that same
  source; and
- explicit save, configured automatic save, and close-time automatic save use
the same policy and report type.

When every process in a split exits during one event-loop poll, close-time
autosave captures the still-addressable focused pane before destructive pane
topology cleanup. This prevents the final save from observing a detached tab
placeholder after the pane manager has been discarded.

The cwd uses the focused terminal's local-authority OSC 7 report when present,
then falls back to platform process inspection. Remote-authority reports are
never serialized as local launch directories.

The session manager labels restored entries as new shells. A focused-pane
snapshot is also labeled as such, and an explicit split-tab save reports that
only focused panes were captured. Complete pane/workspace persistence uses the
separate contract in [workspaces.md](workspaces.md), rather than an ambiguous
extension of this format.

Plugin transcript JSONL is not copied into a terminal session snapshot. It has
its own bounded recorder and export lifecycle. Plugin tab metadata remains tab
metadata; only the focused terminal's cwd, command, and text history enter the
session file.

## Storage and compatibility

Format version 4 adds `capture_scope`, whose values are `single_terminal` and
`focused_pane`. The field has a `single_terminal` default, so versions 1–3
continue loading without rewriting their files. Files newer than the supported
version and malformed TOML fail visibly instead of being partially inferred.

Scrollback is written once, one complete line at a time, to `scrollback.txt`.
The file is capped at exactly 5 MiB; a line that would cross the boundary is not
partially written, and the save result produces a visible truncation warning.
Metadata is limited to 1 MiB and retains the original creation time for a valid
same-ID update. Oversized metadata saves fail without replacing the old file.

Both files use exclusively created, same-directory staging files (mode `0600`
on Unix), buffered writes, flush/sync, and atomic **per-file** replacement. A
failed write or publication preserves the prior final file and cleans up only
the owned staging file. The predictable `session.toml.tmp` name is no longer
used. A final output symlink can be replaced itself, but its target is never
opened for truncation. Existing unrelated staging-name entries are left alone.
Directory sync after publication is best effort; a failure is logged rather
than falsely reporting that an already-replaced file was not published.

Restore, creation-time preservation, the title list, and the legacy session
panel share bounded regular-file reads: 1 MiB for metadata and 5 MiB for
scrollback. The read itself permits at most one extra probe byte, so growth
after metadata inspection cannot evade the limit. Oversized/invalid UTF-8 input
is rejected rather than silently truncated. On Unix, nonblocking/no-follow
opens reject final-component symlinks and let descriptor validation reject a
FIFO without waiting for a writer. A broken metadata symlink is an error, not
an absent snapshot. Lists retain a safe session-ID fallback when metadata is
unreadable. These checks do not confine user-selected parent directories or
establish a non-Unix symlink-race guarantee. See
[#106](https://github.com/Tjemmmic/termeric/issues/106).

### Remaining transaction boundary

`scrollback.txt` and `session.toml` are still published separately. A failure
after scrollback replacement but before metadata replacement, or a concurrent
reader at that boundary, can observe metadata and text from different saves.
Consequently legacy snapshots remain **Partial** until
[#107](https://github.com/Tjemmmic/termeric/issues/107) establishes a complete
generation, concurrency, retention, and restore contract. Atomic replacement
of each file alone does not fix that problem.

Snapshot capture and filesystem operations also remain synchronous. Nonblocking
open flags do not make ordinary file reads/writes or metadata calls real-time,
and byte ceilings are not total-RSS limits for parsed metadata or line vectors.
These are separate boundaries from background transcript export and complete
workspace archives.

## Regression evidence

Linux PTY coverage gives two panes distinct cwd, command, and terminal history,
then changes focus and verifies every captured field changes together. The
selected snapshot is serialized and loaded again. A simultaneous two-pane exit
then verifies close-time autosave persists only the focused command/history
before topology cleanup. Separate tests exercise the exact 5 MiB boundary,
versions 1 and 3, current focused-pane files, malformed files, newer versions,
missing scrollback, and traversal-resistant filenames.

Private sentinel regressions reproduce both original symlink-write defects and
verify the targets stay unchanged after saving. Shared production I/O tests
cover partial-staging and publication failure, cleanup, concurrent replacement
without mixed bytes, private output permissions, exact read ceilings, invalid
UTF-8, growth beyond inspected length, and bounded FIFO rejection. Restore and
both legacy listing surfaces are checked against an oversized metadata fixture;
production scrollback restore enforces the writer's 5 MiB ceiling. No native
GUI or provider execution is required by these fixtures.
