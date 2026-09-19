# Persistent local terminal domains

Live domains are an experimental multiplexing service whose daemon always
listens on a same-user local Unix socket. Unlike a saved workspace, a live
domain owns real PTYs in a detached Termeric daemon. An optional OpenSSH stdio
bridge reaches that same private socket on a remote host without adding a TCP
listener or exporting the daemon token.
Closing every GUI disconnects relay clients but does not terminate the domain's
processes. A later GUI reconstructs the daemon's tab order, nested pane layout,
focus, bounded terminal state, and live output streams.

```bash
# Create a domain with one default shell; the response prints its generated ID.
termeric domain create "Daily work"

# Scriptable JSON, exact argv, cwd, title, and initial cell geometry.
termeric domain create build --json --cwd ~/src --size 120x40 \
  -- /bin/sh -lc 'exec cargo watch -x check'

termeric domain list --json
termeric domain protocol-version --json
termeric domain show DOMAIN --json
termeric domain workspaces DOMAIN --json
termeric domain panes DOMAIN --json

# Add a tab and split stable pane 0. Options must precede `-- COMMAND...`.
termeric domain spawn DOMAIN --title logs -- tail -f app.log
termeric domain split DOMAIN 0 vertical --cwd ~/src
# `show --json` exposes stable split IDs and ratios in thousandths.
termeric domain resize-split DOMAIN 0 350
termeric domain swap-pane DOMAIN 0
termeric domain balance-tab DOMAIN 0
termeric domain layout-tab DOMAIN 0 grid
termeric domain layout-tab DOMAIN 0 tall
# Move pane 1 to stable tab 2 without replacing its process or relay streams.
termeric domain move-pane DOMAIN 1 2
termeric domain rename-tab DOMAIN 0 "Build λ"
# Positions are zero-based; movement remains within the current pin group.
termeric domain move-tab DOMAIN 0 1
termeric domain pin-tab DOMAIN 0
termeric domain unpin-tab DOMAIN 0
# An explicitly empty title clears the override and reveals runtime title state.
termeric domain rename-tab DOMAIN 0 ''
termeric domain focus DOMAIN 1
termeric domain send DOMAIN 1 'printf "ready\\n"' --enter
termeric domain get-text DOMAIN 1 --bytes 65536
termeric domain close-pane DOMAIN 1

# A sole pane is closed with its containing tab; a sole tab requires kill.
termeric domain spawn DOMAIN --title temporary
termeric domain close-tab DOMAIN 1

# Attach a native window, detach by closing it, and later reattach.
termeric domain attach DOMAIN

# Explicitly terminate processes, then stop an otherwise empty daemon.
termeric domain kill DOMAIN
termeric domain stop-server
```

Domain IDs and pane IDs come from `create`, `show`, or the list commands. Human
output is tab-separated and intended for quick inspection; automation should
use `--json`. Text retrieval returns the newest complete UTF-8 suffix up to 1
MiB and reports truncation in JSON. `send` accepts at most 64 KiB per request.
Accepted input enters a four-frame, 256 KiB maximum pane-local queue; a full
queue returns an explicit backpressure error rather than waiting behind a child
that has stopped reading.

## Ownership and reconnect contract

The daemon is started on demand as a detached process. It exclusively owns PTY
masters, terminal parser state, scrollback, process lifetime, tab/pane topology,
and persisted focus. Each GUI pane runs a small raw-mode relay command. The
relay authenticates, receives a bounded ANSI reconstruction snapshot, streams
new output, forwards exact input, and publishes cell-size changes. Destroying a
relay, a window, or the entire GUI process only removes that attachment.
When multiple GUIs attach to one pane, the most recent valid resize is the
daemon's canonical PTY size; output and focus remain shared rather than cloned.

