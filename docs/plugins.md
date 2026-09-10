# Declarative third-party plugins

Termeric plugin APIs v1 through v15 are constrained local extension formats.
They turn a reviewed TOML manifest into command-palette launch profiles; they
do not load native libraries, execute manifest scripts in the UI process, or
download anything. API v2 can additionally pass one explicitly granted, bounded,
read-only JSON launch-context snapshot to the selected out-of-process profile.
API v3 can invoke a separately declared out-of-process handler for four coarse
lifecycle events and accept only bounded informational UI responses.
API v4 can additionally disclose one completed OSC 133 command region to an
explicitly `terminal-text`-granted handler. It is a bounded semantic completion,
not a raw PTY/output stream. API v5 lets a handler opt into one persistent,
request-correlated newline-delimited process so it can retain state without a
process launch for every event. API v6 lets that persistent handler use its
existing `workspace-query` grant to request a fresh, bounded inventory of the
current window. API v7 adds a separate `workspace-actions` grant for exact
stable-ID tab selection and pane focus in that same window. API v8 adds a
separate `workspace-topology` grant for local tab ordering and split ratios.
API v9 lets the same out-of-process handlers contribute bounded, namespaced
command-palette commands with exact-owner dispatch.
API v10 adds bounded typed non-secret settings, explicit user overrides, and
safe update compatibility checks without adding process or host authority.
API v11 adds a separate `terminal-query` grant for one bounded read of the
presented text of an exact current-window pane. It is neither a PTY stream nor
terminal input, and its result cannot initiate another host operation.
API v12 adds a separate `pane-create` grant for splitting one exact ordinary
pane in the current window. It cannot select a command, close anything, mutate
a live domain, or act again from its correlated result.
API v13 adds an independent `tab-create` grant for creating one ordinary local
shell tab in the originating window. The action has no launch, destination, or
input fields, and its result cannot initiate another host operation.
API v14 adds an independent `pane-close` grant for closing one exact ordinary
pane without retargeting or allowing the last pane in a tab to be destroyed.
API v15 adds an independent `tab-close` grant for scheduling closure of one
exact ordinary local tab while protecting the window's final non-closing tab.
Installation, validation, configuration, live enable/disable, update,
diagnostics, and removal require no rebuild.

The complete sample is
[`examples/plugins/task-runner/termeric-plugin.toml`](../examples/plugins/task-runner/termeric-plugin.toml).
The API v2 structured-context sample is
[`examples/plugins/context-probe/termeric-plugin.toml`](../examples/plugins/context-probe/termeric-plugin.toml).
The API v3 event-only sample is
[`examples/plugins/lifecycle-notifier/termeric-plugin.toml`](../examples/plugins/lifecycle-notifier/termeric-plugin.toml).
The API v4 command-event sample is
[`examples/plugins/command-observer/termeric-plugin.toml`](../examples/plugins/command-observer/termeric-plugin.toml).
The API v5 stateful-handler sample is
[`examples/plugins/persistent-counter/termeric-plugin.toml`](../examples/plugins/persistent-counter/termeric-plugin.toml).
The API v6 live-query sample is
[`examples/plugins/workspace-inspector/termeric-plugin.toml`](../examples/plugins/workspace-inspector/termeric-plugin.toml).
The API v7 navigation sample is
[`examples/plugins/workspace-navigator/termeric-plugin.toml`](../examples/plugins/workspace-navigator/termeric-plugin.toml).
The API v8 non-destructive topology sample is
[`examples/plugins/workspace-organizer/termeric-plugin.toml`](../examples/plugins/workspace-organizer/termeric-plugin.toml).
The API v9 handler-command sample is
[`examples/plugins/command-tool/termeric-plugin.toml`](../examples/plugins/command-tool/termeric-plugin.toml).
The API v10 typed-settings sample is
[`examples/plugins/configurable-command/termeric-plugin.toml`](../examples/plugins/configurable-command/termeric-plugin.toml).
The API v11 presented-terminal sample is
[`examples/plugins/terminal-reader/termeric-plugin.toml`](../examples/plugins/terminal-reader/termeric-plugin.toml).
The API v12 pane-creation sample is
[`examples/plugins/pane-builder/termeric-plugin.toml`](../examples/plugins/pane-builder/termeric-plugin.toml).
The API v13 tab-creation sample is
[`examples/plugins/tab-builder/termeric-plugin.toml`](../examples/plugins/tab-builder/termeric-plugin.toml).
The API v14 pane-closure sample is
[`examples/plugins/pane-pruner/termeric-plugin.toml`](../examples/plugins/pane-pruner/termeric-plugin.toml).
The API v15 tab-closure sample is
[`examples/plugins/tab-pruner/termeric-plugin.toml`](../examples/plugins/tab-pruner/termeric-plugin.toml).

## Install and manage

Review the manifest and its program/argv first, then run:

```bash
termeric plugin validate examples/plugins/task-runner
termeric plugin install examples/plugins/task-runner \
  --allow process-spawn --enable
termeric plugin list
termeric plugin show task-runner --json
termeric plugin configure task-runner --default-profile tests
```

An open Termeric process polls the registry at the existing 750 ms config
deadline and updates command-palette entries without a restart. Settings →
Plugins shows a bounded status summary and opens a keyboard-, pointer-, and
screen-reader-accessible manager for every installed plugin. The manager shows
the exact source path, version, enabled/diagnostic state, requested and granted
permissions, effective setting values, and explicit-override count. It can
edit every declared Boolean, integer, text, and choice setting, restore one
manifest default, enable or disable immediately, and use request-scoped
confirmations before updating from source, resetting all overrides, or removing
the managed copy. Integer/text editing is single-line, UTF-8-safe, bounded, and
available to keyboard, pointer, IME, and screen-reader input. Every save is
revalidated through the same atomic registry path as the CLI, and an editor
whose plugin schema changed after opening fails closed. A removal never deletes
the recorded original source directory.

Installation, grant selection, and default-profile selection remain explicit
CLI operations. Typed setting set/unset is supported both in-app and through the
CLI. `plugin list/show` also remains the untruncated/scriptable diagnostic
record.

```bash
termeric plugin disable task-runner
termeric plugin enable task-runner

# Re-read the canonical source directory recorded at installation.
termeric plugin update task-runner

# Or deliberately replace from another reviewed local directory.
termeric plugin install /reviewed/new/source --replace \
  --allow process-spawn --enable

termeric plugin remove task-runner
```

Install copies only `termeric-plugin.toml` into the Termeric configuration
directory. The registry records the canonical source directory and a
deterministic content fingerprint. The fingerprint is change detection, not a
cryptographic signature; provenance is the exact local path shown before and
after installation. Modifying the installed copy outside this flow makes the
plugin fail closed. The registry, installed plugin directory, manifest, rollback
copy, and administrative temporary files must be real files/directories rather
than symlinks; replacement and removal reject unexpected file types before
following or mutating them. Remote installation, a marketplace, automatic
updates, and signing are not implemented.

## Manifest API v1

```toml
api_version = 1
id = "my-tools"              # lowercase ASCII namespace
name = "My Tools"
version = "1.0.0"
description = "Local launch profiles"
permissions = ["process-spawn"]

[[profiles]]
id = "check"
name = "Run Checks"
description = "Run the project checks"
program = "cargo"
args = ["check", "--workspace"]
working_directory = "active" # active or inherit
```

