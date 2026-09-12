# Running-GUI control protocol

Termeric exposes experimental local inventory and narrowly typed control of
already-running GUI processes. Issue
[#29](https://github.com/Tjemmmic/termeric/issues/29) established the bounded
foundation and [#95](https://github.com/Tjemmmic/termeric/issues/95) added exact
local command-tab launch. [#97](https://github.com/Tjemmmic/termeric/issues/97)
adds the bounded fresh-window companion; this remains narrower than a general
remote-control API.

## Command line

The command is display-free and discovers every live Termeric GUI owned by the
current Unix user:

```bash
termeric control list
termeric control list --json
termeric control get-text PROCESS WINDOW TAB PANE --history-lines 40 --bytes 16384
termeric control get-text PROCESS WINDOW TAB PANE --json
termeric control send-text PROCESS WINDOW TAB PANE 'cargo test' --enter
termeric control send-text PROCESS WINDOW TAB PANE 'multi-line text' --paste --json
termeric control new-window PROCESS SOURCE_WINDOW
termeric control new-window PROCESS SOURCE_WINDOW --json
termeric control launch-window PROCESS SOURCE_WINDOW -- /usr/bin/printf '%s\n' 'literal $HOME; value'
termeric control launch-window PROCESS SOURCE_WINDOW --cwd /absolute/project --title build --json -- cargo test --workspace
termeric control new-tab PROCESS WINDOW
termeric control new-tab PROCESS WINDOW --json
termeric control launch-tab PROCESS WINDOW -- /usr/bin/printf '%s\n' 'literal $HOME; value'
termeric control launch-tab PROCESS WINDOW --cwd /absolute/project --title build --json -- cargo test --workspace
termeric control split-pane PROCESS WINDOW TAB PANE horizontal
termeric control split-pane PROCESS WINDOW TAB PANE vertical --json
termeric control focus-window PROCESS WINDOW
termeric control select-tab PROCESS WINDOW TAB
termeric control focus-pane PROCESS WINDOW TAB PANE
termeric control close-window PROCESS WINDOW
termeric control close-tab PROCESS WINDOW TAB
termeric control close-pane PROCESS WINDOW TAB PANE
termeric control move-tab PROCESS SOURCE_WINDOW TAB DESTINATION_WINDOW DESTINATION_INDEX
termeric control move-pane PROCESS SOURCE_WINDOW SOURCE_TAB PANE DESTINATION_WINDOW DESTINATION_TAB TARGET_PANE horizontal
termeric control resize-window PROCESS WINDOW WIDTH HEIGHT
termeric control resize-divider PROCESS WINDOW TAB DIVIDER RATIO
```

Human output is intended for inspection. JSON is the stable scripting shape and
wraps the results in a top-level object with `schema_version: 3` and a
`processes` array. Each process reports its PID,
stable process-local window IDs, ordered tabs, stable window-local tab IDs,
stable tab-local pane and divider IDs, divider direction/ratio/ownership,
window visibility/quick-access identity, focus/active/pinned/activity/closed state, sanitized titles, optional
live-domain identity, native-window physical-pixel dimensions, and terminal cell
dimensions. Multiple
GUI processes are returned in PID order.
Schema 3 adds required per-window `visible` and `quick_access` fields; schema 2
added the required per-tab `dividers` array. Consumers must negotiate the
top-level version instead of assuming an older shape.

Window IDs remain stable until their owning GUI process exits, even when sibling
windows close. Tab and pane IDs retain their existing runtime stability within
their owning window and tab. No ID is promised to survive process restart or
workspace relaunch.

An empty successful discovery prints `No running Termeric GUI processes.` or
returns `{"schema_version": 3, "processes": []}` in JSON mode. A live endpoint that cannot
authenticate or answer fails the command instead of silently returning a
partial healthy-looking inventory. Endpoints belonging to exited PIDs and
ordinary connection-refused crash residue are ignored.

`new-window` targets one discovered process and one exact stable source-window
ID. The new window inherits that source window's current validated
configuration and the focused tab's working directory, matching the in-app New
Window action. Success is returned only after native window, renderer, and
initial tab creation complete, and includes the new stable window ID.

`launch-window` uses the same exact stable source-window selection and validated
configuration cloning, but its sole initial tab directly executes the program
and ordered argv after the required `--`. It accepts the same optional absolute
UTF-8 `--cwd` and bounded initial `--title` as `launch-tab`. Without `--cwd`, it
uses the source's focused-tab cwd, then the GUI process cwd when unavailable. A
selected inherited path that is not an available local directory fails with an
explicit `--cwd` remedy rather than silently changing directories. A source
live-domain tab remains only a configuration/cwd template: the new window and
PTY are ordinary local owners and no daemon mutation occurs. Success is returned
only after hidden native-window, renderer, PTY, and tab initialization complete,
then includes both the new process-stable window ID and initial window-local tab
ID. Missing executables and other initialization failures leave no managed
window or stable success result.

`new-tab` targets one exact stable window ID and returns its stable window-local
tab ID only after the PTY-backed tab exists. An ordinary target creates the same
local shell tab as the in-app New Tab action. If the active tab belongs to a live
domain, protocol v14 keeps the socket worker off the GUI event loop while the
daemon creates the authoritative tab, reconciles the returned snapshot, and
then returns the resulting GUI-local stable ID.

`launch-tab` targets one exact stable window but deliberately creates only an
ordinary local tab. Everything after the required `--` is preserved as one
program and an ordered argument vector and passed directly to the PTY spawn
path; Termeric does not concatenate or evaluate a shell command. Empty
arguments, whitespace, Unicode, leading hyphens, and shell metacharacters
therefore remain literal. `--cwd` accepts an existing absolute UTF-8 directory
and `--title` supplies an optional initial title. If no cwd is supplied, the
child uses the same GUI-process working-directory behavior as other direct
command tabs. The inherited process environment is unchanged and the protocol
does not accept environment overrides. The new tab becomes active and success
returns its stable window-local ID. A missing executable, invalid directory,
closing/uninitialized window, capacity failure, or active live-domain tab fails
without inserting a tab. Use `termeric domain spawn DOMAIN -- PROGRAM [ARG ...]`
for a daemon-owned command instead of implicitly crossing that authority
boundary.

`split-pane` targets one exact process/window/tab/pane path plus a typed
`horizontal` or `vertical` direction. A fresh unsplit tab starts with pane ID
`0`; a pane surviving collapse keeps its existing ID, so automation always uses
the current inventory rather than assuming zero. Success creates exactly one
local shell pane, focuses it within its tab, preserves the window's active-tab
choice, and returns the new tab-local pane ID. A live-domain target uses the
same deferred protocol-v14 acknowledgement as `new-tab`: the response is sent
only after the daemon snapshot has been reconciled and the new remote pane has
an authoritative GUI-local stable ID.

`select-tab` selects one exact stable tab without requesting native-window
activation. `focus-pane` validates the complete window/tab/pane path before
changing anything, then selects the tab and focuses the pane as one event-loop
action. Both report whether their respective state changed. A live-domain pane
uses the normal in-app behavior: exact local focus changes immediately and the
authoritative domain focus update proceeds asynchronously.

`focus-window` is intentionally described and returned as a native focus
*request*, not proof that another application lost focus. It succeeds directly
when the target is already focused, rejects hidden or minimized windows, and
requests activation otherwise. Wayland is rejected explicitly because Winit
cannot request application-driven focus there; the compositor or user must
activate the window. Scripts can poll `control list --json` for the later
`focused` state on supported backends.

`termeric --quick-access` uses inventory only to discover a window explicitly
marked `quick_access`, rejects ambiguous or truncated discovery, then sends the
versioned, idempotent, exact-window `set_quick_access_visibility` request. The
event loop revalidates the target identity before changing native visibility.
The client can safely retry the same request ID and desired state once after an
ambiguous transport failure. This operation is not a general public control
subcommand; its single-owner startup lock and native behavior are defined in
[quick-access.md](quick-access.md).

`close-window` schedules the same window-local shutdown as a native close
request. `close-tab` validates an exact local tab and starts its normal shrink
and process teardown lifecycle. Their responses say `scheduled`; they do not
claim that animation, persistence, or process teardown already finished.
`close-pane` validates the complete local path and last-pane safety before
mutation, removes the exact pane synchronously, and returns both the remaining
pane count and focused pane ID. If two panes collapse to one terminal owner, the
survivor keeps its stable ID through inventory, workspace capture/restore, and
the next split. For live-domain tabs and panes, protocol v14 defers the response
until the authoritative daemon snapshot proves the exact remote object was
removed and GUI reconciliation can report the surviving local pane state. The
domain's last pane and last tab remain protected; terminating the domain is a
separate explicit operation.

`move-tab` identifies the source tab and both windows by stable ID, then places
the tab at an explicit zero-based final visual index. A same-window reorder
preserves both the tab ID and the previously selected tab. A cross-window move
preserves the complete live tab owner, makes it active in the destination,
returns its fresh destination-local tab ID, and reports whether moving the last
source tab scheduled that window to close. The complete source and destination
are validated before detachment. An out-of-range index, a pinned/unpinned group
boundary crossing, or a closing object fails without moving anything. A
live-domain tab can be reordered within its current native window when the
requested visual destination is another slot owned by the same domain. The
daemon receives the corresponding domain-local index, and success is returned
only after reconciliation places the same GUI-stable tab ID at the requested
visual index. Cross-window live transfer and placement into an ordinary or
different-domain slot remain unrepresentable and fail before mutation.

`move-pane` identifies complete source and destination paths, including the
exact destination anchor pane and a typed split direction. It transfers the
existing PTY and terminal state between different ordinary tabs in the same or
different process-owned windows, focuses the fresh destination-local pane, and
returns its new ID. The last source pane cannot move, and a tab-owned plugin
pane must move with its whole tab. Moving within the same tab is currently
rejected because that requires an explicit layout-tree rearrangement contract,
not an ownership transfer. Live-domain panes can move between different tabs in
the same domain and native window. Domain protocol v10 carries the exact remote
anchor and requested direction; protocol v14 responds only after the source
route disappears, the destination layout contains that exact split, and the
moved relay has a destination-local stable ID. Cross-window, cross-domain, and
mixed ordinary/live moves fail before mutation.

`resize-window` requests an exact native inner size in physical pixels, bounded
to 160x120 through 16384x16384. Its response is deliberately a request
acknowledgement: some platforms return an immediate applied size, while others
apply or clamp it asynchronously through the window manager. Scripts must not
treat the requested dimensions as proof of the final native size; poll schema 2
inventory for the target window's later `pixel_width` and `pixel_height`.

Every split in inventory has a tab-local runtime divider ID, direction, and
100-900 thousandths ratio. Ordinary local dividers receive monotonic IDs that
survive ratio changes, swaps, balance, new splits, and unrelated subtree
collapse. Rebuilding the complete layout with a preset creates new divider
objects and therefore new IDs. `resize-divider` synchronously targets one exact
ordinary divider ID, reports its previous ratio and whether state changed, and
fails stale IDs without falling back to a structural position. It rejects
zoomed tabs. A live-domain divider uses its daemon split ID, captures the
previous authoritative ratio, and responds only after the returned snapshot
reconciles to the requested ratio; an absent or mismatched split fails
terminally rather than acknowledging an uncertain layout.

`get-text` is a read-only query against one complete stable
process/window/tab/pane path. It returns the terminal's presented plain UTF-8
viewport and can prepend up to 256 requested primary-screen history rows. The
caller selects a byte ceiling from 1 through 65,536; when that ceiling is
reached, older content is discarded first, any partial line retains its newest
UTF-8-safe suffix, and `truncated` is reported. JSON additionally identifies
`primary` versus `alternate` screen and the number of contributing history
rows. Alternate-screen queries are always viewport-only and cannot read the
hidden primary buffer. Queries use the presentation snapshot, so live bytes
held behind synchronized-output mode are not exposed before presentation. This
is intentionally a fresh read rather than a replay-cached mutation.

`send-text` is the only GUI input action and is disabled by default. The user
must set `security.allow_gui_control_input = true` before authenticated control
clients can use it. The same global policy also gates separately granted API
v17 plugin terminal input, so every external writer shares one explicit opt-in.
The control action accepts at most 64 KiB of UTF-8 text and permits
printable characters, tab, and newline—not arbitrary bytes, carriage returns,
escape sequences, or other controls. `--paste` applies the exact target
terminal's live bracketed-paste mode; `--enter` appends one carriage return
after the text or completed paste frame. Neither option changes focus,
selection, broadcast state, or the active tab.

Plugin-tab input matching the destructive-command policy is captured as an
exact destination write and opens the existing visible confirmation dialog.
The response reports `pending_confirmation` and its UI-local confirmation ID;
no bytes are written unless the user confirms. While a confirmation is
pending, further control input fails instead of changing the command line
behind the modal. Ordinary shell/profile/live-domain input is sent directly
after complete path and policy validation.

Every mutation carries a 128-bit lowercase-hex request ID. The CLI generates
one by default and reports it on success. `--request-id ID` lets an ambiguous
timeout be retried as the same logical action. Each GUI caches the 256 most
recent successful mutations: an exact retry returns the original result without
creating another object, while reuse for different parameters fails closed. An
exact retry received while an acknowledged live-domain mutation is pending
joins that request and receives the same final result; it never sends a second
daemon mutation. Completed live-domain failures are replayed permanently while
cached because an authority or transport error may follow a mutation that
actually took effect. Other failed actions reserve their ID/parameter pairing
but may be reevaluated safely because they created no object.
Input is the exception: a PTY error can occur after a prefix was accepted, so a
failed `send-text` request ID permanently replays the same failure while it
remains in the cache. Use a new ID only after inspecting and correcting the
terminal state. A successful retry never duplicates the write, and a replayed
confirmation request never creates a second prompt.
After an ID ages out of that explicitly bounded cache, its at-most-once replay
guarantee no longer applies.

## Security and privacy boundary

Each GUI process owns a separate `gui-v1-PID.sock` and `gui-v1-PID.token` under
Termeric's user-private runtime directory. A safe `$XDG_RUNTIME_DIR/termeric` is
preferred; the fallback is `/tmp/termeric-UID`. The directory is descriptor-
validated, user-owned, non-symlinked, and mode `0700`. Sockets and independent
256-bit random token files are mode `0600`. Both client and server verify the
connected peer's operating-system user in addition to token authentication.
Token files are opened without following symlinks and validated by owner, type,
permissions, and exact size before their fixed-size contents are read.

Protocol version 14 accepts exactly `inventory`, ID-targeted `read_text`,
`send_text`, `create_window`, `launch_window`, `create_tab`, `launch_tab`,
`split_pane`, `focus_window`, `select_tab`, `focus_pane`,
`close_window`, `close_tab`, `close_pane`, `move_tab`, `move_pane`,
`resize_window`, `resize_divider`, and the exact-window `set_quick_access_visibility`
operation used only after inventory marks that window as quick access. It does **not** accept arbitrary-byte
input, environment overrides, arbitrary plugin calls, or domain controls. Text is returned only by an explicit bounded
`read_text` request; inventory never includes terminal contents, scrollback,
working directories, or argv (a caller supplies launch argv but cannot inspect
another tab's),
environment, clipboard data, transcripts, context, diffs, or AI/provider
credentials. Titles and identifiers are the only user-controlled strings in
published inventory; control characters are removed before publication so
human output cannot inject terminal escape sequences.
Submitted program, argv, cwd, environment values, and provider credentials are
not written to process logs. A failed private window launch may return its
detailed startup diagnostic to the authenticated requester while logging only a
generic initialization failure.

The socket authenticates same-user local processes, not mutually distrustful
programs running under the same Unix account. Any process that can read the
private token as that account can inspect the metadata and issue the explicit
bounded text query described above. When the opt-in input policy is enabled,
that process can also write and execute text in exact panes. The protocol must not be treated as a
sandbox boundary between applications owned by one Unix user. Even when
`security.allow_gui_control_input` is false, possession of the token authorizes
the explicitly bounded `launch_tab` and `launch_window` operations and therefore
direct local process execution as that user.

## Resource and failure policy

- Frames are four-byte big-endian length-prefixed JSON and are limited to
  256 KiB. Empty, truncated, oversized, unknown-field, and incompatible-version
  frames fail closed.
- One text response is limited to 64 KiB of valid UTF-8 and at most 256 requested
  primary-history rows. Newest presented content wins when the byte ceiling is
  reached; alternate screens never authorize primary history.
- One input request is limited to 64 KiB of typed UTF-8 plus an optional Enter.
  Protocol validation rejects C0/C1 controls other than tab/newline before the
  event loop or PTY sees them.
- One direct tab or window launch accepts at most 128 arguments and 16 KiB
  across its program and argument strings. An optional absolute cwd is capped
  at 4 KiB and an
  optional initial title at 256 UTF-8 bytes. NUL-bearing argv and control or
  bidirectional-formatting metadata fail before process or tab creation.
- Reads and writes time out after 12 seconds. The GUI event-loop request must
  answer within 10 seconds, and socket I/O never runs on that event loop. A
  timed-out live-domain mutation can be retried with the same request ID to
  join a still-pending operation or replay its terminal result.
- Each GUI accepts at most 16 concurrent control clients.
- Mutation request IDs are exactly 32 lowercase hex digits. The 256-entry
  process-local replay cache owns only bounded requests, responses, and
  single-line 1 KiB errors.
- One response is capped at 32 windows, 256 tabs, 1,024 panes, 1,023 dividers,
  and 32 KiB of sanitized string data. Per-window and per-tab ceilings are 256
  tabs, 512 panes, and 511 dividers. `truncated` is set when any process,
  window, tab, pane, divider, or metadata
  budget prevents a complete inventory.
- Control-created windows are rejected once the process has 32 live or pending
  windows, so a successful new stable ID can never be immediately hidden beyond
  the inventory ceiling.
- `new-tab` and `launch-tab`, plus the mandatory initial tab of each
  control-created `new-window` or `launch-window`, are rejected at 256 tabs in
  the process.
  Tab actions also enforce the 256-tab target-window ceiling, so a successful
  new stable ID remains discoverable within both inventory ceilings.
- Control-created panes are rejected at 512 panes in the target tab or 1,024 in
  the process, so their stable IDs remain discoverable within inventory limits.
- The maximum permitted inventory shape is serialization-tested against the
  frame ceiling. Discovery itself inspects at most 128 runtime-directory
  entries.
- GUI startup remains available if its control endpoint cannot be created; the
  failure is logged and `termeric control list` cannot discover that process.
  Normal shutdown joins the listener and removes its process-scoped socket and
  credential. Crash residue is harmless and ignored after liveness checks.

The implementation has hermetic framing, negotiation, credential/path,
multi-process discovery, output-format, metadata-sanitization, and maximum-shape
tests. Unix-socket exchange tests run where the test sandbox permits AF_UNIX
I/O. No test launches a plugin, provider-backed AI command, or network service.

## Deliberate next steps

The acknowledged topology surface now covers every live-domain mutation exposed
by the running-GUI commands without pretending daemon topology can place tabs
across native windows. Richer typed workspace/query actions, environment
overrides, and process-exit observation can be added only by demonstrated
automation demand. Arbitrary byte input and implicit
focus-following actions remain deliberately excluded rather than becoming
consequences of the control socket.