GUI focus and close actions are queued to the daemon without blocking window
input. A successful close response is routed to the originating native window
by stable domain, tab, and pane identity; duplicate clicks coalesce, failures
leave the relay tree intact, and every attached GUI also converges when its
affected relay stream closes. Split dissolution remaps local focus back to the
surviving daemon pane instead of retaining a stale identifier. `close-pane`
terminates and removes a split pane, collapses exactly its parent branch,
repairs focus, and closes attached relay streams with an explanation. It
refuses the sole pane in a tab and points to `close-tab`. `close-tab` terminates
every child in that tab, repairs the active tab, and refuses the domain's sole
tab so only explicit `kill` can remove the final live workspace.

New Tab, Split, divider-drag, pane swap, balance, layout presets, cross-tab pane
movement, Rename, Pin/Unpin, shortcut reorder, and direct tab-drag actions in an
attached GUI use the same nonblocking daemon control API. Only one mutation per
domain is sent at a time from a window, preserving the user's action order.
Every attached window keeps a separate authenticated topology subscription:
its atomic initial snapshot closes the describe/subscribe race, and monotonic
whole-domain revisions add, remove, split, collapse, rearrange, title, pin,
order, and focus routes without replacing relay PTYs whose stable identities
still exist.

Client topology reads are cancellable after authentication: the initial
subscription response has a ten-second frame budget, and each later frame has
one ten-second header/body budget starting when input becomes readable. Partial
progress cannot renew it. Waiting between revisions has no inactivity timeout.
Cancellation is checked between reads and on poll intervals capped at 500 ms;
partially read or just-completed frames are discarded when cancellation is
observed, and that stream is closed rather than reused. Callback refusal keeps
the explicit detach path. These read guarantees do not extend to connection
setup, SSH authentication, or request writes. Socket-only regressions exercise
partial initial/update headers and bodies, trickled updates, idle survival,
ordered revisions/closure, pre-cancellation, and callback-driven detach. Native
remote-host acceptance remains deferred.

Each nested divider has its own stable daemon split ID, so a drag cannot become
ambiguous when several ancestors share an orientation. The GUI previews the
exact selected divider, commits its final 100-900 thousandths ratio on release,
and rolls back to the latest authoritative snapshot if the daemon rejects a
stale target.
Divider previews also recheck the one-pending-request rule after button-down.
A newly pending same-domain request finishes the drag through the existing
rejection/rollback path before another resize, without removing the earlier
request or reviving the drag when it clears. Other-domain requests do not block
it. Synthetic topology and local relay fixtures cover refresh/mutation entry
points, cached-ratio restoration, stable pane routes, and one-time cleanup;
native live-domain drag acceptance remains separate.
Explicit tab titles outrank focused-pane OSC titles in the daemon. Clearing an
override reveals the newest title already parsed from that pane. A pinned tab
moves to the end of the domain's pinned prefix; unpinning moves it to the start
of the unpinned group. Stable-ID movement is zero-based and rejected when its
target crosses that boundary. Relay-side OSC title events are not applied as a
second local authority.
Direct pointer and timed overflow tab previews share source/destination checks,
including exact captured domain/remote-tab identity and a fresh pending-request
check before each move. A request arriving after button-down blocks further
previews without canceling the earlier request. Closed/closing destinations are
refused, and finalization cannot send a move for a source whose captured remote
identity no longer matches. Local regression fixtures cover these races and
the unaffected pinned-group/unrelated-domain paths; this is not native
live-domain drag acceptance.
Swap exchanges the focused pane with the next authoritative leaf while keeping
split identities and ratios. Balance retains the tree and resets every divider
to 50%. Columns, rows, and grid rebuild the same ordered pane identities and
allocate fresh stable split IDs. Focus and relay PTYs survive every operation.
Zoom remains intentionally window-local view state and does not mutate the
daemon topology. Cross-tab movement is one atomic daemon transaction: it
rejects the source's last pane and a same/missing destination, preserves the
global pane ID and all relay attachments, collapses the source branch, creates
a fresh vertical split beside the destination focus, then focuses and activates
the moved pane. Attached GUIs transfer their existing local relay owner before
applying the authoritative source and destination trees.
Stale/replayed revisions are ignored, a lagging subscriber is evicted and
reconnects to a fresh snapshot, transient disconnects retry with bounded
backoff, and closing a window cancels its reader. The GUI names the affected
domain and local/SSH authority once per outage. It reports recovery only after
a fresh, route-matched authoritative snapshot reconciles successfully; an
unchanged equal-revision snapshot is still valid reconnect evidence, while a
mismatched route or failed reconciliation leaves the outage active. A
definitive domain-close frame ends the outage without claiming reconnection.
Saved workspace archives and live domains intentionally remain separate
concepts and storage formats.