Every profile contributes `<plugin-id>.open-<profile-id>` under its plugin
name in the command palette. `--profile my-tools:check` and workspace restore
use the same versioned hook and argv construction. A configured default profile
is used only when a restore does not name one; a named archive profile always
wins.

Manifests reject unknown fields, unknown enum values, duplicate profile IDs,
reserved built-in IDs, NULs, multiline identity fields, unsupported API
versions, more than 32 profiles, more than 64 argv entries per profile, entries
over 4 KiB, total profile argv over 32 KiB, and files over 64 KiB. Registries
are schema-versioned, capped at 512 KiB, reject duplicates, relative or
oversized source paths, malformed fingerprints/default profiles, reserved IDs,
and unsupported grants, and contain at most 64 plugins. Both readers stop after
the declared byte ceiling even if a file changes during the read. Plugin
commands must remain inside their ID namespace and conflicts are rejected
before load. A rejected registry unloads third-party entries and is shown as an
error in both Settings and a bounded toast; it is never presented as an empty,
healthy registry.

## Manifest API v2 structured launch context

API v2 retains the complete v1 profile contract and adds one capability:
`workspace-query`. Install it only after reviewing both requested grants:

```bash
termeric plugin validate examples/plugins/context-probe
termeric plugin install examples/plugins/context-probe \
  --allow process-spawn --allow workspace-query --enable
```

When an API v2 plugin both requests and receives `workspace-query`, Termeric
adds one environment variable to that plugin profile's PTY child:
`TERMERIC_PLUGIN_CONTEXT_JSON`. Its compact JSON value is capped at 16 KiB and
has exact schema version 1:

```json
{
  "schema_version": 1,
  "plugin_id": "context-probe",
  "profile_id": "show",
  "active_working_directory": "/home/user/project",
  "tab_count": 3,
  "active_tab_index": 1,
  "terminal": { "columns": 120, "rows": 36 }
}
```

The snapshot is captured for the exact palette, CLI, or workspace-restore
invocation. Workspace restore substitutes its archived working directory before
serialization. The value is read-only and does not update after launch. It
contains no terminal cells or scrollback, commands, selection, transcript,
environment dump, clipboard, Git data, control token, secrets, or filesystem
contents. Like any environment variable, the launched program can pass it to
its descendants; granting it is therefore an intentional disclosure of active
cwd and coarse tab/terminal geometry.

An API v1 manifest cannot receive this grant. Missing grants prevent enablement,
unsupported grants fail installation or enablement, and an oversized context
produces a visible notice instead of launching with a partial or silently
truncated value. This snapshot remains launch-time only; it is not a live query.

## Manifest API v3 lifecycle handlers

API v3 preserves v1 profiles and the optional v2 launch context, then adds one
optional `[event_handler]`. A handler can coexist with profiles or be the only
plugin entry point:

```toml
api_version = 3
id = "lifecycle-notifier"
name = "Lifecycle Notifier"
version = "1.0.0"
permissions = ["process-spawn", "event-subscribe", "ui-actions"]

[event_handler]
program = "my-plugin-handler"
args = ["--stdio"]
events = ["app-ready", "tab-created", "tab-closed", "tab-switched"]
```

Install only the reviewed capabilities:

```bash
termeric plugin install examples/plugins/lifecycle-notifier \
  --allow process-spawn --allow event-subscribe --allow ui-actions --enable
```

For each subscribed event, Termeric starts the declared program off the UI
thread, writes one compact schema-v1 JSON document to stdin, closes stdin, and
reads one schema-v1 response from stdout. Event objects contain only their type
and coarse numeric tab IDs/indices. With a separately requested and granted
`workspace-query`, the envelope also contains active cwd, tab count/index, and
terminal rows/columns; otherwise the entire `context` member is omitted. It
never contains terminal or scrollback text, commands, PTY bytes, selection,
transcripts, environment dumps, clipboard, secrets, Git data, or filesystem
contents.

```json
{
  "schema_version": 1,
  "plugin_id": "lifecycle-notifier",
  "event": { "type": "tab-switched", "from": 0, "to": 1 }
}
```

The only accepted response actions are `show-message` and `show-notice`. A
non-empty action list requires the separate `ui-actions` grant:

```json
{
  "schema_version": 1,
  "actions": [
    { "type": "show-message", "message": "Workspace helper is ready" }
  ]
}
```

API v3 and v4 handlers are process-per-event, and this remains the default in
later APIs. Execution is
serialized through a 32-entry queue, times out after two seconds, caps stdout at
64 KiB and stderr at 4 KiB, accepts at most eight actions, and validates bounded
single-line control-free text. The child receives a cleared environment with
only a minimal `PATH`, plugin ID, and event-schema marker. Queue overflow,
launch/write/read/exit/timeout errors, malformed JSON, unknown fields/actions,
wrong schemas, missing grants, and oversized data fail visibly. Plugin
reload/unload cancels active and queued work from the previous registry
generation and discards any racing stale result. These are informational
callbacks, not terminal mutation, live queries, raw PTY streaming, render
hooks, or a general automation runtime.

## Manifest API v4 semantic command events

API v4 preserves every earlier contract and adds one event kind,
`command-completed`. The manifest must request `process-spawn`,
`event-subscribe`, and `terminal-text`, and the operator must grant all three:

```toml
api_version = 4
id = "command-observer"
name = "Command Observer"
version = "1.0.0"
permissions = ["process-spawn", "event-subscribe", "terminal-text"]

[event_handler]
program = "my-command-handler"
args = ["--stdio"]
events = ["command-completed"]
```

```bash
termeric plugin install examples/plugins/command-observer \
  --allow process-spawn --allow event-subscribe --allow terminal-text --enable
```

The handler receives the same schema-v1 envelope and worker isolation as API
v3. One event identifies the producing application-local tab, optional stable
pane, and terminal-local OSC 133 region. Text exists only while that exact
completed region still has authoritative backing rows:

```json
{
  "schema_version": 1,
  "plugin_id": "command-observer",
  "event": {
    "type": "command-completed",
    "region_id": 44,
    "tab_id": 7,
    "pane_id": 3,
    "exit_code": 0,
    "content_available": true,
    "command": { "text": "cargo test", "truncated": false, "sanitized": false },
    "output": { "text": "…", "truncated": true, "sanitized": false }
  }
}
```

Command extraction allocates at most 4 KiB and output extraction at most 64
KiB before dispatch. The public event then allows at most 2 KiB of JSON-encoded
command string data and 8 KiB of JSON-encoded output string data inside the
existing 16 KiB complete-envelope ceiling. UTF-8 is never split. C0/C1 controls
other than tab and newline are replaced and reported through `sanitized`;
source or envelope clipping sets `truncated`. If scrollback eviction or erasure
retired the region, both strings are empty and `content_available` is false,
even if stale coordinates now refer to unrelated cells.

This event can contain passwords, tokens, pasted commands, build logs, and
other sensitive terminal data. The grant is deliberately separate from
`workspace-query`, `event-subscribe`, and `ui-actions`; an ungranted or API-v3
handler receives no command event. The event does not contain raw PTY bytes,
keystrokes before command completion, arbitrary scrollback, selection,
transcripts, environment variables, clipboard data, or filesystem contents.
It provides no write, rerun, or terminal-control action. Optional coarse
workspace context still requires its independent grant.

## Manifest API v5 persistent handlers

