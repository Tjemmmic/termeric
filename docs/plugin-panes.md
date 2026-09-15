# Explicit plugin panes

The command palette includes **New Codex Pane**, **New Claude Code Pane**, and
equivalent commands for enabled, registered declarative plugins with launch
profiles. Choose **Right** (vertical divider) or **Below** (horizontal divider),
then the command for **Automatic profile** or an explicit **Profile: NAME**.
These commands start a fresh process; they do not resume a provider conversation.
Existing New Tab/Open/Resume commands retain their existing behavior.

Automatic selection uses the plugin's normal repository/configured default
rules. Explicit names take priority, including a profile literally named
`default`. The resolved reference is stored on the new terminal owner and saved
in [schema-3 workspaces](workspaces.md). Configuration changes refresh the
commands; stale selections fail rather than silently launching another profile.
Event-only plugins do not advertise pane commands.

## Placement and safety

The focused terminal when the command is executed is the split target. The
launch stays bound to that stable tab/pane identity. `[panes] cwd` chooses that
target's effective directory (`focused`), its launch primary's directory (`tab`),
or Termeric's current process directory (`process`). This explicit pane policy
takes precedence over plugin tab-launch cwd defaults, including declarative
`working_directory = "inherit"`. An unavailable selected directory is refused.
Plugin-provided `PWD` cannot redirect the child.

The host sends a targeted `PaneLaunch` hook and requires exactly one matching
program/profile/cwd response. Notices indicating fallback, multiple launches,
and unrelated actions are refused without executing those returned actions.
The new PTY and built-in `COLUMNS`/`LINES` environment use the same prospective
child geometry as the split tree, even when the tab was zoomed. Declarative
plugins with `workspace-query` receive those dimensions in their existing
versioned launch context. This adds no structured-handler mutation authority.

Creation requires an open, writable ordinary local process terminal and room
under the 64-pane workspace ceiling. Closing, missing, read-only, serial, and
live-domain targets are refused. Armed or active broadcast must be turned off
first. Missing plugins/profiles, malformed responses, and spawn failure leave
the original owners, focus, layout, and zoom intact. The first split adopts the
original PTY and spawns only the requested child; it does not start a spare shell.

Each created plugin terminal owns fresh session metadata, transcript state, and
diff tracking. Closing a sibling and collapsing/resplitting preserve that owner
and its exact profile. Ordinary split commands still launch ordinary shells or
the tab's process profile; they never implicitly clone the focused plugin.
Plugin panes can move between ordinary local tabs and process-owned windows
through the existing pane-move commands, exact GUI control, and granted plugin
pane-move operations. The same live PTY, reader, terminal, launch configuration,
profile, transcript worker, UTF-8 carry, partial diff, and notices move together;
no plugin launch hook or provider restart runs. The destination assigns a fresh
local pane ID. Pending actions addressed to the old source stay stale rather
than retargeting another owner. Read-only state travels with the pane.

All move paths validate source/anchor liveness, the last-source-pane rule,
destination capacity, and pane/divider allocation history before detaching.
This includes allocation history retained after collapse. Failure preserves
both layouts and processes. Successful app-mediated moves select the destination,
clear broadcast/copy-mode/mouse interaction state, and update source-primary
presentation. Existing same-tab layout actions remain separate; moving the last
pane, local-to-live-domain placement, and serial placement stay unsupported.
Use a whole-tab move for a single-terminal source.

Workspace capture retains each moved owner's launch reference. Moving between
windows does not copy configuration or secrets into the destination: a future
restore still requires the referenced plugin/profile there. Removal of that
configuration does not terminate the already-running moved process.

## Evidence and deferred checks

`cargo test -p termeric plugin_pane_creation_tests` uses temporary fake
executables and real local PTYs. It checks explicit palette dispatch,
same/different-plugin profiles, literal `default`, argv/cwd/environment/kernel
geometry, original-owner preservation, workspace round-trips, cwd policies,
failure rollback, refusal boundaries, and close/collapse/resplit identity.
Shared launch tests reject mismatched or ambiguous synthetic hook responses;
declarative hook tests check active/inherit cwd semantics without running tools.

`cargo test -p termeric plugin_pane_transfer_tests` additionally checks primary
and sibling plugin transfers, same-plugin profile isolation, continued PTY and
recorder identity, split UTF-8 and partial-diff completion across transfer,
read-only/protocol/stale-write exclusions, stale profile choices, archive
save/load/restore, and exhausted collapsed destination IDs. Existing app,
control, and plugin tests cover exact destination routing, native-window
controller handoffs, interaction cleanup, and refused topology boundaries.

No authenticated Codex, Claude Code, or other provider workflow was run. Native
visual acceptance and real-provider behavior remain deferred to the user;
successful PTY creation is not proof of provider readiness.