The daemon is not durable across its own crash or host reboot. A daemon crash
closes PTY masters and the operating system terminates or hangs up their child
sessions; reconnect returns an explicit connection/domain-not-found error.
Upgrade requires terminating domains and stopping the old daemon. Protocol
version mismatch fails before any command or input is accepted. These choices
avoid pretending that snapshot files can recover live process state.
An exited leaf is removed from a multi-pane layout, and an exited sole pane is
removed with its tab when another tab survives. The sole pane of the sole tab
remains as a bounded, inspectable history record until the user kills its
domain; a later relay receives its snapshot followed by an explicit closed
frame. Exited panes never silently respawn, and the server reaps child status.

Terminal-generated PTY replies are produced only by the daemon parser; GUI
replicas suppress duplicates. Bells, notifications, clipboard operations, and
other desktop-only effects are presented by an attached GUI when observed live
and are not replayed after a detached interval. No headless daemon is granted a
desktop clipboard or notification capability.

Reconnect snapshots preserve text, supported ANSI styling, active screen,
cursor position, input/mouse modes, bounded scrollback, and the complete
already-decoded graphics namespace. Protocol-v8 snapshots retain root and
animation-frame RGBA pixels, cache order and revisions, image-number aliases,
anonymous Sixel/iTerm2 ownership, normal and alternate placements, Unicode
virtual prototypes, relative-parent identity, and animation playback clocks.
Incomplete in-flight graphics uploads are not decoded state and are not
transferred; later complete graphics commands still flow live in order.

After the initial `Attached` reply, client snapshot replay allows ten seconds
per complete frame (header and payload together) and five minutes for the whole
replay, including text output and the final flush. Complete fragments do not
renew the overall budget. Expiry fails the attachment before relay workers
start and closes the owned transport; a partly consumed stream is not retried.
Local socket regressions cover trickled headers/payloads, progressing fragments,
late final flushes, rejected frames, and the exact handoff to live output.
These are read deadlines, not cancellation of a blocked local stdout write:
an expired output write/flush is detected only once it returns. Connection and
request setup are separate phases; native SSH acceptance remains deferred.

Pixel buffers are reference-counted at the atomic attachment point and encoded
outside the daemon state lock. The server sends small `SnapshotOutput` frames
before `SnapshotComplete`; live PTY output after the captured sequence waits in
the existing bounded attachment queue. The GUI accepts the internal restore
APC only in a newly created authenticated relay pane, validates a 4 MiB-capped
metadata manifest and the ordinary image/frame limits, commits placement and
identity state only after all decoded pixels match, and consumes the import
permission on success or failure. Ordinary panes ignore that APC. Restore
commands use quiet graphics loads and therefore cannot manufacture replies to
the daemon-owned application PTY.

Import begin and failure cleanup preserve the receiving host's cell-pixel
geometry; replacing decoded graphics must not reset font/DPI metrics to the
bootstrap defaults. At metadata commit, pixel-sized placement footprints are
recomputed for the current receiving cells, including offsets and both screen
buffers. Exact image pixels, source crops, explicit cell-sized destinations,
and relative-parent identities remain unchanged. This also handles a host
font/scale update while fragmented metadata is in flight. Core regressions
cover success/failure cleanup, different sender/receiver metrics, mid-import
metric changes, subsequent image commands, and immediate regional cropping
without a second host resize. The opt-in GPU lane repeats all partial-cell
image edit cases after fragmented relay import; native remote-host acceptance
remains separate and deferred.

