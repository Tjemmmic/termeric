# Remote domain transport and threat model

Termeric has an experimental OpenSSH transport for controlling and attaching
to a live-domain daemon on another Unix host. It does not open a Termeric TCP port.
Instead, each command starts one non-interactive SSH process and runs
`termeric domain proxy-stdio` on the authenticated host. That helper connects
to the remote user's private Unix-domain socket and forwards the existing
bounded protocol over SSH stdin/stdout.

```bash
# The destination is any already configured OpenSSH host or user@host.
termeric domain --host developer@build.example list --json
termeric domain --host developer@build.example create build --json \
  -- /bin/sh -lc 'exec cargo watch -x check'
termeric domain --host developer@build.example show DOMAIN --json
termeric domain --host developer@build.example send DOMAIN PANE 'cargo test' --enter
termeric domain --host developer@build.example get-text DOMAIN PANE --json
termeric domain --host developer@build.example attach DOMAIN
```

## Saved host profiles

Frequently used destinations can have stable, friendly names in
`config.toml`:

```toml
[domains.ssh_hosts.build]
destination = "developer@build.example"
description = "Build farm"

[domains.ssh_hosts.production]
destination = "production-alias"
```

```bash
termeric domain hosts
termeric domain hosts --json
termeric domain --host-profile build list --json
termeric domain --host-profile build attach DOMAIN
```

At most 64 profiles are accepted. Names contain only ASCII letters, digits,
periods, underscores, or hyphens and are capped at 64 bytes. Destinations use
the same fail-closed validation as `--host`; descriptions are optional bounded
single-line text. Invalid configuration and unknown profiles fail before
launching `ssh`.

Profiles do not store passwords, keys, ports, proxy commands, or host-key
exceptions. The destination should normally name a `Host` entry in
`~/.ssh/config`, where OpenSSH remains authoritative for those settings. A
profile is resolved once when a command or GUI is launched. An attached GUI
pins the resolved destination into topology, control, focus, reconnect, and
relay routes, so editing or renaming the profile cannot redirect the live
window.

The command palette's **Attach Live Domain...** action provides the same flow
without leaving the GUI. It first lists this computer and every saved SSH
profile, then discovers that authority's live domains and opens the selection
in a separate native window. Host/domain rows support fuzzy keyboard filtering,
arrow/Enter navigation, mouse selection, wheel scrolling, and Escape to return
one level or close.

Discovery and the authoritative pre-attach snapshot run on bounded background
workers so SSH setup cannot freeze the source window. Results carry a picker
generation; responses from a cancelled or superseded selection are ignored.
The prepared snapshot must match the selected stable domain ID before a window
is queued. Window initialization consumes that authenticated snapshot, while
subsequent topology and relay connections retain the same resolved authority.

The remote host must have a compatible `termeric` executable on `PATH`. Its
host key must already be present in the user's OpenSSH known-hosts policy, and
key or agent authentication must work non-interactively. Termeric deliberately
does not ask for passwords or accept a new host key inside a terminal-control
request.

Protocol negotiation requires an exact wire-version match. The offline
`termeric domain protocol-version [--json]` probe reports the version expected
by an installation without reading credentials, creating a socket, or starting
a daemon. If the OpenSSH bridge reports a mismatch, run the probe locally and
through a separately trusted SSH command on the remote host, then upgrade the
older installation. Nesting `protocol-version` under Termeric's `--host` form
is rejected because that form cannot cross an incompatible bridge.

## Trust and authentication

The trust chain is explicit:

1. The local user trusts the `ssh` executable resolved from their process
   environment and their OpenSSH configuration.
2. OpenSSH authenticates the server using the existing known-hosts policy and
   authenticates the user with a key, certificate, or agent.
3. The remote SSH account selects and trusts the `termeric` executable on its
   `PATH`.
4. The stdio helper connects only to that account's mode-`0600` daemon socket.
   The daemon independently verifies same-user Unix peer credentials and its
   mode-`0600` 256-bit token.

The daemon token never crosses SSH. The outer stdio negotiation requires an
empty token and rejects any nonempty value. Only the remote helper reads the
remote runtime token, uses it on the remote Unix socket, and then forwards
frames after both sides have completed exact protocol-version negotiation.
Stdout is protocol-only; diagnostics use stderr.

Termeric invokes OpenSSH without a shell and places `--` before the validated
destination. It also forces these fail-closed options:

- `BatchMode=yes` and `StrictHostKeyChecking=yes`;
- a ten-second connection timeout;
- bounded server-alive detection;
- no requested TTY, forwarding, local command, or escape character.

The destination is capped at 512 bytes and cannot be an option, URI, contain
whitespace, or contain control characters. OpenSSH configuration can still
provide aliases, ports, identities, proxies, certificates, and policy.

## Framing, flow control, and lifecycle