API v5 preserves all earlier events, disclosures, actions, and permission
gates. It adds only an explicit handler lifetime and framing choice:

```toml
api_version = 5
id = "persistent-counter"
name = "Persistent Counter"
version = "1.0.0"
permissions = ["process-spawn", "event-subscribe", "ui-actions"]

[event_handler]
program = "python3"
args = ["-u", "-c", "..."]
mode = "persistent"
events = ["app-ready", "tab-created", "tab-switched"]
```

Omitting `mode` retains `process-per-event`; API v1 through v4 therefore keep
their exact launch behavior. `persistent` is rejected before installation
unless `api_version` is 5 or newer.

Termeric starts at most one persistent process for a plugin in each application
window and plugin-registry generation. Requests and responses are compact JSON,
one document per newline. Termeric upgrades the existing event envelope to
schema 2 and injects a nonzero `request_id`:

```json
{"schema_version":2,"request_id":42,"plugin_id":"persistent-counter","event":{"type":"tab-created","tab_id":1}}
```

The handler must emit exactly one newline-terminated response before accepting
the next request, echoing both schema 2 and that exact ID:

```json
{"schema_version":2,"request_id":42,"actions":[{"type":"show-message","message":"Persistent event #2"}]}
```

Requests remain within the 16 KiB complete-envelope ceiling, including the
injected correlation fields. Responses retain the 64 KiB line, eight-action,
permission, and text limits. One worker serializes all event requests through
the existing 32-entry queue; no handler can have concurrent requests. A window
may own at most eight persistent plugin processes. Stderr is drained for the
entire lifetime so the child cannot block, while only the first 4 KiB is kept
for a bounded diagnostic.

Every response has a two-second deadline. Launch failure, exit, timeout,
malformed or unterminated JSON, an oversized line, wrong schema/request ID, or
a handler descriptor change kills and forgets that process. A later event may
start a fresh copy. Registry reload, plugin disable/removal, window shutdown,
or application shutdown kills the complete process group and rejects stale
generation results. The process receives the same cleared environment and
minimal variables as process-per-event handlers, with
`TERMERIC_PLUGIN_EVENT_SCHEMA=2`.

API v5 persistent mode is state retention and launch-amortization, not
additional authority. It adds no permission, event, terminal query, mutation,
raw-output, network, filesystem, render, or secret API. There are no
unsolicited messages or background actions; output is consumed only as the
correlated response to a Termeric request.

## Manifest API v6 live workspace query

API v6 preserves the API v5 process and framing contract. A persistent handler
that both requests and receives `workspace-query` may return one additional
action from an ordinary subscribed-event response:

```bash
termeric plugin install examples/plugins/workspace-inspector \
  --allow process-spawn --allow workspace-query \
  --allow event-subscribe --allow ui-actions --enable
```

```json
{"schema_version":2,"request_id":42,"actions":[{"type":"query-workspace","query_id":"current-window"}]}
```

`query_id` is a plugin-owned, control-free UTF-8 identifier of at most 64 bytes.
It need not match the transport `request_id`; Termeric echoes both so the plugin
can correlate its logical query and the exact protocol exchange. The query
action does not require `ui-actions` and may coexist with informational UI
actions. It is rejected unless all of these are true:

- the manifest uses API v6;
- the handler uses `mode = "persistent"`;
- `workspace-query` was requested and explicitly granted; and
- the response contains at most one workspace query.

After validating the response and confirming that the same plugin generation
is still loaded in the exact originating window, Termeric captures current
state and queues a schema-v2 `workspace-query-result` request through the same
plugin's persistent handler. It reuses the same process while healthy and
follows the documented restart behavior if that process exited:

```json
{
  "schema_version": 2,
  "request_id": 43,
  "plugin_id": "workspace-inspector",
  "event": {
    "type": "workspace-query-result",
    "query_id": "current-window",
    "snapshot_schema_version": 1,
    "active_working_directory": "/home/user/project",
    "active_working_directory_truncated": false,
    "tab_count": 3,
    "active_tab_index": 1,
    "terminal": { "columns": 120, "rows": 36 },
    "window": {
      "focused": true,
      "visible": true,
      "quick_access": false,
      "workspace_id": "project",
      "pixel_width": 1200,
      "pixel_height": 800,
      "tabs": [],
      "truncated": false
    }
  }
}
```

The `tabs` inventory contains stable current-window tab IDs, sanitized titles,
typed `shell`/`profile`/`plugin`/`live_domain` kinds, active/pinned/activity/
closed state, optional domain ID, pane inventory, divider topology, and
per-tab truncation. Panes contain stable tab-local IDs, sanitized optional
titles, focused/closed state, and cell dimensions. Dividers contain a stable
topology-local ID, local/live-domain kind, horizontal/vertical direction, and
ratio in thousandths.

The query exposes at most 16 tabs, 32 panes, 31 dividers, 2 KiB of aggregate
inventory metadata, and 1 KiB of active-cwd text inside the existing 16 KiB
persistent request ceiling. Control characters are removed, UTF-8 is never
split, and every affected scope reports truncation. `tab_count` and
`active_tab_index` describe the full current window even when the detailed
inventory is clipped. The result has no synthetic window ID because every ID
inside it is scoped to the exact originating window.

Results are asynchronous. Previously queued subscribed events may reach the
serialized handler before the query result, so authors must use `query_id`
rather than assuming adjacency. The response to a query result may contain the
ordinary bounded UI actions and, under API v7's separate grant, one workspace
action, but its query authority is cleared: results cannot chain another query.
This prevents self-sustaining query loops while allowing one fresh snapshot in
response to each external Termeric event.

The snapshot never contains terminal cells, scrollback, PTY bytes, command
regions, selections, environment variables, clipboard data, filesystem
contents, secrets, process-global inventories, or other windows. API v6 itself
adds no mutation action and does not broaden `terminal-text`.

## Manifest API v7 workspace actions

API v7 preserves all earlier contracts and introduces one new capability:
`workspace-actions`. A plugin must request and receive this grant separately
from both `workspace-query` and `ui-actions`, and must use a persistent handler:

```bash
termeric plugin install examples/plugins/workspace-navigator \
  --allow process-spawn --allow workspace-query \
  --allow workspace-actions --allow event-subscribe \
  --allow ui-actions --enable
```

A granted response may contain one of these typed operations:

```json
{"type":"select-tab","action_id":"select-results","tab_id":12}
{"type":"focus-pane","action_id":"focus-shell","tab_id":12,"pane_id":3}
```

`action_id` is plugin-owned, control-free UTF-8 of at most 64 bytes and is
echoed in the result. `tab_id` and `pane_id` are the stable, current-window IDs
returned by the API v6 inventory; they are not tab indices. Termeric validates
the entire target before changing either tab or pane focus, so a missing,
closing, or closed target fails without partially retargeting the window.

At most one host operation—one query or one workspace action—is accepted in a
single response. Informational `show-message` or `show-notice` actions may
coexist when separately granted. Workspace actions are rejected unless the
manifest uses API v7, the handler is persistent, and `workspace-actions` was
both requested and granted. Process-per-event handlers and API v1 through v6
cannot acquire this authority.

Termeric applies the operation only after confirming that the originating
plugin generation is still loaded in the exact current window, then sends a
correlated result through that plugin's persistent handler:

```json
{
  "schema_version": 2,
  "request_id": 44,
  "plugin_id": "workspace-navigator",
  "event": {
    "type": "workspace-action-result",
    "action_id": "select-results",
    "action": "select-tab",
    "ok": true,
    "tab_id": 12,
    "changed": true,
    "error": null
  }
}
```

Successful `focus-pane` results instead contain `pane_id`, `tab_changed`, and
`pane_changed`. A validation failure is still a normal correlated result with
`ok: false`, a bounded control-safe `error`, null change fields, and an
`error_truncated` flag. This lets a handler react without parsing a toast or log.

A query result retains workspace-action and API-v8 topology authority so the
bounded useful flow is `external event → query → one mutation → result`. A
workspace-action result clears every query/navigation/topology authority; its
response may contain only separately granted informational UI actions.
Plugin-origin navigation also suppresses the
derived `tab-switched` subscription event. Together these rules prevent a
handler from manufacturing a navigation loop, while ordinary user and
authenticated-control tab switches continue to emit the lifecycle event.
Subscribed events already queued for the serialized handler may interleave, so
authors must correlate with `action_id` and transport `request_id` rather than
assuming adjacent messages.

API v7 itself does not add tab/pane creation, closure, movement, resize,
terminal input, terminal text access, process-global navigation, cross-window
targeting, or arbitrary commands.

## Manifest API v8 non-destructive topology actions

API v8 preserves all earlier contracts and adds `workspace-topology`, separate
from query, navigation, UI, and terminal-text authority. It requires an API-v8
persistent handler:

```bash
termeric plugin install examples/plugins/workspace-organizer \
  --allow process-spawn --allow workspace-query \
  --allow workspace-topology --allow event-subscribe \
  --allow ui-actions --enable
```

The grant permits exactly two typed, current-window operations:

```json
{"type":"move-tab","action_id":"order-build","tab_id":12,"destination_index":2}
{"type":"resize-divider","action_id":"widen-shell","tab_id":12,"divider_id":4,"ratio_millis":650}
```

`move-tab` uses a stable local tab ID plus a final zero-based visual index. The
index must be below the protocol's 256-tab ceiling and the window's current tab
count. Termeric rejects missing, closing, live-domain, or pinned-boundary
crossing targets before changing order. Reordering preserves the exact active
tab rather than selecting whatever lands at its prior index.

`resize-divider` uses stable tab-local divider identity from the v6 inventory.
Ratios are integer thousandths from 100 through 900. The operation rejects
missing or closing tabs, unsplit or zoomed layouts, missing dividers, and
live-domain topology rather than guessing at a sibling or optimistically
acknowledging daemon state. Successful local changes recalculate pane geometry
and request a redraw without changing tab or pane focus.

Both operations return the existing correlated `workspace-action-result` event.
A tab result contains `destination_index` and `changed`; a divider result
contains `divider_id`, `previous_ratio_millis`, `ratio_millis`, and `changed`.
An invalid current target is a bounded `ok: false` result with null result
fields and a control-safe error. Static protocol violations such as a 99‰ ratio
or an index at/above the 256-tab ceiling reject the handler response itself.

At most one query, navigation action, or topology action is accepted per
response. A query result may perform one separately granted topology action;
the resulting event clears query, navigation, and topology authority. Neither
operation emits a derived tab lifecycle hook, so response-driven topology
changes cannot manufacture an event loop. Already queued subscribed events may
still interleave, and authors must correlate logical and transport IDs.

API v8 does not permit tab/pane creation or closure, pane movement/splitting,
cross-window or process-global topology, live-domain mutation, terminal input,
terminal content, or arbitrary commands. `workspace-topology` can be granted
without `workspace-query` when a plugin already has a current stable ID, but
Termeric does not make stale IDs succeed.

## Manifest API v9 handler commands

API v9 preserves every earlier permission and response boundary while allowing
an event handler to add ordinary command-palette entries. It adds no permission
and no host action: commands require the existing `process-spawn` and
`event-subscribe` grants, an `[event_handler]`, and an explicit subscription to
`command-invoked`.

```bash
termeric plugin install examples/plugins/command-tool \
  --allow process-spawn --allow workspace-query \
  --allow event-subscribe --allow ui-actions --enable
```

```toml
api_version = 9
id = "command-tool"
name = "Command Tool"
version = "1.0.0"
permissions = ["process-spawn", "event-subscribe", "ui-actions"]

[[commands]]
id = "show-status"
name = "Show Plugin Status"
description = "Show status computed by the plugin handler"

[event_handler]
program = "command-tool-handler"
args = ["--stdio"]
mode = "persistent"
events = ["command-invoked"]
```

Each local ID becomes `<plugin-id>.<command-id>`; the example contributes
`command-tool.show-status` under the plugin's palette category. IDs use the
same lowercase identifier grammar as profiles. Names are capped at 80 bytes,
descriptions at 512 bytes, and a manifest may declare at most 32 handler
commands. Together with the existing 32-profile ceiling, a plugin can never
exceed the manager's 64-command boundary. Duplicate local IDs and collisions
with generated `open-<profile-id>` commands fail validation.

Selecting the entry sends only its owning plugin a normal bounded handler
request. No other plugin observes the invocation:

```json
{
  "schema_version": 1,
  "plugin_id": "command-tool",
  "event": {
    "type": "command-invoked",
    "command_id": "command-tool.show-status"
  }
}
```

The optional coarse `context` member follows the existing `workspace-query`
grant. Responses use the handler's existing schema and permissions; API v9 does
not implicitly grant UI, workspace query, navigation, topology, terminal text,
or any other authority. Both process-per-event and persistent handlers are
valid. Undeclared IDs, another plugin's namespace, missing subscriptions, API
v8 or older manifests, and namespace/profile conflicts fail closed. Direct
diff, context, export, and profile-selection commands are reserved to the
compiled-in `claude-code` and `codex` integrations even if a third-party command
uses the same local suffix.

## Manifest API v10 typed settings

API v10 preserves all earlier permissions and adds a bounded non-secret
configuration schema. Settings do not grant capabilities and cannot read host
state; they are values the user explicitly supplies to the already reviewed
profile or handler executable.

```toml
api_version = 10
id = "configurable-command"
name = "Configurable Command"
version = "1.0.0"
permissions = ["process-spawn", "event-subscribe", "ui-actions"]

[[settings]]
type = "boolean"
id = "include-command-id"
name = "Include command ID"
default = false

[[settings]]
type = "integer"
id = "line-count"
name = "Line count"
default = 10
min = 1
max = 100

[[settings]]
type = "text"
id = "greeting"
name = "Greeting"
default = "Hello from Termeric"

[[settings]]
type = "choice"
id = "audience"
name = "Audience"
default = "workspace"
choices = ["workspace", "terminal"]
```

A manifest may declare at most 32 unique setting IDs. Names are capped at 80
bytes, descriptions at 512 bytes, text values at 512 bytes, and choice lists at
32 unique non-empty values of at most 80 bytes each. Text is single-line and
NUL-free. Integer manifests require `min <= default <= max`; configured values
must remain inside that inclusive range. Effective values, including defaults,
must fit one 8 KiB compact JSON object. Unknown types/fields, duplicate IDs,
wrong value types, unknown choices, oversized values, and API v9 or older
manifests fail closed.

Configure one override per atomic command:

```bash
termeric plugin configure configurable-command --set greeting "Hello team"
termeric plugin configure configurable-command --set audience terminal
termeric plugin configure configurable-command --set include-command-id true
termeric plugin configure configurable-command --unset greeting
termeric plugin configure configurable-command --reset-settings
```

The same four setting types can be edited under Settings → Plugins → select the
plugin → Configure settings. Boolean and choice values use bounded option lists;
integer and text values use a visible single-line editor. `Ctrl+D` restores the
selected setting's manifest default while an input is active.

`--unset` restores one manifest default; `--reset-settings` restores every
default. The registry stores only explicit overrides. `plugin show ID` and its
JSON form expose effective values deliberately, so this is not a secret store:
tokens, passwords, credentials, and other sensitive values must not be placed in
plugin settings. Registry type tampering makes the plugin fail discovery rather
than coercing a value.

API v10 profiles receive a second compact environment variable independent of
the optional workspace context:

```json
{
  "schema_version": 1,
  "plugin_id": "configurable-command",
  "values": {
    "audience": "terminal",
    "greeting": "Hello team",
    "include-command-id": true
  }
}
```

This document is passed as `TERMERIC_PLUGIN_SETTINGS_JSON`. Handler requests
receive the same effective map as a top-level `settings` member. No setting is
interpolated into argv, a shell command, paths, permissions, or environment
variable names. Existing structured framing and request bounds still apply.

Update and `--replace` preserve compatible explicit overrides. If a new
manifest removes a configured key, changes its type, narrows its range, removes
its selected choice, or otherwise makes the stored value invalid, replacement
fails before changing the installed manifest or registry. The user can unset or
reset the old overrides and retry; Termeric never silently drops or guesses a
migration.

## Manifest API v11 presented-terminal query

API v11 preserves all earlier capabilities and adds the independent
`terminal-query` permission. Only a persistent event handler may use it. A
typical user-invoked command first requests the API v6 workspace inventory,
selects the stable IDs of its active tab and focused pane, and then returns one
query action:

```json
{
  "schema_version": 2,
  "request_id": 42,
  "actions": [{
    "type": "query-terminal",
    "query_id": "active-pane-text",
    "tab_id": 12,
    "pane_id": 3,
    "history_lines": 20,
    "max_bytes": 2048
  }]
}
```

The manifest must use API v11, request `terminal-query`, use
`mode = "persistent"`, and receive an explicit `--allow terminal-query` grant.
`query_id` is a control-safe identifier of at most 64 bytes. `history_lines`
may be 0 through 256 and `max_bytes` may be 1 through 8,192. The tab and pane
IDs are scoped to the exact originating window; missing, closing, or stale
targets return an error for those IDs and are never retargeted to whichever
pane is active later.

Termeric responds through the same correlated persistent process:

```json
{
  "schema_version": 2,
  "request_id": 43,
  "plugin_id": "terminal-reader",
  "event": {
    "type": "terminal-query-result",
    "query_id": "active-pane-text",
    "text_schema_version": 1,
    "ok": true,
    "tab_id": 12,
    "pane_id": 3,
    "screen": "primary",
    "requested_history_lines": 20,
    "requested_max_bytes": 2048,
    "history_lines": 20,
    "text_bytes": 1731,
    "text": "presented terminal text",
    "truncated": false,
    "error": null
  }
}
```

The primary screen may include only the requested bounded primary history. An
active alternate screen is viewport-only and never crosses into primary
history. Text comes from the exact presentation state: output hidden inside an
unfinished synchronized-output batch is not disclosed. The result is UTF-8,
reports its actual included history and text-byte count, and marks clipping.
If JSON escaping would exceed the persistent request ceiling, Termeric reduces
the text budget and reports `truncated = true`; the complete source event still
fits below 16 KiB.

One response may contain at most one host operation across workspace query,
terminal query, navigation, topology, pane/tab creation, and pane/tab closure,
though bounded UI notices may coexist. A workspace-query result retains
separately granted terminal-query, pane-create, tab-create, pane-close, and
tab-close authority so one external command can discover current state and
then perform one granted operation.
After a terminal-query result, all query and workspace-mutation authority is
cleared. A later external subscribed event may start another independently
bounded request. The API does not expose cell styles, images, selection,
clipboard, environment, raw PTY bytes, live output subscriptions, terminal
input, other windows, or process-global terminal discovery.

## Manifest API v12 ordinary-pane creation

API v12 preserves every earlier contract and adds the independent
`pane-create` permission. Only a persistent handler may use it. Install the
sample only after reviewing its ordinary Python process and both host-data and
pane-creation grants:

```bash
termeric plugin validate examples/plugins/pane-builder
termeric plugin install examples/plugins/pane-builder \
  --allow process-spawn --allow workspace-query \
  --allow pane-create --allow event-subscribe --allow ui-actions --enable
```

The sample contributes **Pane Builder: Split Active Pane** to the command
palette. It first uses the existing bounded current-window inventory to obtain
stable IDs, then returns exactly one operation:

```json
{
  "schema_version": 2,
  "request_id": 12,
  "actions": [{
    "type": "create-pane",
    "action_id": "split-active-pane",
    "tab_id": 4,
    "pane_id": 1,
    "direction": "vertical"
  }]
}
```

`direction` is exactly `horizontal` (panes stacked vertically) or `vertical`
(panes side by side). IDs are resolved when the operation executes. The target
must be a live, non-closing ordinary shell/profile/plugin pane in the
originating window. Missing IDs, serial tabs, live-domain tabs, the per-tab pane
limit, an unavailable renderer for a first split, and PTY spawn failures return
bounded errors without retargeting another pane.

Success returns the new stable pane ID; failure returns it as `null`:

```json
{
  "schema_version": 1,
  "plugin_id": "pane-builder",
  "event": {
    "type": "workspace-action-result",
    "action_id": "split-active-pane",
    "action": "create-pane",
    "ok": true,
    "tab_id": 4,
    "pane_id": 1,
    "direction": "vertical",
    "created_pane_id": 2,
    "error": null
  }
}
```

The pane inherits the target tab's frozen split launch and configured cwd
policy. No executable, argv, environment, cwd, title, or input field exists in
this action. Creation counts as the response's sole host operation, and its
result clears every host-operation authority. A later external event may begin
a new bounded request. Tab
creation requires the separate API v13 grant and pane closure requires the
separate API v14 grant. Tab closure requires the separate API v15 grant;
arbitrary commands, live-domain mutation, and cross-window placement remain
unsupported powers.

## Manifest API v13 ordinary-tab creation

API v13 preserves every earlier contract and adds the independent `tab-create`
permission. Only a persistent handler may use it. Install the sample only after
reviewing its ordinary Python process and tab-creation grant:

```bash
termeric plugin validate examples/plugins/tab-builder
termeric plugin install examples/plugins/tab-builder \
  --allow process-spawn --allow tab-create \
  --allow event-subscribe --allow ui-actions --enable
```

The sample contributes **Tab Builder: New Task Tab** to the command palette and
returns exactly one operation:

```json
{
  "schema_version": 2,
  "request_id": 13,
  "actions": [{
    "type": "create-tab",
    "action_id": "new-task-tab"
  }]
}
```

The action always targets the handler's originating native window and creates
the same ordinary local shell tab as the in-app New Tab action. It accepts no
window ID, executable, argv, environment, cwd, title, profile, domain, or input.
The new tab becomes active and uses the window's current validated default
shell configuration. If the tab manager is unavailable, the active tab belongs
to a live domain, the per-window limit is reached, or PTY launch fails, creation
returns a bounded error without a fabricated identity.

