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
Plugin-pane movement remains guarded under
[#126](https://github.com/Tjemmmic/termeric/issues/126).

## Evidence and deferred checks

`cargo test -p termeric plugin_pane_creation_tests` uses temporary fake
executables and real local PTYs. It checks explicit palette dispatch,
same/different-plugin profiles, literal `default`, argv/cwd/environment/kernel
geometry, original-owner preservation, workspace round-trips, cwd policies,
failure rollback, refusal boundaries, and close/collapse/resplit identity.
Shared launch tests reject mismatched or ambiguous synthetic hook responses;
declarative hook tests check active/inherit cwd semantics without running tools.

No authenticated Codex, Claude Code, or other provider workflow was run. Native
visual acceptance and real-provider behavior remain deferred to the user;
successful PTY creation is not proof of provider readiness.
