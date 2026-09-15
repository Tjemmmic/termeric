# Complete workspace persistence

Termeric workspaces are versioned relaunch plans, not process checkpoints. A
restore starts fresh shell, terminal-profile, and plugin processes and
reconstructs their terminal organization. File descriptors, process memory,
shell jobs, remote connections, and authenticated AI conversations are never
described as restored.

## Archive contract

Each workspace has its own directory in the platform data directory under
`termeric/workspaces/<workspace-id>/`. `workspace.toml` records schema version
3, stable ID and display name, timestamps, the active tab, informational theme
and shell references, and up to 64 ordered tabs. A tab records:

- shell, reusable terminal-profile, or versioned plugin launch type and its
  stable profile reference;
- effective title, optional user-title override, pinned order, explicit launch
  primary, and focused pane;
- a recursive pane tree with horizontal/vertical direction and exact split
  ratios from 0.1 through 0.9;
- up to 64 pane records with stable IDs, their exact shell, named-profile, or plugin
  relaunch identity, OSC title, cwd, last-command metadata, and an optional
  scrollback object reference.

Schema 0/1 archives did not carry a launch identity per pane. Migration assigns
the tab launch to its primary pane, the named tab profile to historical profile
splits, and the configured shell to historical shell/plugin splits. Schema 2
identities migrate unchanged. Schema 3 requires every pane identity and permits
mixed shell/profile/plugin layouts, including distinct profiles of the same
plugin and different plugins. Migration never invents sibling plugin launches.
The explicit launch primary is preserved even when its ID is not zero.
Explicit plugin-pane creation and plugin-pane movement remain guarded pending
the remaining work in [#126](https://github.com/Tjemmmic/termeric/issues/126).

Each archive represents one native window. Process-level application state
stores the ordered set of window workspace IDs so automatic startup can
recreate multiple windows without coupling their tab/pane archives. The schema
deliberately remains separate from legacy focused-terminal session snapshots.
Transient pane zoom and broadcast state are deliberately not archived; the
underlying split tree is always the saved layout. See
[advanced workspace controls](advanced-workspaces.md).

## Durability and limits

Metadata is written to a same-directory temporary file, synchronized, and
atomically renamed. Before replacing an existing archive, the previous metadata
is retained as `workspace.toml.bak`; loading falls back to it when the primary
is missing or corrupt. Interrupted `.tmp` files are ignored. Workspace,
scrollback, metadata, backup, and temporary paths must be real files/directories,
not symlinks; save, load, and delete reject substitutions before following or
mutating them. Renaming an archive recovered from its backup installs repaired
primary metadata without replacing that last known-good backup with the corrupt
primary; a later write interruption or second primary failure therefore remains
recoverable.

Optional scrollback keeps the most recent complete lines, with a 5 MiB limit
per pane and 50 MiB logical limit per workspace. Identical content is stored
once using a deterministic content/length name with collision verification.
Restore reapplies both byte limits before allocating or parsing an object, even
when corrupted metadata references the same object repeatedly. Truncation is
recorded per pane and reported after explicit saves. Metadata is limited to 5
MiB, identifiers and object paths are validated, and pane trees have bounded
leaf count and nesting depth.

## Recovery behavior

Restore validates and migrates metadata before launching anything. Corrupt tabs
and missing scrollback objects are skipped independently. An unavailable cwd
falls back to the process default with a warning, never to a focused sibling's
cwd. A whole tab is skipped when any pane's built-in plugin, named profile, or
executable is unavailable; other tabs
still restore. Filtering an earlier corrupt tab remaps focus to the exact
surviving source tab rather than retaining a stale list index. If the saved
focused tab itself cannot restore, the first recovered tab receives focus. If
no tab can be relaunched, the current live workspace is left untouched.

A terminal-profile tab is likewise skipped if its named `[profiles.NAME]`
entry was removed or its serial device cannot be opened. Serial profiles remain
single-pane by contract and are never cloned during restore. The same rule
applies when a process-profile pane was moved into a
different shell/profile tab: if any referenced pane profile is unavailable,
that tab is skipped rather than silently launching the pane with another
command. Workspace archives store names rather than copying argv or environment
values, so edited profiles take effect on the next restore and secrets are not
duplicated into workspace metadata. See the
[terminal-profile contract](terminal-profiles.md).

Saved theme and shell values are references, not embedded configuration. When
they differ from the current configuration, restore keeps the current safe
configuration and reports the mismatch. Plugin profile references are stricter:
an explicitly saved profile must still exist so restoration cannot silently
launch a materially different permission or sandbox policy.

Every pane launch is resolved before spawning that tab's PTYs. Plugin restore
hooks are dispatched only to the named plugin and must return exactly one
matching launch with the saved profile and that pane's usable cwd. Ambiguous
responses, notices indicating configuration fallback, and unrelated actions
are rejected without executing those actions. An unspecified profile may
resolve to the current configured default; its resolved reference is retained
for the next save. A configured profile literally named `default` remains an
explicit reference. Plugin panes bypass interactive-shell hook injection, and
hook-provided `PWD` cannot override the saved cwd or missing-cwd fallback.

Actual PTY creation checks executable availability; restoration does not run an
extra `--version` probe. A spawn/layout failure drops that tab's newly created
owners immediately, without close animations or disturbing already recovered
tabs. This cannot undo external side effects of processes already started.
Restored plugin sessions start with fresh metadata; active recorders, pending
diffs, and provider conversations are not resumed. Fake-executable tests prove
local launch and archive behavior, not provider readiness or native appearance.

## User workflow

Use **Save Workspace** and **Workspace Manager** in the command palette. The
manager lists archives and provides Load, Rename, and Delete controls. Rename is
an inline text field: Enter submits and Escape cancels. The existing `[session]`
configuration keys retain their names for compatibility; `auto_save` now saves
each complete window workspace on normal application exit, `auto_restore`
restores the stable ordered workspace-ID set recorded in application state, and
`save_scrollback` controls workspace history capture. Startup never guesses the
newest directory by timestamp.

Legacy **Save Open Tabs as Legacy Snapshots** remains available for interoperating with
the older single-terminal snapshot format. Those entries still restore as new
shells and intentionally do not preserve complete topology.