Remote transport retains the same 4 MiB encoded-frame cap, 64 KiB stream-chunk
cap, resource limits, and exact version requirement as local domains. Initial
negotiation and one-shot responses have a ten-second, partial-frame-aware
deadline. A peer cannot send only a length prefix and hold a client forever.

The bridge uses fixed-size 16 KiB forwarding buffers and kernel pipes; it does not
accumulate an unbounded user-space queue. If a client stops reading, the remote
daemon's existing five-second socket-write deadline disconnects that client
without blocking PTY draining. Topology/output queues remain bounded at the
daemon. Killing SSH, losing the network, or closing the local command removes
only that attachment; remote PTYs and domain state remain owned by the daemon.
Its input worker is scoped and joined before the bridge returns, including
output errors, final-flush failures, and unwinding. Input read/write failures or
panics close the shared socket and wake output forwarding instead of silently
leaving it blocked. Output-side completion cancels idle input on at most 500-ms
poll intervals and closes the socket before joining, also waking socket writes.
Normal input EOF instead half-closes the socket's write side so final daemon
responses can drain. Negotiation and forwarding share one unbuffered owned
stdin descriptor, preventing prefetched request bytes from being stranded
behind descriptor polling. Local fixtures cover coalesced negotiation/request
bytes, half-close response draining, error/panic cleanup, and joined input
ownership. They invoke no SSH process and do not establish native acceptance;
blocked stdout and bridge forwarding deadlines remain separate limitations.
The next command creates a fresh SSH channel and can reconnect. An attached GUI
retries its topology subscription with bounded exponential backoff, identifies
the exact domain and pinned SSH destination in one warning per outage, and
announces recovery only after a route-matched authoritative snapshot has been
reconciled. An unchanged equal-revision snapshot confirms the new channel;
mismatched frames and reconciliation failures cannot clear the disconnected
state.

Once authenticated, topology frame reads use the same cancellable deadline
reader for local sockets and SSH stdout. Initial replies and started updates
have a ten-second complete-frame budget; idle revision waits remain unlimited.
Partial reads check cancellation on at most 500-ms poll intervals and close the
channel instead of delivering stale work after cancellation is observed.
Local socket fixtures cover the subscription lifecycle; they do not constitute
real-SSH acceptance or change SSH connection/authentication policy.

An attached relay's started output worker owns completion cleanup: any exit
clears its shared liveness flag and closes the connection without acquiring the
input/resize writer mutex. For SSH this uses the existing child owner, which
reaps an exited process or terminates and reaps its still-running child. Local
socket and inert-child tests cover output errors, sink failures, unwinding,
normal completion, and blocked-writer release. No real SSH process or remote
host is needed for those tests. Live output now reports malformed/truncated
frames and transport read errors instead of treating all read failures as clean
EOF. EOF before a new frame remains normal; read interruption caused by local
owner shutdown is also normal, while decode errors remain errors. Local fixtures
cover these distinctions without claiming SSH acceptance. Live-output reads
allow unlimited idle time between frames, then apply a ten-second deadline to
each started header/payload without renewing it as bytes arrive. Cancellation
checks occur at most every 500 ms of polling, including partial reads. Local
socket tests cover stalls, trickles, idle delivery, and cancellation; these do
not establish native network acceptance or cancellation of blocked stdout.
A scope-owned worker set closes and joins every
thread that started on input/poll errors, later spawn failure, normal input EOF,
and unwinding, without waiting to append `Detach` to a blocked writer. Early-exit
and partial-start fixtures cover that ownership boundary. These changes do not
add connection deadlines or make a blocked local stdout sink cancellable.
Resize workers now share completion cleanup and return write/flush/poison errors
through the joined worker result instead of silently dropping them. A failed
resize closes the transport before unlocking the writer, and producers recheck
liveness after acquiring that lock. Successful geometry is deduplicated only
after the frame is written. Intentional shutdown remains normal; the existing
250-ms poll is unchanged. Attached frame writes now have one ten-second budget
from header through flush; progress cannot renew it, and each successful flush
resets it for the next frame. Per-call nonblocking socket sends preserve reader
flags; only the separately owned SSH stdin pipe becomes nonblocking. Local
socket/anonymous-pipe tests cover stalls, slow readers, full-frame expiry, and
successful wire bytes without launching SSH. Mutex waits, serialization,
connection setup, stdout, and the separate forwarding bridge are not bounded
by this write budget. Hermetic tests cover
partial writes, flush failure, poison/unwind, and cell/pixel geometry deduplication.
Input and resize share a guarded frame-send transaction, so input errors and
panics likewise stop both producers and close the channel before unlocking the
writer. A paused-cleanup fixture verifies this lock ordering without an SSH
process; successful frame bytes and intentional shutdown behavior are unchanged.
Separately, attachment snapshot replay bounds each frame read to ten seconds
and the full replay to five minutes; complete fragments cannot renew that total.
Expiry closes the owned transport before workers start. Text writes and the
final flush count toward the replay budget, but synchronous blocked stdout is
only checked once it returns. Local socket tests cover these read limits and
successful handoff to live output, not real SSH/network acceptance.

