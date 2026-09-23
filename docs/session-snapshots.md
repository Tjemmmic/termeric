# Session snapshot contract

Termeric sessions are bounded, versioned snapshots. Restoring one starts a new
shell with saved metadata and replays saved text history; it never resurrects a
process, PTY, tab set, or pane tree. Use the separate complete workspace format
for topology-preserving relaunches; neither format resurrects processes.

## Capture policy

Snapshots are written automatically when a tab closes; there is no explicit
save command. Every save path uses `Tab::save_as_session()` and one stable
capture of the terminal selected at the start of that save:

- a normal tab captures its single live terminal;
- a split tab captures only its currently focused pane;
- changing focus changes the source of the next save;
- cwd, last foreground command, and scrollback always come from that same
  source; and
- ordinary tab close and whole-split close use the same policy and report
  type.

When every process in a split exits during one event-loop poll, close-time
autosave captures the still-addressable focused pane before destructive pane
topology cleanup. This prevents the final save from observing a detached tab
placeholder after the pane manager has been discarded.

The cwd uses the focused terminal's local-authority OSC 7 report when present,
then falls back to platform process inspection. Remote-authority reports are
never serialized as local launch directories.

The Workspace Manager's **Tab snapshots** view lists these entries and notes
that opening one starts a new shell. A focused-pane snapshot is labeled as
such. Complete pane/workspace persistence uses the
separate contract in [workspaces.md](workspaces.md), rather than an ambiguous
extension of this format.

Plugin transcript JSONL is not copied into a terminal session snapshot. It has
its own bounded recorder and export lifecycle. Plugin tab metadata remains tab
metadata; only the focused terminal's cwd, command, and text history enter the
session file.

## Storage and compatibility

New saves use format version 5: **one self-contained `session.toml`** publishes
metadata and text together. Every close-time save calls the same storage
transaction. A failed first save leaves the tab's session ID
unset; an update failure preserves the previous complete snapshot.

The file begins with a TOML comment,
`# termeric-snapshot-v5 metadata-bytes=NNNNNNNN`, followed by a newline, exactly
that many UTF-8 metadata bytes, and a `scrollback_base64` string inside the final
`[snapshot_content]` table. Metadata includes decoded text length and SHA-256.
The metadata-only reader bounds and reads the header without loading the base64
body; title listings therefore do not decode multi-megabyte text. Listing checks
the declared total file length, **not** the payload checksum. Actual restore
validates framing, decoded length, SHA-256, and UTF-8 before creating a PTY.

Metadata remains limited to 1 MiB. Text remains limited to exactly 5 MiB,
retaining whole lines from the start of captured history; the first line that
would exceed the cap and all subsequent lines are omitted with a visible
truncation report. Empty/disabled captures embed empty text. Base64 expansion
plus framing gives a maximum container size of **8,039,155 bytes**. A valid
same-ID update preserves the original creation time. Malformed, incompatible,
unreadable, or differently identified existing metadata fails closed rather
than being overwritten.

### Publication, readers, and bounded recovery

Writers take a **nonblocking**, per-session OS file lock on the persistent
`.snapshot-v5.lock` inode. A competing writer receives a visible retryable save
error; the UI never waits for that lock. One `.snapshot-v5.pending` file is
created exclusively, written, flushed, and synced, then atomically replaces
`session.toml`. The lock is never unlinked by a save. The names are reserved for
this protocol; neither file is opened with truncation. On Unix the files are
private (`0600`), and lock/recovery validation requires a regular, owned,
non-hard-linked file without group/other permissions.

The writer owns an explicit scope guard: it unlocks before closing its file on
normal return, error, or unwinding. On Linux, merely closing one descriptor can
leave a duplicate (including one inherited around concurrent process creation)
holding the same advisory lock. Explicit unlock prevents that duplicate from
extending a completed save's lock lifetime. Unlock failures are logged without
snapshot paths/content and do not panic during cleanup; they are not hidden by
blocking retries or replacement of the persistent lock inode.

A reader uses one open descriptor for metadata and embedded text. Replacement
cannot mix generations or remove that reader's data. If a selected metadata
header differs when reopened for restore, restore reports an explicit stale
selection error: refresh/retry; it does not silently switch captures. This check
occurs before allocating a tab ID or starting a shell. Identical content and
metadata are equivalent selections, even if republished.

