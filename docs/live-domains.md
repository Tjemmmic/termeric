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

PTY reader threads never write to client sockets. They publish into bounded
64-frame queues; a full/disconnected subscriber is evicted. Socket writes time
out after five seconds, so slow clients cannot block PTY draining or cause an
unbounded output queue. A topology subscriber retains at most one pending
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
