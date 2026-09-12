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
- Simple transmission with no compression or zlib compression.
- `send`, `file`, `data`, `end_data`, `cancel`, and `finish`, with canonical
  status/progress replies in the requesting terminal's S7C1T/S8C1T mode.
- At most 4 active transfers, 1,024 entries per transfer, 4,096 decoded bytes
  per data command, 64 MiB of declared/uncompressed payload per transfer, a
  1,024-command worker queue, and 10 minutes of inactivity.
- File contents staged outside the visible destination, individual regular
  files published without overwrite, and best-effort transaction rollback if
  a later entry fails during `finish`.
- Mode bits limited to `0777`; setuid, setgid, and sticky bits are stripped.
  Directories retain owner access while assembling and receive final mode and
  modification time after their contents. Non-negative modification times are
  applied where the platform supports them.

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
publication uses no-clobber rename semantics.

Hard links may name only a regular file ID in the same transfer. Symbolic links
support Kitty's `fid:`, `fid_abs:`, and `path:` forms. `fid` targets must name an
entry in the same transfer. A `path:` target may intentionally point outside
the transfer so ordinary relative, absolute, and broken symlinks can be
preserved; the approval dialog calls out that the remote side controls symbolic
link targets. The link destination itself remains subject to every path and
no-overwrite check.

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
