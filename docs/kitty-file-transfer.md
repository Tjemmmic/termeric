# Kitty terminal file transfer

Termeric implements a bounded Linux subset of Kitty's
[file-transfer protocol](https://sw.kovidgoyal.net/kitty/file-transfer-protocol/)
over OSC 5113. Kitty `send` lets a child process or remote shell place files on
the computer running the Termeric window. A narrower Kitty `receive` slice lets
that exact child read explicitly listed regular files after a separate
path-disclosing approval. These back the common
`kitten transfer remote-file /local/path` and regular-file
`kitten transfer --direction=upload /local/file remote-file` workflows.

The support status is **Partial** until a current native `kitten transfer` run
has accepted evidence. The parser, consent boundary, filesystem transaction,
and failure paths have hermetic coverage; native client acceptance is deferred.

## Supported profile

- One explicit approval per transfer session and direction, scoped to the exact
  originating tab or pane.
- Ordinary shell/profile terminals and attached live-domain panes on Linux.
- Regular files, directories, symbolic links, and hard links.
- Simple transmission with no compression or zlib compression. Compression EOF
  is independent of packet boundaries: a complete stream may be followed by
  empty `data` packets and an empty `end_data`. Only `end_data` completes the
  entry, and only `finish` publishes it. Truncation, checksum errors, trailing
  bytes, concatenated streams, and declared-size mismatches are rejected.
- `send`, `file`, `data`, `end_data`, `cancel`, and `finish`, with canonical
  status/progress replies in the requesting terminal's S7C1T/S8C1T mode.
- At most 4 active transfers, 1,024 entries per transfer, 4,096 decoded bytes
  per data command, 64 MiB of declared/uncompressed payload per transfer, a
  1,024-command worker queue, and 10 minutes of inactivity.
- File contents staged outside the visible destination, individual regular
  files published without overwrite, and best-effort transaction rollback if
  a later entry fails during `finish`.
- Mode bits limited to `0777`; setuid, setgid, and sticky bits are stripped.
  Directories use private `0700` assembly permissions, even with a restrictive
  umask, then receive the requested mode without adding owner permissions.
  On Linux, unreadable newly created directories are pinned with `O_PATH` and
  recovered through their trusted `/proc/self/fd` references, never through the
  original destination path. This recovery requires accessible procfs; failure
  does not fall back to a pathname chmod. Missing or mismatched handles fail
  without changing replacement directories or symlink targets.
  Final directory modes and modification times are applied deepest-first after
  their contents. Signed nanoseconds since the Unix epoch, including pre-1970
  times, are passed to the filesystem; its supported
  date range and timestamp precision still apply. Hermetic transaction tests
  check pre-epoch fractional seconds, zero, and positive times on files,
  directories, hard links, and symbolic links without following symlink targets.

For Kitty `receive` (host-to-child), the supported subset is intentionally
smaller:

- The start command must declare 1–16 path specifications. Termeric collects
  exactly that many unique IDs and unique lexical paths before showing a modal.
- The modal shows every host-resolved path. No requested path is opened or
  inspected before approval.
- Only UTF-8 regular files are exported. Directories, symbolic links, hard
  links, devices, sockets, and FIFOs fail closed; recursive traversal is not
  performed.
- No compression is emitted in the `receive` slice; zlib remains supported
  only when the child sends data to the host.
- Relative paths and `~/` resolve against the host home directory. Empty,
  traversal, repeated, display-control, oversized-component, and
  oversized-total paths fail before consent.
- Approved files are opened through Linux `openat2` with `RESOLVE_NO_SYMLINKS`,
  rejecting a link in any component and failing closed if the kernel cannot
  provide that guarantee. The opened descriptor must be regular and must match
  the path's re-read device and inode before metadata is returned.
- The aggregate opened size is capped at 64 MiB. Contents stream one file at a
  time in 4,096-byte protocol chunks. At most eight chunks enter one UI event,
  and the worker waits for exact-origin PTY acknowledgement before producing
  the next batch, so the event loop cannot accumulate a whole transfer.
- Metadata and payload use the session's captured S7C1T/S8C1T framing. A file
  can be requested once and its request must repeat the approved ID and path.

Relative destination paths and `~/` are resolved against the terminal host's
home directory, as required by the protocol. Absolute destinations are
accepted. Empty paths, NUL, traversal components, oversized components,
non-directory or symbolic-link ancestors, existing destination entries,
duplicates, and inconsistent sizes fail closed. Destination state is checked
when metadata arrives and again immediately before commit. Regular-file
publication uses no-clobber semantics. On Linux, its destination parent is
pinned with `openat2(RESOLVE_NO_SYMLINKS)` before publication, rejecting symlinks
in any parent component. Publication addresses that retained directory through
`/proc/self/fd`, so renaming or replacing a parent afterward cannot redirect
the destination write. If renamed, the original directory receives the file,
not the replacement at its old pathname. This requires `openat2` and accessible
procfs; failure does not retry the original destination path. The `O_PATH`
parent handle does not require directory read permission. See the
[Linux resolution contract](https://man7.org/linux/man-pages/man2/openat2.2.html).
Deterministic regressions replace both immediate parents and higher ancestors
with directories or symlinks, and verify retained file identity/metadata,
untouched replacements, and refusal to overwrite regular files or symlinks.
The source is also descriptor-bound: Linux publication uses
[`linkat(AT_SYMLINK_FOLLOW)`](https://man7.org/linux/man-pages/man2/link.2.html)
on the staging file's trusted `/proc/self/fd` reference, never its mutable name.
The filesystem must support hard links on the same mount. Kernel/filesystem
rejection fails without falling back to a pathname rename or copying from the
staging name. A renamed source can still publish its original inode; a removed
source with no remaining links fails closed. Regressions replace staging names
with regular files or symlinks and check destination bytes, inode, permissions,
timestamps, untouched replacements, and rejection of unlinked originals.

Staging cleanup on cancellation, failure, and successful publication compares
the remaining name's no-follow identity with the retained file before unlinking.
Unknown or replaced names are left alone, and moved staging files are not searched
for. This cleanup and transaction rollback remain best effort: neither
identity-check-then-unlink is atomic, and either may leave an orphan. Ancestor
races during other creation operations and same-inode content mutation by a
process with write authority also remain outside these publication guarantees.

Hard links may name only a regular file ID in the same transfer. On Linux,
creation uses that source's recorded regular-file handle after validating its
type and device/inode, not a fresh lookup of its destination name. It shares
regular-file publication's pinned destination parent, descriptor-based source,
procfs requirement, and atomic no-overwrite link operation. Bookkeeping records
the known source identity directly; subsequent link timestamps therefore apply
to the same original shared inode, even when its earlier name was replaced.
Missing, foreign, wrong-type, and unlinked source handles fail without a path
fallback or new rollback authority. Regressions cover replaced file/symlink
source names, unchanged replacement timestamps/content, invalid handles,
existing destinations, symbolic-link ancestors, and rollback preservation.
This does not close races in symbolic-link or directory creation, or make
rollback unlink atomic.

Symbolic links
support Kitty's `fid:`, `fid_abs:`, and `path:` forms. `fid` targets must name an
entry in the same transfer, including a directory. Relative and absolute links
can name entries declared later: their destination paths are resolved at commit
without dereferencing them. Hard links to directories and unknown target IDs
are rejected, with transaction rollback covered by local tests. A `path:` target
may intentionally point outside
the transfer so ordinary relative, absolute, and broken symlinks can be
preserved; the approval dialog calls out that the remote side controls symbolic
link targets. The link destination itself remains subject to every path and
no-overwrite check.

Linux rollback bookkeeping retains created entry identities through file
handles; symlink identities use metadata-only, no-follow handles.
Retained directory handles let rollback restore owner access top-down before
removing children after restrictive final modes have been applied; this does not
chmod a replacement directory or follow a replacement link. Final directory
permissions and timestamps also use those handles, after checking their recorded
device/inode identity; missing or mismatched handles fail rather than falling
back to a path lookup. Replacement-directory and replacement-symlink regressions
verify that only the original directory receives metadata. Regular files receive
timestamps and permissions through the retained staging handle and are synced
before no-overwrite publication; there is no destination-path timestamp update
after publication. Regressions cover metadata before publication, absent and
signed timestamps, restrictive modes, replacement paths, and unchanged existing
destinations after publication refusal. On Linux, link timestamps use the
recorded metadata-only handle with `utimensat(AT_EMPTY_PATH | AT_SYMLINK_NOFOLLOW)`
after checking type and device/inode identity. This requires Linux 5.8 or newer
when link timestamps are supplied; missing handles, kernel rejection, and stale
identity fail without a pathname fallback. Tests cover hard-link shared-inode
semantics, untouched symlink targets and replacement paths, absent timestamps,
invalid guards, and dangling links whose names were removed.
Bookkeeping does not
open symbolic-link targets as data files, so links to FIFOs do not wait for a target
writer. Local regressions cover
writer-free FIFO-link commit, target preservation, dangling-link identity, and
preserving a replacement link during rollback. General filesystem calls can
still block; this fix does not eliminate ancestor-path replacement races.

Created-entry bookkeeping rejects observed types that do not match the entry
being created. A failed identity lookup or type mismatch grants no rollback
authority and triggers no speculative unlink or chmod. Fault-injected tests
preserve files, directories, and links replaced during an inspection failure;
type-mismatch tests preserve both the entry and any symlink target. Cleanup
remains best effort: an entry whose identity could not be recorded may remain
on disk, and same-type substitutions before identity capture are still a gap.

## Consent and lifecycle

Terminal output is untrusted. An initial `send` command only opens a modal; it
does not create a destination or stage payload bytes. An initial `receive`
command only establishes a 60-second bounded path-list collection; no file is
opened until the complete list is visible and approved. Data received before
approval retires the applicable pending session. A stale modal choice cannot
approve a new request. Replacing/dismissing the modal, closing the originating
pane or tab, canceling, timing out, closing the window, or shutting down the app
drops the transaction, closes approved read descriptors, and removes staged
download files.

After approval, filesystem work runs on a bounded worker rather than the
render/input path. A persistent toast reports only entry and byte counts; it
does not expose paths. Replies use the captured origin even if focus changes.
A vanished origin cancels instead of falling back to the focused terminal.

Session keys retain the stable tab-local pane identity, not a tab-versus-pane
event route. Path collection, consent, command lookup, cancellation, and worker
replies follow the same surviving owner through split/collapse (including a
nonzero surviving pane). Removing that owner never transfers its approval or
host-file bytes to a replacement single pane. Closed/closing owners are rejected
before reply delivery; stale upload completions receive a failed delivery
acknowledgement and their active session is canceled. Protocol replies still
bypass editable-input and broadcast routes, including in a read-only pane.

Attached live domains relay the raw OSC to the GUI, which owns consent and the
filesystem transaction. A detached domain immediately returns `EPERM`; its
daemon never writes desktop-host files or preserves a request for later.

The existing `[security] allow_kitty_file_transfers` option is unrelated. It
authorizes narrowly constrained ordinary-file reads for Kitty graphics `t=f`
in local terminals. It never bypasses OSC 5113 consent.

## Explicitly unsupported

- `receive` directory recursion and symbolic/hard-link export. The receive
  profile is regular-file-only.
- Quiet levels 1 and 2. Termeric requires acknowledgement flow so bounded queue
  saturation and filesystem errors cannot become silent or unbounded.
- Password/public-key confirmation bypass.
- Rsync/delta mode, replacement, resume, and partial retention.
- Detached-domain, plugin, serial, macOS, and Windows host writes.

Unsupported sessions receive a bounded `EINVAL` or `EPERM` response when a
valid session ID is available. Malformed and oversized input is discarded
without filesystem effects; parser errors with a recoverable ID receive
`EINVAL`. Payload and bypass bytes are never included in debug formatting or
normal logs.

## Evidence

Automated coverage is documented in [`testing.md`](testing.md). The deferred
real-client procedure and evidence fields are in
[`operator-acceptance.md`](operator-acceptance.md#5a-kitty-terminal-file-transfer-client).
Until both direction rows pass on a candidate build, do not advertise complete
bidirectional Kitty transfer or mark the feature Supported.

Owner-routing regressions use raw local PTY capture plus synthetic worker
completions: both directions, S7/S8 framing, pending path collection, split and
collapse, a nonzero survivor, removed/replaced/closed/closing owners, sibling
session-ID isolation, pruning, and replayed completion. These tests do not start
a native window or approve real host-file workers, and are not native-client
acceptance. See [#120](https://github.com/Tjemmmic/termeric/issues/120).