## Protocol and security boundary

The current client wire version is available offline through `termeric domain
protocol-version [--json]`; this command does not discover or start a daemon.
The local and OpenSSH transports use the same length-prefixed, tagged JSON
frames. Version 2 added
daemon-owned pane/tab closure; version 3 adds topology subscription snapshots,
monotonic change frames, and explicit domain-close signaling; version 4 adds
stable split-node identities and validated divider resizing; version 5 adds
authoritative tab title overrides, pinned-prefix state, and stable-ID reorder;
version 6 adds stable-pane swap, recursive balance, and columns/rows/grid layout
reconstruction with fresh stable split identities; version 7 adds atomic
stable-pane movement between tabs without replacing daemon PTYs or GUI relays;
version 8 adds ordered, bounded decoded-graphics snapshot frames and an
explicit snapshot-complete boundary; version 9 carries rows, columns, and both
physical-pixel dimensions on attachment resize frames; version 10 lets pane
movement name an exact destination anchor and split direction while retaining
the focused-pane/vertical default for existing domain CLI commands. Versions
11 through 15 add the bounded upload transaction used by local and OpenSSH
file/directory transfers: safe typed descriptors, contiguous chunks, explicit
cancel/commit, resumable offsets, prefix digests, retained-partial inventory,
and idempotent completion replay. Version 16 adds focused-master Tall and Fat
layout preset values; both retain pane identities and allocate fresh stable
split identities.

Renderer-backed relays preserve the exact pane grid geometry at the daemon
PTY. An external parent that reports zero pixel fields retains the daemon's
last known physical cell extent rather than erasing valid geometry.
Negotiation requires exact equality and fails before an unknown request or any
input is sent. On a mismatch, run the offline probe in each installation to
identify which executable must be upgraded; remote users can invoke the same
command through an ordinary trusted SSH session because the Termeric bridge
itself intentionally refuses incompatible versions. Encoded frames are capped
at 4 MiB and decoded streaming chunks at 64 KiB. Every connection
must first negotiate the exact protocol version and present a 256-bit random
token. On Linux the
server also checks `SO_PEERCRED` and rejects a different effective UID.

Runtime discovery prefers a user-owned `XDG_RUNTIME_DIR` and otherwise uses
`/tmp/termeric-UID`. The Termeric subdirectory is forced to mode `0700`; the
socket and token are mode `0600`. Symlinked, foreign-owned, public-token, and
non-socket replacement paths fail closed. Runtime directories and token files
are opened without following their final path component and are validated from
the opened descriptor, so a metadata/open substitution cannot redirect a
permission change or credential read. Token reads accept exactly 64 ASCII-hex
bytes into a fixed buffer before authentication; sparse or oversized files
cannot cause attacker-sized allocation. Temporary publication names use an
independent random nonce and do not disclose a token prefix. The daemon accepts
at most 64 clients, 16 domains, 16 tabs per domain, 64 panes per domain, 16
panes across the daemon, 8 attachments per pane, and 8 topology subscribers per
domain. Parser scrollback is capped at 100,000 lines and 8 MiB per pane.
Existing core Kitty-image limits also apply independently.

Connection setup gives the complete authentication frame and the complete
first request separate ten-second read deadlines. Each deadline spans both
the length header and JSON body; trickling partial bytes does not renew it.
Authenticated clients that never submit a request are disconnected and release
their client slot. The setup reader restores the preceding socket timeout on
success and failure, and completed setup clears the read timeout before entering
a long-lived attachment/subscription (uploads apply their own frame policy).
Socket-only fixtures check slot release, partial-header/body deadlines, timeout
restoration, and an established idle subscription surviving its setup budget.
These are setup I/O limits, not a total request-execution or transfer deadline.

