# Multi-window and command-line launch contract

Termeric parses startup arguments completely before creating the event loop.
It also loads configuration and validates every explicit shell, command, and
terminal-profile cwd before connecting to a display server. Invalid and
conflicting options, missing directories, and non-UTF-8 PTY cwd values return a
nonzero error without opening a window. `--help` and `--version` also require no
display server.

## Grouping model

One invocation owns all requested native windows in one winit event loop. The
first window and first tab are implicit. `--new-window` starts another window
group; a leading `--new-window` is accepted as a fresh one-window desktop-launch
compatibility flag and suppresses automatic restore. `--tab` starts another tab in the current window. Options
apply to the current window or tab until the next delimiter.

```bash
# Two shell tabs in one window, then an independent workspace window
termeric --working-directory ~/src --tab --working-directory /tmp \
  --new-window --workspace workspace-20260826

# A shell tab with one right-hand pane
termeric --working-directory ~/src --split vertical
```

Every requested window owns its renderer/surface and redraw scheduler, tabs and
panes, UI/title/focus state, clipboard routing, plugin manager, PTYs, and
workspace archive. PTY and transcript wakeups are broadcast across windows,
but an event is consumed only by its owning terminal. Renderer device loss is
recreated per window without replacing PTYs; unrecoverable out-of-memory or
adapter failure removes only that window, while other windows continue. The
event loop exits only after the last window closes.

## Runtime window view

F11 toggles borderless fullscreen for the focused native window. The same
**Toggle Fullscreen** action is available in the command palette and Settings >
Keyboard Shortcuts. Each window owns its state independently. Fullscreen is
transient presentation state: it is not copied when tabs or panes move and is
not saved in workspace archives. A compositor-driven resize continues through
the ordinary per-window geometry path, which updates the renderer and every
visible terminal PTY.

`termeric --quick-access` toggles or launches one persistent drop-down terminal
for a desktop global shortcut. Its initial monitor-relative geometry, ownership
lock, authenticated discovery, visibility rules, persistence boundary, and
Wayland compositor caveat are defined in [quick-access.md](quick-access.md).

## Options

