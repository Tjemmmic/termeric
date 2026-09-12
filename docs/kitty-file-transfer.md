# Kitty terminal file transfer

Termeric implements a bounded Linux subset of Kitty's
[file-transfer protocol](https://sw.kovidgoyal.net/kitty/file-transfer-protocol/)
over OSC 5113. The supported direction is Kitty `send`: a child process or
remote shell sends files onto the computer running the Termeric window. This
backs the common `kitten transfer remote-file /local/path` workflow.

The support status is **Partial** until a current native `kitten transfer` run
has accepted evidence. The parser, consent boundary, filesystem transaction,
and failure paths have hermetic coverage; native client acceptance is deferred.

## Supported profile

- One explicit approval per transfer session, scoped to the exact originating
  tab or pane.
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
does not create a destination or stage payload bytes. Data received before
approval retires the pending session. A stale modal choice cannot approve a
new request. Replacing/dismissing the modal, closing the originating pane or
tab, canceling, timing out, closing the window, or shutting down the app drops
the transaction and its staged files.

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

- `receive` / `kitten transfer --direction=upload`, because it lets terminal
  output request reads from the host.
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
Until that row passes on a candidate build, do not advertise bidirectional
Kitty transfer or mark this direction Supported.