Success returns the new stable window-local tab ID; failure returns it as
`null`:

```json
{
  "schema_version": 1,
  "plugin_id": "tab-builder",
  "event": {
    "type": "workspace-action-result",
    "action_id": "new-task-tab",
    "action": "create-tab",
    "ok": true,
    "created_tab_id": 5,
    "error": null
  }
}
```

Creation counts as the response's sole host operation. Its result clears query,
navigation, topology, terminal-query, pane-create, tab-create, pane-close, and
tab-close authority.
Termeric deliberately suppresses the derived `tab-created` hook for this
plugin-originated action, so neither the requester nor another plugin can turn
one creation into a lifecycle feedback loop. A later independent external
event may begin another bounded request. Arbitrary launch selection,
live-domain creation, and cross-window placement remain unsupported. Closure
requires the separate API v15 grant.

## Manifest API v14 ordinary-pane closure

API v14 preserves every earlier contract and adds the independent `pane-close`
permission. Only a persistent handler may use it. Because closure destroys a
running pane and its process, review both the executable and this grant before
installing the sample:

```bash
termeric plugin validate examples/plugins/pane-pruner
termeric plugin install examples/plugins/pane-pruner \
  --allow process-spawn --allow workspace-query \
  --allow pane-close --allow event-subscribe --allow ui-actions --enable
```

The sample contributes **Pane Pruner: Close Sibling Pane** to the command
palette. It queries the bounded current-window inventory, selects one
non-focused sibling in the active ordinary tab, and returns one exact action:

```json
{
  "schema_version": 2,
  "request_id": 14,
  "actions": [{
    "type": "close-pane",
    "action_id": "close-sibling-pane",
    "tab_id": 4,
    "pane_id": 1
  }]
}
```

The stable IDs are resolved again when the action executes. Termeric refuses a
missing or already closed pane, a closing tab, a live-domain or serial tab, an
unsplit tab, and any operation that would remove the tab's last pane. It does
not implicitly focus or select the target. Closing a non-active pane therefore
leaves the active tab unchanged. If one pane remains after a successful close,
the split manager collapses back to the ordinary single-terminal layout.

Success reports the remaining count and resulting focused pane; failure keeps
those fields `null` and returns a bounded, sanitized error:

```json
{
  "schema_version": 1,
  "plugin_id": "pane-pruner",
  "event": {
    "type": "workspace-action-result",
    "action_id": "close-sibling-pane",
    "action": "close-pane",
    "ok": true,
    "tab_id": 4,
    "pane_id": 1,
    "remaining_panes": 1,
    "focused_pane_id": 2,
    "error": null
  }
}
```

Pane closure is the response's sole host operation. Its result clears query,
navigation, topology, terminal-query, pane-create, tab-create, pane-close, and
tab-close authority. A later external event may begin another bounded
request. API v14 does not itself grant tab closure, live-domain mutation,
terminal input, cross-window targeting, arbitrary process signaling, or a
force/confirmation bypass. Tab closure requires the separate API v15 grant.

## Manifest API v15 ordinary-tab closure

API v15 preserves every earlier contract and adds the independent `tab-close`
permission. Only a persistent handler may use it. Tab closure terminates the
tab's PTY/process tree after the normal close animation, so review both the
executable and this grant before installing the sample:

```bash
termeric plugin validate examples/plugins/tab-pruner
termeric plugin install examples/plugins/tab-pruner \
  --allow process-spawn --allow workspace-query \
  --allow tab-close --allow event-subscribe --allow ui-actions --enable
```

The sample contributes **Tab Pruner: Close Background Tab** to the command
palette. It queries the bounded current-window inventory, selects one visible
non-active non-live-domain tab, and returns one exact action:

```json
{
  "schema_version": 2,
  "request_id": 15,
  "actions": [{
    "type": "close-tab",
    "action_id": "close-background-tab",
    "tab_id": 7
  }]
}
```

The stable ID is resolved again when the action executes. Shell, profile,
plugin, pinned, and serial tabs are eligible because they are process-local.
Termeric refuses a missing, closed, already-closing, or live-domain target. It
also counts only non-closing tabs and refuses the final survivor, even if other
tab records are still completing their animations. This grant cannot close a
window implicitly.

Closing a background tab preserves the current selection. Closing the active
tab uses the ordinary tab manager's deterministic next-tab selection. Any copy
mode owned by that tab is dismissed and focused terminal surfaces are rebound.
Success means the normal animated close was accepted and scheduled; it does
not claim that autosave, animation, PTY shutdown, or physical removal has
already completed:

```json
{
  "schema_version": 1,
  "plugin_id": "tab-pruner",
  "event": {
    "type": "workspace-action-result",
    "action_id": "close-background-tab",
    "action": "close-tab",
    "ok": true,
    "tab_id": 7,
    "scheduled": true,
    "remaining_tabs": 2,
    "active_tab_id": 9,
    "active_changed": false,
    "error": null
  }
}
```

Failures return `scheduled: false`, null remaining/active fields, and a bounded
sanitized error. Tab closure is the response's sole host operation, and its
result clears query, navigation, topology, terminal-query, creation, and both
closure authorities. Termeric suppresses the derived `tab-closed` hook for a
plugin-originated close, making the correlated result the only follow-up and
preventing lifecycle-driven close loops. A later independent external event
may begin another bounded request. API v15 does not grant final-tab/window
closure, live-domain mutation, cross-window targeting, force-kill/signals,
terminal input, or a confirmation bypass.

## Permissions and isolation

| Capability | Public API status |
|---|---|
| `process-spawn` | Supported only after `--allow process-spawn`; required by declarative launch profiles and API v3+ handlers. |
| `workspace-query` | API v2+, after `--allow workspace-query`; exposes the schema-v1 bounded launch snapshot and the same coarse optional event context. API v6 persistent handlers may additionally request the bounded current-window inventory described above. |
| `workspace-actions` | API v7+, after `--allow workspace-actions`; permits only current-window stable-ID `select-tab` and `focus-pane` through a persistent handler, with correlated results and loop prevention. |
| `workspace-topology` | API v8+, after `--allow workspace-topology`; permits only local current-window stable-ID `move-tab` and `resize-divider`, with atomic validation and correlated results. |
| `event-subscribe` | API v3+, after `--allow event-subscribe`; enables the four coarse lifecycle events, plus API v4 `command-completed` when its separate text grant is present and API v9 exact-owner `command-invoked` for declared commands. |
| `ui-actions` | API v3+, after `--allow ui-actions` and with `event-subscribe`; accepts only bounded `show-message` and `show-notice` responses. |
| `filesystem-read`, `filesystem-write` | Reserved and denied to the plugin API. |
| `network` | Reserved and denied to the plugin API. |
| `clipboard` | Reserved and denied. |
| `terminal-text` | API v4+, after `--allow terminal-text`; discloses bounded authoritative text from subscribed completed OSC 133 regions, never arbitrary grid/selection/transcript/raw PTY data. |
| `terminal-query` | API v11+, after `--allow terminal-query`; permits one exact current-window stable-ID query of at most 256 history lines and 8 KiB of presented text through a persistent handler, with correlated results and host-operation loop prevention. |
| `pane-create` | API v12+, after `--allow pane-create`; permits one exact current-window ordinary-pane split through a persistent handler, with inherited launch/cwd policy, a correlated stable-ID result, and host-operation loop prevention. |
| `tab-create` | API v13+, after `--allow tab-create`; permits one ordinary local shell tab in the originating window through a persistent handler, with a correlated stable-ID result and lifecycle/host-operation loop prevention. |
| `pane-close` | API v14+, after `--allow pane-close`; permits closure of one exact ordinary current-window pane through a persistent handler, refuses unsupported or last-pane targets, and returns a correlated bounded result. |
| `tab-close` | API v15+, after `--allow tab-close`; permits scheduling closure of one exact ordinary local current-window tab through a persistent handler, protects the final non-closing tab, and returns correlated selection state. |
| `secrets` | Reserved and denied; there is no secret API or manifest interpolation. |