| Option | Meaning |
|---|---|
| `--new-window` | Begin another native-window group (or explicitly request the implicit first window when leading). |
| `--quick-access` | Toggle the existing dedicated drop-down terminal, or launch exactly one frameless monitor-relative always-on-top window. |
| `--tab` | Begin another tab in the current window. |
| `--split horizontal\|vertical` | Add a shell pane by sequentially splitting the focused pane after its tab launches. |
| `--working-directory PATH`, `--cwd PATH` | Set the current tab's cwd. It must exist; Unicode paths are supported. |
| `--command PROGRAM` | Run one program directly with no arguments. |
| `-- COMMAND ARG...` | Run a direct command and preserve every remaining argument, including values beginning with `-`; this must be last. |
| `--terminal-profile NAME` | Launch a reusable process or Linux serial entry from `[profiles.NAME]`; exact transport settings come from the profile. |
| `--profile PLUGIN:PROFILE` | Launch an enabled built-in plugin with exactly the named profile, such as `codex:review`. Missing plugins/profiles/CLIs fail visibly. |
| `--workspace ID` | Restore one complete window workspace. It is exclusive with tab/cwd/title/command/profile/split controls in that window. |
| `--title TITLE` | Set the current tab's initial title. |
| `--geometry WIDTHxHEIGHT` | Override the current window's initial physical-pixel size (160x120 through 16384x16384). |
| `--class NAME`, `--app-id NAME` | Set both X11 WM_CLASS naming and Wayland application ID on Linux. |
| `--renderer auto\|gpu\|software` | Override adapter selection for every initial window. `auto` tries hardware then WGPU software; forced modes are strict. |
| `domain protocol-version [--json]` | Print the local executable's expected live-domain wire version without starting or contacting a daemon. |
| `control list [--json]` | Inspect bounded metadata for every already-running local GUI process without opening a display. |
| `control launch-status PROCESS REQUEST_ID [--json]` | Query the initial child of a retained exact command launch, including its portable exit result after terminal cleanup. The CLI status reports query success; inspect JSON for the child result. |
| `control wait-launch PROCESS REQUEST_ID [--timeout SECONDS] [--json]` | Wait for a retained initial child using short read-only queries. Return its portable code (0–255; larger codes map to 1). An optional 1–86400-second timeout stops only the waiter. Errors produce stderr and no result on stdout. |
| `control new-window PROCESS SOURCE_WINDOW [--request-id ID] [--json]` | Create a native window from an exact running source window; an explicit request ID safely retries an ambiguous recent result. |
| `control launch-window PROCESS SOURCE_WINDOW [--cwd ABSOLUTE_DIRECTORY] [--title TITLE] [--env NAME=VALUE ...] [--unset-env NAME ... | --clear-env] [--request-id ID] [--json] -- PROGRAM [ARG ...]` | Launch one exact argv vector plus bounded non-secret environment overlays as the initial tab of a new source-configured native window and return both stable IDs. |
| `control new-tab PROCESS WINDOW [--request-id ID] [--json]` | Create a local shell tab in an exact running window and return its stable ID. |
| `control launch-tab PROCESS WINDOW [--cwd ABSOLUTE_DIRECTORY] [--title TITLE] [--env NAME=VALUE ...] [--unset-env NAME ... | --clear-env] [--request-id ID] [--json] -- PROGRAM [ARG ...]` | Launch one exact argv vector plus bounded non-secret environment overlays directly in a new ordinary local tab and return its stable ID. |
| `control split-pane PROCESS WINDOW TAB PANE horizontal\|vertical [--request-id ID] [--json]` | Split an exact local pane and return the new stable pane ID. |
| `control focus-window PROCESS WINDOW [--request-id ID] [--json]` | Request focus for an exact native window on supported backends; Wayland fails explicitly. |
| `control select-tab PROCESS WINDOW TAB [--request-id ID] [--json]` | Select an exact stable tab without activating its native window. |
| `control focus-pane PROCESS WINDOW TAB PANE [--request-id ID] [--json]` | Atomically select an exact tab and focus an exact stable pane. |
| `control close-window PROCESS WINDOW [--request-id ID] [--json]` | Schedule an exact native window to close. |
| `control close-tab PROCESS WINDOW TAB [--request-id ID] [--json]` | Schedule an exact local tab to close through its normal animation/lifecycle. |
| `control close-pane PROCESS WINDOW TAB PANE [--request-id ID] [--json]` | Immediately close an exact local pane and report the remaining count/focus. |
| `control move-tab PROCESS SOURCE_WINDOW TAB DESTINATION_WINDOW DESTINATION_INDEX [--request-id ID] [--json]` | Move an exact local tab to a zero-based visual index and return its destination-local ID. |
| `control move-pane PROCESS SOURCE_WINDOW SOURCE_TAB PANE DESTINATION_WINDOW DESTINATION_TAB TARGET_PANE horizontal\|vertical [--request-id ID] [--json]` | Transfer an exact local pane beside an exact destination pane and return its destination-local ID. |
| `control resize-window PROCESS WINDOW WIDTH HEIGHT [--request-id ID] [--json]` | Request an exact native-window inner size in physical pixels. |
| `control resize-divider PROCESS WINDOW TAB DIVIDER RATIO [--request-id ID] [--json]` | Set an exact ordinary divider to a 100-900 thousandths ratio. |
| `plugin SUBCOMMAND` | Validate, install, inspect, configure, enable/disable, update, or remove local declarative plugins without opening a window. |
| `-h`, `--help` | Print complete usage without opening a window. |
| `-V`, `--version` | Print the package version without opening a window. |

Paths supplied to `--working-directory` use the operating system path type
during parsing. The current PTY boundary requires a UTF-8-representable path;
failure is explicit. Direct command argv is passed as individual arguments and
never joined or evaluated through a shell.

## Precedence and persistence

Precedence is deterministic:

1. Explicit CLI window/tab launch controls win over automatic restore.
2. `--workspace` supplies that window's complete tab/pane launch plan; only
   geometry and application identity may accompany it.
3. CLI cwd/title/command/profile controls override configuration defaults for
   the current tab. Explicit cwd/title override a selected terminal profile;
   its program, argv, and environment remain exact. Unspecified shell, font,
   theme, and window values come from configuration.
4. `--renderer` overrides `[renderer].backend` for every initial window and is
   accepted once per invocation.
5. Repeated scalar cwd/title/geometry/identity options use the last value.
   Selecting more than one command/profile target for a tab is an error.

Reusable terminal-profile syntax, bounds, pane inheritance, and workspace
behavior are defined in [terminal-profiles.md](terminal-profiles.md).