Upload chunks, cancellation, and the final commit each have a separate
thirty-second complete-frame read deadline, replacing the former per-read idle
timeout that partial bytes could renew indefinitely. This includes the wait
for the frame's first byte, length header, and JSON body. Each completed frame
starts a fresh budget; the overall transfer is not limited to thirty seconds.
A timed-out partial frame is never applied. The active upload slot is released,
only fully accepted chunks are retained under the existing bounded resume/TTL
policy, and no destination is published without a complete valid commit.
Local socket/temporary-file fixtures exercise timeout during both a chunk and
the final commit, inspect retained bytes and inactive-slot accounting, then
resume on a fresh connection using the same validated offset/prefix hash and
verify exact published contents. Slow-link/remote-host acceptance is separate.

PTY reader threads never write to client sockets. They publish into bounded
64-frame queues; a full/disconnected subscriber is evicted. PTY attachment
eviction now shuts down both directions of its socket immediately, using one
extra cloned descriptor per active attachment (within the existing attachment
limits). This interrupts pending socket work instead of draining obsolete
queued output first. Explicit detach also shuts down that exact connection.
Input and resize dispatch revalidate the exact attachment ID and open-pane
state under the eviction/mutation lock, so a frame already read before eviction
cannot mutate the surviving pane or acquire a replacement attachment's authority.
Normal closure still queues `Closed` after preceding output; if that frame
cannot fit, the connection is shut down without claiming successful delivery.
Local fixtures cover queue-full/disconnected-receiver shutdown, stale/replaced
reader rejection, closed-pane refusal, and live replacement input/resize.

On the client, every started output-worker exit clears relay liveness and closes
the shared transport, including server/unexpected-frame errors, output write or
flush failures, and unwinding. Cleanup never takes the input/resize writer mutex:
socket shutdown or the existing owned-child cleanup must first wake a writer
blocked on that connection. Normal EOF and `Closed` retain their output behavior,
and repeated main-thread cleanup is idempotent. Local fixtures cover these exits,
an actually blocked socket writer, and reaping an inert transport stand-in child.
The output worker distinguishes EOF before a new frame from a truncated header
or payload. Invalid lengths, malformed JSON, and transport read failures now
return an error instead of silently reporting a successful disconnect. A local
owner-initiated shutdown can interrupt a partial read without manufacturing a
peer failure; decoder errors are not suppressed by that exception. Tests cover
every truncated byte boundary of a representative output frame, invalid lengths
and JSON, connection reset, fragmented/interrupted reads, and local cancellation.
Production live-output reads now wait indefinitely between frames, but once
readiness starts a frame, a single ten-second deadline covers its header and
payload. Trickled bytes do not renew that deadline. Polls check cancellation at
most every 500 ms, including partial-frame and idle waits; a completed frame is
rechecked before delivery. Expiry reports a read error and closes the shared
transport. Socket-only regressions cover stalled and trickled headers/payloads,
idle waits longer than the test frame budget, ordered output, and cancellation
after observed partial consumption. This does not change the wire format or
bound local stdout writes, connection setup, or the entire relay lifetime.
A scope-owned worker set also closes first and joins every successfully started
thread when input/poll processing returns an error, a later thread cannot start,
or the relay unwinds. Normal input EOF uses this same close-before-join path;
it does not wait for a writer mutex to append a `Detach` frame. The daemon's
existing EOF handler retires only the exact attachment. Both worker handles
are joined before either worker error is returned, repeated cleanup is safe,
and input write failures cannot silently become success. A server/output error
takes precedence over resize/input errors. The resize worker also owns completion
cleanup on failure or unwinding and returns write/flush/poison errors to the
joining caller. It records geometry only after a successful frame write, never
retries a partial frame, and closes a failed transport before releasing its
writer lock. Both input and resize recheck liveness after acquiring that lock.
They now use one guarded frame-send transaction: input failure, a writer panic,
or an already poisoned writer also clears liveness and closes the transport
before unlocking. No producer retries or appends to an incomplete frame after
that failure. Successful frames preserve the wire bytes and keep the transport
open. A deterministic local fixture pauses transport cleanup and verifies the
writer remains locked on both error and unwind; it starts no SSH process.
Local shutdown interrupting a resize write is normal cleanup, not a new failure.
Cell and pixel geometry deduplication and the 250-ms polling interval are
unchanged. Attached input/resize writes now share one ten-second budget from
the first header write through payload and flush; partial progress does not
renew it. Successful flush resets the budget for the next frame, and idle time
between writes does not count. Socket sends use per-call nonblocking flags,
preserving the concurrent reader's descriptor flags. The separately owned SSH
stdin pipe is made nonblocking without changing its stdout reader. Expiry uses
the same close-before-unlock failure path. Socket and anonymous-pipe fixtures
cover stalled/slow readers, header-to-flush expiry, fresh budgets, unchanged wire
bytes, and reader flags without invoking SSH. This does not bound mutex waits,
serialization, connection setup, local stdout, or the separate forwarding
bridge. The separate SSH bridge now scopes and joins its input worker, closes
both directions on failure/unwind, and half-closes only input on normal EOF to
drain final daemon responses. Negotiation and forwarding share unbuffered stdin
ownership; local socket fixtures cover pipelined requests and teardown without
running SSH. Its idle input cancellation uses 500-ms polls, but bridge forwarding
and blocked stdout still have no overall teardown deadline. Local fixtures inject
zero/partial/full-start exits, input failure and unwinding, worker failures,
blocked socket writes, and inert-child cleanup; they do not exhaust the OS
thread limit. This is not a global teardown deadline: connection setup and a
blocked local stdout sink retain their separate limitations. Pre-worker snapshot
reads have the per-frame and overall replay budgets described above.