Normal errors clean up the owned staging file. After an interrupted save, the
next writer under the same lock may recover only the reserved pending file if
it meets ownership/type/size checks and has a full or partial expected magic
prefix (including empty, for a crash before the first write). Unrecognized,
oversized, symlinked, or otherwise unsafe entries fail closed and remain for
manual inspection. No directory sweep or recursive cleanup runs during save.

There is **one current container, at most one pending container, and one empty
lock file per session**: at most 16,078,310 bytes of protocol-managed named file
content. There are no backup generations or automatic rollback history. An
already open reader can temporarily pin one additional old container, at most
8,039,155 bytes per in-flight reader, until its descriptor closes. Saving does
not accumulate named generations. Existing legacy text and unrelated files,
including staging artifacts from earlier implementations, are not silently
deleted and are outside this new managed-file budget.

This selects the self-contained-container option in
[#107](https://github.com/Tjemmmic/termeric/issues/107): roughly one-third text
encoding overhead buys a single publication point and eliminates generation
garbage collection. A bounded metadata header avoids the listing-cost drawback.
Two independent renames cannot provide this contract; immutable text generations
would instead require retention and stale-reader garbage-collection machinery.

Directory sync after publication remains best effort; failure is logged, not
reported as though publication had not happened. Atomic replacement is not a
power-loss/filesystem-corruption guarantee. File locks coordinate cooperating
v5 writers, not external in-place editors or binaries predating this protocol.

### Legacy compatibility and rollback

Versions 1–4 remain readable without rewriting files. Version 4's `capture_scope`
still distinguishes `single_terminal` and `focused_pane`; earlier versions
default to `single_terminal`. Their separate scrollback file retains the 5-MiB
bounded reader. Concurrent old-binary saves retain the old two-file limitation;
do not run old and new writers against the same session directory.

Only saving upgrades an old snapshot to v5. Its old `scrollback.txt` is left
untouched, but it is no longer referenced. V5 remains valid TOML, so older
binaries reject its newer version (or their older metadata-size ceiling),
rather than restoring it as a v4 snapshot. For rollback, preserve an external
copy of the **complete v1–v4 session directory before saving with v5**. The
leftover text alone is not a backup of the old metadata. There is no automatic
down-converter or backup created during upgrade.

All readers use bounded regular-file opens. Unix nonblocking/no-follow flags
reject final symlinks and allow FIFO rejection without waiting for a writer;
a broken metadata symlink is an error, not an absent snapshot. Lists fall back
to the session ID for unreadable metadata. These checks do not confine
user-selected parent directories or establish a non-Unix symlink-race guarantee.
See [#106](https://github.com/Tjemmmic/termeric/issues/106).

Snapshot capture and bounded filesystem operations remain synchronous. This
change avoids blocking lock acquisition, not slow ordinary filesystem calls;
it is not the background transcript-export worker. Byte ceilings also are not
total-RSS limits for parsed metadata, decoded text, or line vectors.

## Regression evidence

Linux PTY coverage gives two panes distinct cwd, command, and terminal history,
then changes focus and verifies every captured field changes together. The
selected snapshot is serialized and loaded again. A simultaneous two-pane exit
then verifies close-time autosave persists only the focused command/history
before topology cleanup. Storage tests inject a failure after complete staging
and before commit; read at both publication boundaries; retain an old open
descriptor across replacement; reject stale selections before tab creation;
exercise independent-process lock contention and concurrent writers/readers;
keep duplicate descriptors alive across normal/error/unwind guard exits while
checking immediate reacquisition, continued exclusion by the next writer, and
the same persistent inode; verify failed save hooks clean staging and preserve
the previous snapshot;
and repeat interrupted staging at every magic-prefix boundary with constant file
counts. Exact 5-MiB production writes, metadata overflow, corrupt payloads,
symlink/permission checks, v1–v4 upgrades, and first-save identity failure are
covered in private fixtures. No GUI or provider-backed execution is required.

Private sentinel regressions reproduce both original symlink-write defects and
verify the targets stay unchanged after saving. Shared production I/O tests
cover partial-staging and publication failure, cleanup, concurrent replacement
without mixed bytes, private output permissions, exact read ceilings, invalid
UTF-8, growth beyond inspected length, and bounded FIFO rejection. Restore and
both legacy listing surfaces are checked against an oversized metadata fixture;
production scrollback restore enforces the writer's 5 MiB ceiling. No native
GUI or provider execution is required by these fixtures.