The helper starts the remote user's daemon on demand using the same private
runtime-directory checks as a local invocation. One SSH process is used per
request, topology subscription, or pane relay. There is no shared
unauthenticated local listener, connection pool, or background credential
cache.

A GUI attachment records its authority separately from the daemon's domain ID.
Describe, topology reconnect, mutations, focus updates, and every pane relay all
reuse that authority. A local domain with the same textual ID therefore cannot
receive a remote window's actions. Focus coalescing is bounded independently by
authority and domain so a disconnected host cannot suppress another window's
focus update.

## Security consequences and exclusions

An authenticated remote account and its `termeric` executable can observe and
control every domain that account owns. A compromised remote host can return
malicious terminal bytes; normal terminal parser limits and desktop-effect
policies still apply, but SSH does not make the host trustworthy. Users should
not attach to hosts they would not trust with the displayed terminal content.

Kitty ordinary-file graphics are always denied in domain-owned PTYs and their
GUI relay replicas, even when `allow_kitty_file_transfers` is enabled for
ordinary local tabs. Protocol-v8 attachment serializes the daemon's bounded
decoded image cache and semantic placement/animation metadata, not the
originating path; treating a remote `t=f` path as a local path would expose
same-named desktop files. Ordinary-file graphics are therefore unsupported in
local and remote live domains; the SSH transport transfers decoded pixels for
an already accepted image and never transfers or reopens the file.

Kitty OSC 5113 has a different authority boundary. An attached live-domain
pane may request a remote-to-host `send` transfer, but the desktop app must show
request-scoped consent and all replies stay on that exact pane relay. The
daemon does not write host files. If no consenting GUI attachment exists, it
returns `EPERM`; it never queues a transfer for a future attachment. The
reverse `receive` direction, which would let terminal output request reads from
the host, is unsupported. See
[`kitty-file-transfer.md`](kitty-file-transfer.md).

The authenticated stdio protocol does provide one narrow transfer path: an
explicit CLI request or confirmed GUI drop can stream one regular local file or
bounded deterministic directory archive (64 MiB maximum) to the exact pane's
current directory. The daemon stages it privately, commits atomically without
overwrite, and cleans interrupted partial data. Ordinary retries use a
60-second retained offset. CLI users may explicitly retain a partial for up to
24 hours, list it through the same authenticated authority, and resume it from
another process only after the local prefix matches the daemon's SHA-256
digest. Retention does not survive daemon shutdown or restart. Local source
paths never cross the wire and no remote shell command is constructed. See
[`file-drag-and-drop.md`](file-drag-and-drop.md).

This phase does **not** provide interactive password entry, automatic host-key
enrollment, a native TCP/TLS listener, cross-host discovery, port forwarding,
daemon-restart-persistent or metadata-preserving transfer, remote clipboard,
notification forwarding, desktop URL opening, or audit/revocation beyond the
user's OpenSSH policy. The picker discovers
domains only after the user chooses one configured authority; it is not a LAN
or account discovery service. Native real-host acceptance remains a separate
manual step.

## Verification

Parser tests reject option-like, URI, whitespace/control, and nested transport
forms before process launch. They also prove that remote attachment retains its
SSH authority and that relay argv includes the same `--host` destination.
Configuration tests cover profile bounds, validation, round trips, inventory,
unknown-name failure, and one-time resolution. A real-binary fixture uses an
isolated config directory and fake `ssh` spy to prove the resolved destination
and strict options reach process argv while an unknown profile starts nothing.
Picker tests cover local/SSH authority selection, shared mouse hit geometry,
keyboard navigation, empty/error states, stale discovery suppression,
mismatched prepared-snapshot rejection, modal close cleanup, and exact
new-window target/snapshot handoff.
Authority tests keep identically named local and SSH domains disjoint. Deadline
tests hold a partial frame open and require bounded failure. The stdio helper
rejects daemon-token disclosure before opening a socket. A hermetic integration
fixture replaces `ssh` with a local command spy, creates/lists/kills a real
daemon domain through pipes, relays live output, streams a multi-frame binary
file into its exact cwd, rejects overwrite, checks every forced OpenSSH option,
and verifies no token appears in argv. A separate early-disconnect
fixture must fail promptly. No fixture contacts a network host or uses
credentials. An executable-level check also proves the protocol-version probe
is machine-readable and creates no daemon socket or token. Native-window
attachment to a real remote host remains a manual acceptance step. Headless GUI
tests cover authority-specific disconnect text, one-warning outage
deduplication, equal-revision recovery, route-mismatch rejection, and
domain-close cleanup without opening an SSH connection.

The local daemon contract, resource limits, and GUI behavior are in
[`live-domains.md`](live-domains.md).