These checks are not native remote-host acceptance. Topology subscriptions
also retain one shutdown descriptor within their eight-per-domain ceiling:
overflow, receiver loss, or explicit detach closes only that subscription's
transport, without waiting to drain an obsolete snapshot. A registration
guard releases the exact slot on initial-snapshot write, descriptor-clone,
or reader-spawn failure as well as normal completion. Healthy subscribers
still receive ordered revisions and `DomainClosed`; a full queue on domain
closure instead disconnects, without claiming the close frame was delivered.
Socket-only tests reproduce failed initial publication without a later topology
edit, check repeated slot reuse, exercise overflow/receiver-loss/full-close
shutdown, and join real subscription workers after exact detach and normal
domain closure while another subscriber remains live. Complete outbound frames
now have one elapsed-time budget spanning serialization, header, body, and flush:
ten seconds for negotiation replies and five seconds for established responses
and stream frames. Partial-write progress does not renew the budget. All daemon
send paths use the bounded writer, including errors, shutdown acknowledgement,
upload replies, attachment snapshots, graphics fragments, and topology frames.
Per-call nonblocking sends plus deadline-bounded readiness waits keep a single
large write from continuing internally beyond its budget. The writer leaves
the socket timeout and shared blocking mode unchanged, including on the
concurrent reader's cloned descriptor. A failed frame
shuts down both socket directions, so callers cannot append an error reply to
a partially transmitted frame. A response timeout does not roll back a domain
action or upload already committed; clients must inspect resulting state.
Separate frames get fresh budgets, so this is not a whole-session, snapshot-batch,
or transfer deadline. Socket-only tests reproduce timeout renewal under a slowly
draining peer, then verify bounded failure, connection shutdown, unchanged wire
bytes on success, and preservation of socket policy on success/failure.
Bounded queues and eviction keep slow clients from blocking PTY draining or
retaining unbounded output. A topology subscriber retains at most one pending
lightweight snapshot before eviction; revision frames omit command argv, cwd,
geometry, and attachment-count inspection metadata. Terminal snapshots are
capped at 512 KiB. PTY input and terminal-generated replies likewise use one
bounded writer worker per pane. Shared daemon state performs only nonblocking
queue admission, so a stalled child cannot hold the global state lock or starve
unrelated domains; overflowing attached relays are disconnected and can
reattach from a fresh snapshot.
Authentication, resource validation, and path checks happen before state
mutation.