Requested and granted sets are independent. Installation may retain a disabled
manifest with denied capabilities for inspection, but enabling fails until all
requests are explicitly granted. API v1 refuses every capability except
`process-spawn`; API v2 refuses every capability except `process-spawn` and
`workspace-query`; API v3 additionally permits `event-subscribe` and
`ui-actions`; API v4 through v15 may also request `terminal-text` for
`command-completed`. API v5 and v6 add no new grant; API v6 gives the existing
`workspace-query` grant a separately gated live-query operation. API v7 adds
only `workspace-actions`, independently of query, UI, or terminal-text grants.
API v8 similarly adds only the independent `workspace-topology` grant.
API v9 adds no grant; handler commands reuse `event-subscribe` and whatever
separate response/disclosure permissions the manifest already requests.
API v10 also adds no grant; typed settings are explicit user-authored input and
are always visible through plugin diagnostics.
API v11 adds only `terminal-query`, independently of `terminal-text`, workspace
mutation, UI, filesystem, network, clipboard, or secret access.
API v12 adds only `pane-create`, independently of query, navigation, topology,
terminal text, UI, filesystem, network, clipboard, or secret access.
API v13 adds only `tab-create`, independently of pane creation, query,
navigation, topology, terminal text, UI, filesystem, network, clipboard, or
secret access.
API v14 adds only `pane-close`, independently of pane/tab creation, query,
navigation, topology, terminal text, UI, filesystem, network, clipboard, or
secret access.
API v15 adds only `tab-close`, independently of pane/tab creation, pane close,
query, navigation, topology, terminal text, UI, filesystem, network, clipboard,
or secret access.
Updates that add a permission do not silently migrate the
old grant or replace the working installed copy, even while the plugin is
disabled; the operator must use the explicit reviewed reinstall/replace flow.

`process-spawn` is intentionally a powerful grant: the selected executable is
a normal user process and may have the ambient filesystem, environment, and
network access that the operating system gives it. Termeric passes argv as
separate values and does not invoke a shell unless the manifest explicitly
selects one, but API permissions are not an OS sandbox. Do not grant launch
permission to an untrusted manifest or executable.

Profile hooks perform only bounded in-memory lookup and action construction. A
selected profile executable is invoked exactly once as the ordinary PTY child;
Termeric does not synchronously probe it with `--version` or another
side-effecting command. API v3+ handler programs run only on the bounded worker
described above, never in Termeric's UI/PTY/render threads. A handler executable
is still ordinary native code with the user's OS authority despite its cleared
environment; the API permission model is disclosure/action gating, not a kernel
sandbox. WASM/in-process modules, network hooks supplied by Termeric,
process-global terminal queries, raw terminal-output subscriptions, terminal
input, and render actions remain unsupported. API v5+'s persistent newline protocol is deliberately narrower
than a bidirectional JSON-RPC service.

## Author testing and upgrades

Before distributing a local directory:

1. Run `plugin validate` and inspect the source/fingerprint/permission output.
2. Install disabled first, then inspect `plugin show ID --json`.
3. Enable only with the exact requested grant and invoke every palette profile.
4. Test missing executables, immediate child failure, a long-running child,
   Unicode argv, active/inherited cwd, workspace save/restore, disable, and
   remove.
5. Change the version and profile set in the source, run `plugin update`, and
   verify configured/default and archived named profiles. Removed configured
   profiles are cleared; removed archived profiles fail visibly at restore.
6. Add a reserved capability in a fixture and prove the update/enable is denied.
7. For API v3+, feed every subscribed event to the handler over stdin; test an
   empty response, every allowed UI action, malformed/oversized output, nonzero
   exit, timeout, queue pressure, reload while pending, and operation without
   `workspace-query` or `ui-actions`.
8. For API v4, test successful and failed multiline commands, split-pane source
   IDs, long/control-bearing text, evicted regions, and denial without the exact
   `terminal-text` grant. Treat captured command/output as sensitive data.
9. For API v5, send multiple requests through one process and test retained
   state, exact ID echo, crash and restart, timeout, oversized/unterminated
   lines, stderr flooding, queue/process ceilings, reload/disable teardown, and
   behavior when `mode` is omitted.
10. For API v6, test granted and denied `query-workspace` responses, logical and
    transport correlation, asynchronous interleaving, topology and metadata
    clipping, control-bearing titles/IDs, stale generation refusal, and the
    no-query-from-result rule. Treat cwd, titles, workspace IDs, and domain IDs
    as potentially sensitive metadata.
11. For API v7, test granted and denied `select-tab`/`focus-pane`, invalid and
    closing stable IDs, query-to-action correlation, one-operation rejection,
    action-result authority removal, lifecycle-hook suppression, stale plugin
    generations, and interleaved subscribed events.
12. For API v8, test granted and denied `move-tab`/`resize-divider`, pinned and
    live-domain refusal, invalid/stale IDs, index and ratio bounds, idempotence,
    active-focus preservation, one-operation rejection, result authority
    removal, and interleaved subscribed events.
13. For API v9, test palette discovery, exact namespace routing, duplicate and
    profile-generated command conflicts, undeclared IDs, both handler modes,
    missing subscriptions/grants, live reload, and local IDs matching a
    compiled-in command suffix without invoking the built-in action.
14. For API v10, test every setting type/default/bound, set/unset/reset,
    effective-value diagnostics, profile and handler delivery, live reload,
    registry type tampering, compatible update preservation, and refusal of
    removed or narrowed configured values.
15. For API v11, test exact grant/version/mode denial, workspace-to-terminal
    correlation, invalid/closing stable IDs, primary and alternate screens,
    hidden synchronized output, UTF-8/JSON clipping, history/byte limits,
    one-host-operation rejection, and complete authority removal after the
    result. Treat all returned terminal text as potentially sensitive.
16. For API v12, test exact grant/version/mode denial, typed split direction,
    stale and unsupported stable-ID targets, workspace-to-create correlation,
    one-host-operation rejection, returned pane identity, inherited launch/cwd
    policy, and complete authority removal after the result.
17. For API v13, test exact grant/version/mode denial, action-ID and schema
    bounds, current-window stable identity, unavailable/live/capacity/launch
    failures, lifecycle-hook suppression, one-host-operation rejection, and
    complete authority removal after the result.
18. For API v14, test exact grant/version/mode denial, strict action shape,
    stale/closed/closing/unsupported stable-ID targets, non-active target focus
    preservation, split collapse and last-pane refusal, one-host-operation
    rejection, correlated result fields, and complete authority removal.