Normal exit saves each configured window to its own complete workspace archive.
Application state stores all live window archive IDs for the next automatic
restore and retains the older single-ID field for migration. Intentionally
closing one window while siblings remain removes it from that automatic set
without deleting its manually recoverable archive.

## Controlling live domains

`termeric control list [--json]` discovers multiple process-scoped private
endpoints and reports stable runtime window/tab/pane IDs, state, sanitized
titles, native physical-pixel size, cell geometry, and stable divider
IDs/directions/ratios. `termeric control new-window PROCESS SOURCE_WINDOW`
then creates one window from an exact listed source, inheriting its configuration
and focused cwd. `termeric control new-tab PROCESS WINDOW` creates a shell tab
in one exact listed window and returns its stable window-local ID. When the
active tab belongs to a live domain, it waits for the daemon's authoritative
snapshot and returns only after the new remote tab has a reconciled GUI-local
ID. `termeric control launch-tab PROCESS WINDOW [options] -- PROGRAM [ARG ...]`
instead creates an ordinary local tab, preserves exact UTF-8 argument
boundaries without invoking a shell, optionally applies an existing absolute
cwd and initial title, activates the tab, and returns its stable ID. It inherits
the GUI process environment by default; `--unset-env NAME` removes inherited
variables, while `--clear-env` starts from Termeric's documented minimum instead.
Each repeatable `--env NAME=VALUE` overlay is applied exactly afterward. These
options affect only this child. An active live-domain target is rejected; use `domain spawn`
for that authority. Literal values are not a secret-injection mechanism because
shell history and process-argument inspection may expose them. `termeric control
launch-window PROCESS SOURCE_WINDOW [options] -- PROGRAM [ARG ...]` applies the
same exact-argv, cwd, title, environment, and replay contract to the sole initial
tab of a new ordinary local window. The source supplies validated configuration
and, unless `--cwd` is given, focused cwd; an inherited path unavailable locally
fails explicitly. Success returns both the process-stable window ID and its
initial window-local tab ID only after native, renderer, and PTY initialization.
It does not route through a source live domain. `termeric control split-pane
PROCESS WINDOW TAB PANE horizontal|vertical`
targets the complete stable-ID path and likewise acknowledges an ordinary or
live-domain split only after it can return the new tab-local pane ID.
`select-tab` and `focus-pane`
validate the complete stable-ID path before changing local state and report
whether the tab or pane changed. Live-domain pane focus follows the same
asynchronous authority update as in-app pointer focus after applying the exact
local focus. `focus-window` requests native activation and reports whether the
window was already focused; it fails on Wayland because application-driven
window focus is unsupported there. The CLI generates a
replay-safe request ID; after an ambiguous failure it reports the ID for an
exact `--request-id ID` retry. `termeric control get-text PROCESS WINDOW TAB
PANE [--history-lines N] [--bytes N] [--json]` reads the exact pane's presented
UTF-8 viewport. It can add at most 256 primary-screen history rows and returns
the newest content within a caller-selected ceiling no larger than 64 KiB.
Alternate-screen queries are viewport-only, and synchronized output that has
not been presented remains hidden. GUI control does not expose existing argv,
environment, clipboard, or provider data. A caller can supply bounded argv and
non-secret environment overlays to either direct launch action, but inventory
and text reads cannot inspect existing process argv or environment.
See the complete
[running-GUI control contract](gui-control.md).

`termeric control send-text PROCESS WINDOW TAB PANE TEXT [--paste] [--enter]
[--request-id ID] [--json]` requires the disabled-by-default
`security.allow_gui_control_input` policy, shared with separately granted API
v17 plugin terminal input. It accepts printable UTF-8, tab, and
newline within 64 KiB, never arbitrary control bytes. `--paste` honors the
exact pane's bracketed-paste mode and `--enter` appends one carriage return.
Successful and failed request IDs are replay-stable so an ambiguous or partial
PTY write is never retried as fresh input. Destructive plugin-tab text remains
subject to visible exact-destination confirmation.

`close-window` and an ordinary `close-tab` acknowledge that normal teardown has
been scheduled; `close-pane` acknowledges only after the pane has been removed
and reports the surviving count and focused pane ID. A pane surviving collapse
to a single terminal retains its ID through persistence and the next split.
For a live-domain tab or pane, the command waits for the authoritative daemon
snapshot and GUI reconciliation before acknowledging the exact removal. Closing
the domain's last pane or last tab is rejected; domain termination remains an
explicit `termeric domain kill` operation.