## Failure and test coverage

The local control-surface fixture deliberately queues PTY echo after an
attachment's snapshot, observes that output in daemon state, and then closes
the pane without first draining the attachment. It requires contiguous output
sequence numbers, the complete marker before the exact control-close reason,
and socket EOF. Reads have timeouts and retained fixture output is capped at
4 KiB. This removes the former race in which valid output preceding `Closed`
was mistaken for a closure failure; it does not change runtime behavior or
the separate slow-subscriber eviction policy.

Deterministic tests cover wire round trips, invalid/truncated/oversized frames,
seeded byte mutation, unsafe paths, descriptor-verified directories, token
permissions and fixed-size reads, disclosure-free temporary names, split and
pruned topology,
bounded UTF-8/snapshot output, incompatible versions, the full control API,
last-pane/last-tab refusal, attached-client closure, nonblocking GUI completion,
stable-ID and error routing, topology revision ordering and slow-reader
eviction, incremental GUI split/tab materialization, stale-revision rejection,
exact nested-divider targeting, resize validation and convergence, rejected
preview rollback, title precedence and clearing, pinned-boundary validation,
stable-ID tab reorder without relay replacement, rejected tab-state rollback,
stable-pane swap/balance/preset convergence, fresh split-ID allocation,
focus/relay retention, single-pane layout-operation rejection, relay-title
authority, atomic cross-tab pane movement, source/destination focus repair,
relay handoff without respawn, last-pane/same-tab/missing-tab rejection,
relay-driven split dissolution, focus-map repair,
ownerless-tab removal,
multiple reconnecting clients, GUI-style detach, PTY survival, and cleanup.
The socket integration fixture skips only when the host sandbox denies AF_UNIX.
The native X11 lane creates a domain, attaches a GUI, produces output, closes
the last window, verifies the domain remains live, reattaches a new GUI, and
continues the same shell.

## Remote phase and threat model

Experimental remote CLI control and GUI attachment use a threat-reviewed
OpenSSH stdio bridge.
The daemon still exposes no TCP/TLS listener: `ssh` authenticates and encrypts
the connection, while a remote same-user helper authenticates to the private
Unix socket without exporting its token. Strict known-host verification,
non-interactive key/agent authentication, bounded partial-frame deadlines, and
disabled forwarding/TTY/local-command behavior fail closed. GUI attachments
retain the SSH authority for topology, controls, focus, reconnects, and every
pane relay, preventing fallback to a same-named local domain. Bounded saved
profiles give existing OpenSSH destinations friendly names and are resolved
once before attachment. The command palette provides asynchronous local/saved-
host domain discovery and opens a selected domain in a separate native window
after validating its authenticated prepared snapshot. One explicitly
confirmed, bounded regular-file or deterministic-directory upload can target an
exact local or SSH domain pane cwd without shell interpolation or overwrite;
the CLI equivalent is `domain upload`. GUI transfers expose in-flight progress
and exact pre-commit cancellation. CLI transfers can opt into a 24-hour partial
record, list it, and resume or cancel it from another authenticated process
while the same daemon is alive; retained prefixes are SHA-256 verified before a
suffix is sent. Daemon-restart persistence and rich metadata preservation
remain unavailable. Audit/revocation beyond OpenSSH,
remote clipboard, broader filesystem workflows, notification, and native TLS
features remain unavailable. The complete trust chain, options, exclusions, and
hermetic evidence are in
[`remote-domains.md`](remote-domains.md).
