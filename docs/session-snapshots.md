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
Metadata is replaced atomically while retaining the original creation time.

## Regression evidence

Linux PTY coverage gives two panes distinct cwd, command, and terminal history,
then changes focus and verifies every captured field changes together. The
selected snapshot is serialized and loaded again. A simultaneous two-pane exit
then verifies close-time autosave persists only the focused command/history
before topology cleanup. Separate tests exercise the exact 5 MiB boundary,
versions 1 and 3, current focused-pane files, malformed files, newer versions,
missing scrollback, and traversal-resistant filenames.