`move-tab` supports same-window reorder and cross-window ownership transfer.
It enforces pinned-group boundaries; cross-window moves assign a fresh
destination-local tab ID and can close an emptied source window. `move-pane`
transfers an existing PTY only between different ordinary tabs, in one or two
windows, using an exact destination anchor and direction. It assigns a fresh
destination-local pane ID and focuses that pane. A live-domain pane can likewise
move between different tabs of the same domain in one window; domain protocol
v10 preserves the exact destination anchor and direction, and GUI control waits
for authoritative reconciliation before returning its destination-local ID.
Last-source-pane, tab-owned plugin-pane, same-tab layout-rearrangement,
cross-window/domain, and mixed ordinary/live moves fail before detachment. A
live-domain tab can reorder within its current window only
to another visual slot owned by the same domain; it waits for authoritative
reconciliation and retains its GUI-local ID. Cross-window or cross-domain live
tab placement remains rejected. Both commands return the IDs needed by later
actions.

`control list` also publishes stable tab-local divider IDs with direction,
ratio, and local/live-domain ownership. `resize-divider` synchronously changes
an exact ordinary divider and returns its previous ratio; stale IDs and zoomed
tabs fail explicitly. A live-domain divider waits for the authoritative daemon
snapshot, verifies the requested ratio after reconciliation, and returns the
previous ratio without treating an uncertain result as success. `resize-window` accepts physical
pixel dimensions from 160x120 through 16384x16384. It acknowledges the native
request and reports an immediate applied size only when the platform supplies
one; the window manager may otherwise apply or clamp the size asynchronously.
Schema 3 inventory exposes the later applied `pixel_width` and `pixel_height`
for polling by stable window ID.

`termeric domain ...` commands are parsed and executed without creating a GUI.
The offline `domain protocol-version [--json]` probe additionally reports the
local executable's exact wire version without discovering or starting a daemon;
it is intentionally rejected under `--host` because an incompatible bridge
cannot carry its own diagnosis. Other commands list, create, inspect, split,
resize stable nested dividers, rename/pin/
reorder stable tabs, swap/balance/rebuild pane layouts, atomically move panes
between stable tabs, focus, close panes/tabs, write, retrieve bounded text,
upload or resume one bounded regular file or directory into an exact pane cwd without overwrite,
list/cancel retained partial uploads,
attach, terminate, and stop persistent local terminal domains. The upload form
is `domain upload DOMAIN PANE SOURCE [--name NAME] [--retain|--resume ID]
[--json]`. `--retain` prints a reusable ID before streaming and keeps an
interrupted partial for up to 24 hours; `domain uploads [--json]` lists partials
and `domain cancel-upload ID [--json]` removes an inactive one. `--resume`
verifies the local prefix against the daemon before sending a suffix and never
silently restarts a missing or mismatched record. Resume survives CLI exit but
not termination or restart of the domain daemon.
Prefix a non-GUI control with `--host SSH_DESTINATION` to run it through the
experimental OpenSSH stdio transport, for example
`termeric domain --host build.example list --json`. The host key and key/agent
authentication must already work non-interactively. A validated entry under
`[domains.ssh_hosts.NAME]` can instead be selected with `--host-profile NAME`;
`termeric domain hosts [--json]` lists the configured inventory without
starting a daemon or SSH process. Both `--host HOST attach DOMAIN` and
`--host-profile NAME attach DOMAIN` open a native window whose topology
subscription, controls, focus updates, reconnects, and pane relays retain the
resolved SSH authority. Exact trust and transport policy are in
[`remote-domains.md`](remote-domains.md).
The in-app command palette action **Attach Live Domain...** presents the same
local and saved-host inventory, discovers domains asynchronously, and opens the
selection in a new native window.
Pane and tab closure is daemon-owned and refuses to implicitly destroy the last
tab; terminating the complete domain requires the separate `kill` command.
`termeric domain attach DOMAIN` and its remote forms create a native window
whose relay panes are reconstructed from daemon-owned state. Domain selection is exclusive with
snapshot-workspace and ordinary tab launch controls. Geometry and app identity
remain window-local rather than part of the live domain.

The complete local syntax, security model, resource limits, and reconnect
semantics are in [`live-domains.md`](live-domains.md).

`termeric plugin ...` is also display-free. It manages only reviewed local
directories; exact syntax, source records, permission grants, API compatibility,
and authoring are in [`plugins.md`](plugins.md).