19. For API v15, test exact grant/version/mode denial, strict action shape,
    stale/closed/closing/live-domain stable-ID refusal, local serial acceptance,
    background/active selection behavior, final-non-closing-tab protection,
    scheduled result fields, copy-mode cleanup, lifecycle-hook suppression,
    one-host-operation rejection, and complete authority removal.

Termeric's own tests cover schema/API mismatch, permission denial, forbidden
capabilities, fingerprint and symlink substitution, administrative temporary
file safety, resource ceilings, child crash/timeout isolation, profile/command
conflicts, explicit upgrades, lifecycle operations, and exact workspace/command
actions. API v3 adds hermetic local-process stdin/stdout, response-schema,
disclosure, action-grant, and bounds coverage. API v4 adds cross-layer semantic
region routing, source IDs, permission denial, stale-content refusal, UTF-8 and
JSON bounds, control sanitization, and a checked-in command observer. API v5
adds stateful single-process reuse, correlation, restart, cancellation, framing,
and process-count tests plus a checked-in persistent counter. API v6 adds
independent permission denial, non-chainable query responses, bounded current-
window snapshot, control sanitization, same-process round-trip, and checked-in
workspace inspector coverage. API v7 adds exact permission/version/mode denial,
query/action/result authority reduction, stable-ID success/failure routing,
non-retargeting invalid targets, hook-loop suppression, same-process reuse, and
a checked-in non-invasive workspace navigator. None of these tests launches a
provider process. API v8 adds exact topology grant/version/mode denial, static
index/ratio bounds, cross-kind one-operation enforcement, atomic reorder/resize
success and failure routing, focus preservation, same-process correlation, and
a checked-in non-invasive workspace organizer.
API v9 adds manifest/version/subscription/resource validation, generated-ID
conflicts, exact-owner manager routing, process-per-event and persistent
invocation construction, third-party reserved-suffix isolation, and a checked-
in command tool. No provider process is launched.
API v10 adds typed schema and aggregate bounds, atomic override commands,
default merging, profile/handler disclosure, status visibility, tamper refusal,
compatible preservation, incompatible-update rollback, and a checked-in
configurable command. These tests construct launch/invocation requests but do
not execute the sample or a provider.
API v11 adds exact permission/version/mode gating, response-schema and static
limit validation, stable-ID success/failure routing, presentation-safe hidden-
output exclusion, JSON-envelope clipping, same-process correlation, host-
operation loop prevention, and a checked-in terminal reader. The tests use only
in-memory terminals and inert local shell/Python fixtures; no provider runs.
API v12 adds exact permission/version/mode gating, typed direction parsing,
stable-ID creation and failure routing, ordinary/live/serial target
distinction, same-process correlation, host-operation loop prevention, and a
checked-in pane builder. Its PTYs run only inert local shell fixtures; no
provider runs.
API v13 adds exact permission/version/mode gating, strict action parsing,
ordinary stable-ID creation, unavailable/live/capacity/launch failure routing,
lifecycle-hook suppression, same-process correlation, authority removal, and a
checked-in tab builder. Its PTYs run only inert local shell fixtures; no
provider runs.
API v14 adds exact permission/version/mode gating, strict stable-ID action
parsing, ordinary/live/serial/closing/last-pane refusal, non-active targeting,
split collapse, bounded correlated results, same-process query-to-close
correlation, one-operation enforcement, authority removal, and a checked-in
pane pruner. Its PTYs run only inert local shell fixtures; no provider runs.
API v15 adds exact permission/version/mode gating, strict stable-ID action
parsing, local serial acceptance, missing/live/closing/final-tab refusal,
background focus preservation, active handoff, copy-mode cleanup, scheduled
correlated results, lifecycle-hook suppression, same-process query-to-close
correlation, one-operation enforcement, authority removal, and a checked-in
tab pruner. Its PTYs run only inert local shell fixtures; no provider runs.

Install also rejects a symlinked plugin root. Removal first atomically renames a
real installation into a same-root quarantine, then commits the registry
change; a registry-write failure restores the original directory. Cleanup
failure can therefore leave only an inactive, visibly named quarantine rather
than an unregistered plugin at its executable location.

## Built-in integration boundary

Claude Code and Codex remain compiled-in privileged integrations. They use
internal transcript, diff review/apply, clipboard, context collection,
approval, resume, and profile-selection types that the public APIs intentionally do
not expose. Their launch commands still pass through the same `PluginManager`,
palette namespace checks, `PluginAction::OpenTab`, PTY ownership, and workspace
restore dispatch as declarative plugins. Moving them entirely onto the public
layer would either remove those reviewed features or prematurely grant
third-party access to arbitrary terminal state, filesystem mutation, clipboard,
and secrets. API v15's current-window inventory/navigation/topology, exact
presented-text, separately granted ordinary pane/tab creation and closure,
persistent lifetime, namespaced handler commands, and non-secret settings, plus
the narrowly granted completed-region snapshot, are not
enough to represent those privileged workflows; broader migration waits for a
separately designed sandboxed API.

Built-in context collection is bounded local preprocessing, not an AI request:
it gathers selected Git, tree, terminal/test, and explicitly workspace-bounded
file inputs on a timeout-controlled worker, then writes the resulting bundle
only to the captured tab/pane terminal. Provider-side interpretation starts
after that PTY write and remains outside automated acceptance.

## Compatibility policy

`api_version = 1`, `api_version = 2`, `api_version = 3`, `api_version = 4`,
`api_version = 5`, `api_version = 6`, `api_version = 7`, `api_version = 8`,
`api_version = 9`, `api_version = 10`, `api_version = 11`,
`api_version = 12`, `api_version = 13`, `api_version = 14`, and
`api_version = 15`
are separately supported exact contracts; registry `schema_version = 1`
remains exact. API v1
preserves its process-only behavior, while API v2 adds the explicitly granted
structured context described above, API v3 adds the bounded lifecycle-handler
contract, API v4 adds the separately granted semantic command-completion event,
API v5 adds the opt-in correlated persistent lifetime, API v6 adds the
separately gated live current-window workspace inventory query, and API v7 adds
separately granted stable-ID tab selection and pane focus with correlated
results. API v8 adds independently granted local tab ordering and divider-ratio
adjustment with the same correlated-result and authority-reduction contract.
API v9 adds bounded exact-owner handler commands without adding a permission or
host-action surface.
API v10 adds bounded typed non-secret settings and explicit migration refusal,
again without adding a permission or host-action surface.
API v11 adds the separately granted, bounded exact-pane presented-terminal
query and terminal-result authority reduction described above.
API v12 adds separately granted exact ordinary-pane creation and returns the
new stable pane identity before removing every host-operation authority.
API v13 adds separately granted ordinary-tab creation in the originating
window, returns the new stable tab identity, and suppresses lifecycle feedback
before removing every host-operation authority.
API v14 adds separately granted exact ordinary-pane closure, refuses the final
pane and unsupported tab kinds, and returns the remaining/focused pane state
before removing every host-operation authority.
API v15 adds separately granted exact ordinary-tab closure scheduling, protects
the final non-closing tab, returns the remaining/active tab state, and
suppresses lifecycle feedback before removing every host-operation authority.
Any other API or registry value fails before registration. Additive optional
fields require a documented Termeric release;
removals or semantic changes require a new API version plus an explicit
migration tool. Termeric does not guess at forward compatibility because
permissions, argv, and disclosed context are security boundaries.
