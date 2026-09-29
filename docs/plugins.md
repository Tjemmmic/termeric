# Declarative third-party plugins

Termeric plugin APIs v1 through v32 are constrained local extension formats. They turn a reviewed TOML manifest into command-palette launch profiles; they do not load native libraries, execute manifest scripts in the UI process, or download anything. API v2 can additionally pass one explicitly granted, bounded, read-only JSON launch-context snapshot to the selected out-of-process profile. API v3 can invoke a separately declared out-of-process handler for four coarse lifecycle events and accept only bounded informational UI responses. API v4 can additionally disclose one completed OSC 133 command region to an explicitly `terminal-text`-granted handler. It is a bounded semantic completion, not a raw PTY/output stream. API v5 lets a handler opt into one persistent, request-correlated newline-delimited process so it can retain state without a process launch for every event. API v6 lets that persistent handler use its existing `workspace-query` grant to request a fresh, bounded inventory of the current window. API v7 adds a separate `workspace-actions` grant for exact stable-ID tab selection and pane focus in that same window. API v8 adds a separate `workspace-topology` grant for local tab ordering and split ratios. API v9 lets the same out-of-process handlers contribute bounded, namespaced command-palette commands with exact-owner dispatch. API v10 adds bounded typed non-secret settings, explicit user overrides, and safe update compatibility checks without adding process or host authority. API v11 adds a separate `terminal-query` grant for one bounded read of the presented text of an exact current-window pane. It is neither a PTY stream nor terminal input, and its result cannot initiate another host operation. API v12 adds a separate `pane-create` grant for splitting one exact ordinary pane in the current window. It cannot select a command, close anything, mutate a live domain, or act again from its correlated result. API v13 adds an independent `tab-create` grant for creating one ordinary local shell tab in the originating window. The action has no launch, destination, or input fields, and its result cannot initiate another host operation. API v14 adds an independent `pane-close` grant for closing one exact ordinary pane without retargeting or allowing the last pane in a tab to be destroyed. API v15 adds an independent `tab-close` grant for scheduling closure of one exact ordinary local tab while protecting the window's final non-closing tab. API v16 adds an independent `pane-move` grant for transferring one exact ordinary pane between tabs in the originating window without recreating its PTY. API v17 adds an independent `terminal-input` grant for bounded UTF-8 text sent to one exact current-window terminal. It additionally requires the user's disabled-by-default external-input policy, and destructive matches retain a visible exact-destination confirmation. API v18 adds an independent `pane-layout` grant for swapping two exact panes, balancing dividers, or applying a columns, rows, grid, tall, or fat preset within one ordinary current-window split tab without recreating its terminals. API v19 adds an independent `window-close` grant for the exact window in which a declared plugin command was invoked. It delivers one final correlated result before normal window teardown and grants neither a window identifier nor lifecycle-triggered closure. API v20 adds an independent `process-query` grant for a bounded, read-only inventory of every live window owned by the current Termeric process. It does not disclose terminal text or grant cross-window mutation. API v21 adds an independent `process-actions` grant for exact, non-activating tab selection and pane focus in any live process-owned window. It grants no native activation or broader cross-window mutation. API v22 adds an independent `process-terminal-text` grant for one bounded, presentation-safe read of an exact pane in any live process-owned window. It does not expose raw PTY bytes, stream output, activate a window, or grant input. API v23 adds an independent `process-terminal-input` grant for one guarded, bounded UTF-8 write to an exact pane in any live process-owned window. The destination window's disabled-by-default external-input policy and destructive confirmation remain mandatory. API v24 adds an independent `process-tab-create` grant for creating and selecting one ordinary local shell tab in an exact live process-owned window. It does not activate the native window, inherit an active-window target, or grant broader cross-window lifecycle control. API v25 adds an independent `process-pane-create` grant for splitting one exact ordinary pane in an exact live process-owned window. It reuses current-window split inheritance and validation without granting arbitrary launches, native activation, or broader topology mutation. API v26 adds an independent `process-pane-move` grant for moving one exact ordinary pane between exact live process-owned tabs or windows. It preserves the live terminal owner and returns a fresh destination-local pane identity without granting native activation, closure, or broader topology mutation. API v27 adds an independent `process-tab-move` grant for exact same-window tab reordering and cross-window ordinary-tab movement. It preserves the complete live tab state while explicitly refusing implicit source-window closure. API v28 adds an independent `process-pane-close` grant for closing one exact ordinary pane in an exact live process-owned window while preserving its tab and window and refusing final-pane or unsupported targets. API v29 adds an independent `process-tab-close` grant for scheduling closure of one exact ordinary tab in an exact live process-owned window while preserving its window and refusing the final non-closing tab. API v30 adds an independent `process-pane-layout` grant for swapping, balancing, or applying columns/rows/grid/tall/fat layouts to one exact ordinary split tab in an exact live process-owned window while preserving terminal identity and focus. API v31 adds an independent `process-window-close` grant for exact process-owned window closure after correlated result submission while protecting the final live process window. API v32 adds an independent `process-window-focus` grant for requesting native focus on one exact live process-owned window through the authenticated control policy without claiming compositor-confirmed activation. Installation, validation, configuration, live enable/disable, update, diagnostics, and removal require no rebuild.

The complete sample is [`examples/plugins/task-runner/termeric-plugin.toml`](../examples/plugins/task-runner/termeric-plugin.toml). The API v2 structured-context sample is [`examples/plugins/context-probe/termeric-plugin.toml`](../examples/plugins/context-probe/termeric-plugin.toml). The API v3 event-only sample is [`examples/plugins/lifecycle-notifier/termeric-plugin.toml`](../examples/plugins/lifecycle-notifier/termeric-plugin.toml). The API v4 command-event sample is [`examples/plugins/command-observer/termeric-plugin.toml`](../examples/plugins/command-observer/termeric-plugin.toml). The API v5 stateful-handler sample is [`examples/plugins/persistent-counter/termeric-plugin.toml`](../examples/plugins/persistent-counter/termeric-plugin.toml). The API v6 live-query sample is [`examples/plugins/workspace-inspector/termeric-plugin.toml`](../examples/plugins/workspace-inspector/termeric-plugin.toml). The API v7 navigation sample is [`examples/plugins/workspace-navigator/termeric-plugin.toml`](../examples/plugins/workspace-navigator/termeric-plugin.toml). The API v8 non-destructive topology sample is [`examples/plugins/workspace-organizer/termeric-plugin.toml`](../examples/plugins/workspace-organizer/termeric-plugin.toml). The API v9 handler-command sample is [`examples/plugins/command-tool/termeric-plugin.toml`](../examples/plugins/command-tool/termeric-plugin.toml). The API v10 typed-settings sample is [`examples/plugins/configurable-command/termeric-plugin.toml`](../examples/plugins/configurable-command/termeric-plugin.toml). The API v11 presented-terminal sample is [`examples/plugins/terminal-reader/termeric-plugin.toml`](../examples/plugins/terminal-reader/termeric-plugin.toml). The API v12 pane-creation sample is [`examples/plugins/pane-builder/termeric-plugin.toml`](../examples/plugins/pane-builder/termeric-plugin.toml). The API v13 tab-creation sample is [`examples/plugins/tab-builder/termeric-plugin.toml`](../examples/plugins/tab-builder/termeric-plugin.toml). The API v14 pane-closure sample is [`examples/plugins/pane-pruner/termeric-plugin.toml`](../examples/plugins/pane-pruner/termeric-plugin.toml). The API v15 tab-closure sample is [`examples/plugins/tab-pruner/termeric-plugin.toml`](../examples/plugins/tab-pruner/termeric-plugin.toml). The API v16 pane-movement sample is [`examples/plugins/pane-mover/termeric-plugin.toml`](../examples/plugins/pane-mover/termeric-plugin.toml). The API v17 terminal-input sample is [`examples/plugins/terminal-writer/termeric-plugin.toml`](../examples/plugins/terminal-writer/termeric-plugin.toml). The API v18 pane-layout sample is [`examples/plugins/pane-arranger/termeric-plugin.toml`](../examples/plugins/pane-arranger/termeric-plugin.toml). The API v19 window-closure sample is [`examples/plugins/window-closer/termeric-plugin.toml`](../examples/plugins/window-closer/termeric-plugin.toml). The API v20 process-inventory sample is [`examples/plugins/process-inspector/termeric-plugin.toml`](../examples/plugins/process-inspector/termeric-plugin.toml). The API v21 cross-window navigation sample is [`examples/plugins/process-navigator/termeric-plugin.toml`](../examples/plugins/process-navigator/termeric-plugin.toml). The API v22 cross-window terminal-text sample is [`examples/plugins/process-terminal-reader/termeric-plugin.toml`](../examples/plugins/process-terminal-reader/termeric-plugin.toml). The API v23 cross-window terminal-input sample is [`examples/plugins/process-terminal-writer/termeric-plugin.toml`](../examples/plugins/process-terminal-writer/termeric-plugin.toml). The API v24 cross-window tab-creation sample is [`examples/plugins/process-tab-builder/termeric-plugin.toml`](../examples/plugins/process-tab-builder/termeric-plugin.toml). The API v25 cross-window pane-creation sample is [`examples/plugins/process-pane-builder/termeric-plugin.toml`](../examples/plugins/process-pane-builder/termeric-plugin.toml). The API v26 cross-window pane-movement sample is [`examples/plugins/process-pane-mover/termeric-plugin.toml`](../examples/plugins/process-pane-mover/termeric-plugin.toml). The API v27 cross-window tab-movement sample is [`examples/plugins/process-tab-mover/termeric-plugin.toml`](../examples/plugins/process-tab-mover/termeric-plugin.toml). The API v28 cross-window pane-closure sample is [`examples/plugins/process-pane-pruner/termeric-plugin.toml`](../examples/plugins/process-pane-pruner/termeric-plugin.toml). The API v29 cross-window tab-closure sample is [`examples/plugins/process-tab-pruner/termeric-plugin.toml`](../examples/plugins/process-tab-pruner/termeric-plugin.toml). The API v30 cross-window pane-layout sample is [`examples/plugins/process-pane-arranger/termeric-plugin.toml`](../examples/plugins/process-pane-arranger/termeric-plugin.toml). The API v31 process-window closure sample is [`examples/plugins/process-window-closer/termeric-plugin.toml`](../examples/plugins/process-window-closer/termeric-plugin.toml). The API v32 process-window focus sample is [`examples/plugins/process-window-focuser/termeric-plugin.toml`](../examples/plugins/process-window-focuser/termeric-plugin.toml).

## Install and manage

Review the manifest and its program/argv first, then run:

```bash
termeric plugin validate examples/plugins/task-runner
termeric plugin install examples/plugins/task-runner \
  --allow process-spawn --enable
termeric plugin list
termeric plugin show task-runner --json
termeric plugin configure task-runner --default-profile tests
termeric plugin configure task-runner --clear-default-profile
```

An open Termeric process polls the registry at the existing 750 ms config deadline and updates command-palette entries without a restart. Settings → Plugins shows a bounded status summary and opens a keyboard-, pointer-, and screen-reader-accessible manager for every installed plugin. The manager shows the exact source path, version, enabled/diagnostic state, requested and granted permissions, effective setting values, and explicit-override count. It can install a plugin from an absolute local directory path after showing the canonical source, manifest fingerprint, identity, version, API, profile and command counts, and every requested permission. Grants are selected explicitly; the reviewed plugin may be installed disabled with a subset, while installing enabled requires every requested permission. The runtime re-reads the manifest and requires its fingerprint to match the reviewed fingerprint immediately before using the same atomic install path as the CLI. Duplicate IDs are refused and directed to the existing Update action rather than replaced silently. It can show the effective default launch profile and whether it comes from an explicit choice or manifest order. Its bounded profile picker selects by stable profile ID and can clear an explicit choice back to the first-profile manifest-order fallback; event-only and unhealthy plugins do not expose the picker. It can edit every declared Boolean, integer, text, and choice setting, restore one manifest default, enable or disable immediately, and review an update from its canonical source before committing it. Update review shows current and candidate versions and fingerprints, profile/command counts, added and removed permissions, and any configured default profile that the candidate removes. Existing grants start as their intersection with candidate requests; the update may remain disabled with a subset, while enabling requires every candidate permission. Commit revalidates both the reviewed source fingerprint and the exact installed registry record, so source or configuration drift requires a fresh review. Compatible settings remain intact, removed permissions cannot leave stale grants, and a removed configured profile falls back to manifest order. Settings reset and managed-copy removal retain request-scoped confirmations. Integer/text editing is single-line, UTF-8-safe, bounded, and available to keyboard, pointer, IME, and screen-reader input. Every save is revalidated through the same atomic registry path as the CLI, and an editor whose plugin schema changed after opening fails closed. A removal never deletes the recorded original source directory.

Replacement installation and relative-path interpretation remain explicit CLI operations. Default-profile selection/reset and typed setting set/unset are supported both in-app and through the CLI. `plugin list/show` remains the untruncated/scriptable diagnostic record. Remote installation, catalogs, and download/signature trust are not implied by the local in-app flow.

```bash
termeric plugin disable task-runner
termeric plugin enable task-runner

# Re-read the canonical source directory recorded at installation.
# This CLI path is intentionally conservative: newly requested permissions are
# refused. Use the in-app review to inspect and explicitly grant a changed set.
termeric plugin update task-runner

# Or deliberately replace from another reviewed local directory.
termeric plugin install /reviewed/new/source --replace \
  --allow process-spawn --enable

termeric plugin remove task-runner
```

Install copies only `termeric-plugin.toml` into the Termeric configuration directory. The registry records the canonical source directory and a deterministic content fingerprint. The fingerprint is change detection, not a cryptographic signature; provenance is the exact local path shown before and after installation. The in-app reviews bind installation and update to their displayed manifest fingerprints; update also binds the exact installed registry record. A source or configuration edit therefore requires a fresh review. Modifying the installed copy outside this flow makes the plugin fail closed. The registry, installed plugin directory, manifest, rollback copy, and administrative temporary files must be real files/directories rather than symlinks; replacement and removal reject unexpected file types before following or mutating them. Remote installation, a marketplace, automatic updates, and signing are not implemented.

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

Every profile contributes `<plugin-id>.open-<profile-id>` under its plugin name in the command palette. `--profile my-tools:check` and workspace restore use the same versioned hook and argv construction. A configured default profile is used only when a restore does not name one; a named archive profile always wins. With no configured default, the first profile in manifest order is used. The in-app **Use manifest order** action and CLI `plugin configure ID --clear-default-profile` restore that adaptive fallback; they do not store today's first profile ID.

Manifests reject unknown fields, unknown enum values, duplicate profile IDs, reserved built-in IDs, NULs, multiline identity fields, unsupported API versions, more than 32 profiles, more than 64 argv entries per profile, entries over 4 KiB, total profile argv over 32 KiB, and files over 64 KiB. Registries are schema-versioned, capped at 512 KiB, reject duplicates, relative or oversized source paths, malformed fingerprints/default profiles, reserved IDs, and unsupported grants, and contain at most 64 plugins. Both readers stop after the declared byte ceiling even if a file changes during the read. Plugin commands must remain inside their ID namespace and conflicts are rejected before load. A rejected registry unloads third-party entries and is shown as an error in both Settings and a bounded toast; it is never presented as an empty, healthy registry.

## Manifest API v2 structured launch context

API v2 retains the complete v1 profile contract and adds one capability: `workspace-query`. Install it only after reviewing both requested grants:

```bash
termeric plugin validate examples/plugins/context-probe
termeric plugin install examples/plugins/context-probe \
  --allow process-spawn --allow workspace-query --enable
```

When an API v2 plugin both requests and receives `workspace-query`, Termeric adds one environment variable to that plugin profile's PTY child: `TERMERIC_PLUGIN_CONTEXT_JSON`. Its compact JSON value is capped at 16 KiB and has exact schema version 1:

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

The snapshot is captured for the exact palette, CLI, or workspace-restore invocation. Workspace restore substitutes its archived working directory before serialization. The value is read-only and does not update after launch. It contains no terminal cells or scrollback, commands, selection, transcript, environment dump, clipboard, Git data, control token, secrets, or filesystem contents. Like any environment variable, the launched program can pass it to its descendants; granting it is therefore an intentional disclosure of active cwd and coarse tab/terminal geometry.

An API v1 manifest cannot receive this grant. Missing grants prevent enablement, unsupported grants fail installation or enablement, and an oversized context produces a visible notice instead of launching with a partial or silently truncated value. This snapshot remains launch-time only; it is not a live query.

## Manifest API v3 lifecycle handlers

API v3 preserves v1 profiles and the optional v2 launch context, then adds one optional `[event_handler]`. A handler can coexist with profiles or be the only plugin entry point:

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

For each subscribed event, Termeric starts the declared program off the UI thread, writes one compact schema-v1 JSON document to stdin, closes stdin, and reads one schema-v1 response from stdout. Event objects contain only their type and coarse numeric tab IDs/indices. With a separately requested and granted `workspace-query`, the envelope also contains active cwd, tab count/index, and terminal rows/columns; otherwise the entire `context` member is omitted. It never contains terminal or scrollback text, commands, PTY bytes, selection, transcripts, environment dumps, clipboard, secrets, Git data, or filesystem contents.

```json
{
  "schema_version": 1,
  "plugin_id": "lifecycle-notifier",
  "event": { "type": "tab-switched", "from": 0, "to": 1 }
}
```

The only accepted response actions are `show-message` and `show-notice`. A non-empty action list requires the separate `ui-actions` grant:

```json
{
  "schema_version": 1,
  "actions": [
    { "type": "show-message", "message": "Workspace helper is ready" }
  ]
}
```

API v3 and v4 handlers are process-per-event, and this remains the default in later APIs. Execution is serialized through a 32-entry queue, times out after two seconds, caps stdout at 64 KiB and stderr at 4 KiB, accepts at most eight actions, and validates bounded single-line control-free text. The child receives a cleared environment with only a minimal `PATH`, plugin ID, and event-schema marker. Queue overflow, launch/write/read/exit/timeout errors, malformed JSON, unknown fields/actions, wrong schemas, missing grants, and oversized data fail visibly. Plugin reload/unload cancels active and queued work from the previous registry generation and discards any racing stale result. These are informational callbacks, not terminal mutation, live queries, raw PTY streaming, render hooks, or a general automation runtime.

## Manifest API v4 semantic command events

API v4 preserves every earlier contract and adds one event kind, `command-completed`. The manifest must request `process-spawn`, `event-subscribe`, and `terminal-text`, and the operator must grant all three:

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

The handler receives the same schema-v1 envelope and worker isolation as API v3. One event identifies the producing application-local tab, optional stable pane, and terminal-local OSC 133 region. Text exists only while that exact completed region still has authoritative backing rows:

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

Command extraction allocates at most 4 KiB and output extraction at most 64 KiB before dispatch. The public event then allows at most 2 KiB of JSON-encoded command string data and 8 KiB of JSON-encoded output string data inside the existing 16 KiB complete-envelope ceiling. UTF-8 is never split. C0/C1 controls other than tab and newline are replaced and reported through `sanitized`; source or envelope clipping sets `truncated`. If scrollback eviction or erasure retired the region, both strings are empty and `content_available` is false, even if stale coordinates now refer to unrelated cells.

This event can contain passwords, tokens, pasted commands, build logs, and other sensitive terminal data. The grant is deliberately separate from `workspace-query`, `event-subscribe`, and `ui-actions`; an ungranted or API-v3 handler receives no command event. The event does not contain raw PTY bytes, keystrokes before command completion, arbitrary scrollback, selection, transcripts, environment variables, clipboard data, or filesystem contents. It provides no write, rerun, or terminal-control action. Optional coarse workspace context still requires its independent grant.

## Manifest API v5 persistent handlers

API v5 preserves all earlier events, disclosures, actions, and permission gates. It adds only an explicit handler lifetime and framing choice:

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

Omitting `mode` retains `process-per-event`; API v1 through v4 therefore keep their exact launch behavior. `persistent` is rejected before installation unless `api_version` is 5 or newer.

Termeric starts at most one persistent process for a plugin in each application window and plugin-registry generation. Requests and responses are compact JSON, one document per newline. Termeric upgrades the existing event envelope to schema 2 and injects a nonzero `request_id`:

```json
{"schema_version":2,"request_id":42,"plugin_id":"persistent-counter","event":{"type":"tab-created","tab_id":1}}
```

The handler must emit exactly one newline-terminated response before accepting the next request, echoing both schema 2 and that exact ID:

```json
{"schema_version":2,"request_id":42,"actions":[{"type":"show-message","message":"Persistent event #2"}]}
```

Requests remain within the 16 KiB complete-envelope ceiling, including the injected correlation fields. Responses retain the 64 KiB line, eight-action, permission, and text limits. One worker serializes all event requests through the existing 32-entry queue; no handler can have concurrent requests. A window may own at most eight persistent plugin processes. Stderr is drained for the entire lifetime so the child cannot block, while only the first 4 KiB is kept for a bounded diagnostic.

Every response has a two-second deadline. Launch failure, exit, timeout, malformed or unterminated JSON, an oversized line, wrong schema/request ID, or a handler descriptor change kills and forgets that process. A later event may start a fresh copy. Registry reload, plugin disable/removal, window shutdown, or application shutdown kills the complete process group and rejects stale generation results. The process receives the same cleared environment and minimal variables as process-per-event handlers, with `TERMERIC_PLUGIN_EVENT_SCHEMA=2`.

API v5 persistent mode is state retention and launch-amortization, not additional authority. It adds no permission, event, terminal query, mutation, raw-output, network, filesystem, render, or secret API. There are no unsolicited messages or background actions; output is consumed only as the correlated response to a Termeric request.

## Manifest API v6 live workspace query

API v6 preserves the API v5 process and framing contract. A persistent handler that both requests and receives `workspace-query` may return one additional action from an ordinary subscribed-event response:

```bash
termeric plugin install examples/plugins/workspace-inspector \
  --allow process-spawn --allow workspace-query \
  --allow event-subscribe --allow ui-actions --enable
```

```json
{"schema_version":2,"request_id":42,"actions":[{"type":"query-workspace","query_id":"current-window"}]}
```

`query_id` is a plugin-owned, control-free UTF-8 identifier of at most 64 bytes. It need not match the transport `request_id`; Termeric echoes both so the plugin can correlate its logical query and the exact protocol exchange. The query action does not require `ui-actions` and may coexist with informational UI actions. It is rejected unless all of these are true:

- the manifest uses API v6;
- the handler uses `mode = "persistent"`;
- `workspace-query` was requested and explicitly granted; and
- the response contains at most one workspace query.

After validating the response and confirming that the same plugin generation is still loaded in the exact originating window, Termeric captures current state and queues a schema-v2 `workspace-query-result` request through the same plugin's persistent handler. It reuses the same process while healthy and follows the documented restart behavior if that process exited:

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

The `tabs` inventory contains stable current-window tab IDs, sanitized titles, typed `shell`/`profile`/`plugin`/`live_domain` kinds, active/pinned/activity/ closed state, optional domain ID, pane inventory, divider topology, and per-tab truncation. Panes contain stable tab-local IDs, sanitized optional titles, focused/closed state, and cell dimensions. Dividers contain a stable topology-local ID, local/live-domain kind, horizontal/vertical direction, and ratio in thousandths.

The query exposes at most 16 tabs, 32 panes, 31 dividers, 2 KiB of aggregate inventory metadata, and 1 KiB of active-cwd text inside the existing 16 KiB persistent request ceiling. Control characters are removed, UTF-8 is never split, and every affected scope reports truncation. `tab_count` and `active_tab_index` describe the full current window even when the detailed inventory is clipped. The result has no synthetic window ID because every ID inside it is scoped to the exact originating window.

Results are asynchronous. Previously queued subscribed events may reach the serialized handler before the query result, so authors must use `query_id` rather than assuming adjacency. The response to a query result may contain the ordinary bounded UI actions and, under API v7's separate grant, one workspace action, but its query authority is cleared: results cannot chain another query. This prevents self-sustaining query loops while allowing one fresh snapshot in response to each external Termeric event.

The snapshot never contains terminal cells, scrollback, PTY bytes, command regions, selections, environment variables, clipboard data, filesystem contents, secrets, process-global inventories, or other windows. API v6 itself adds no mutation action and does not broaden `terminal-text`.

## Manifest API v7 workspace actions

API v7 preserves all earlier contracts and introduces one new capability: `workspace-actions`. A plugin must request and receive this grant separately from both `workspace-query` and `ui-actions`, and must use a persistent handler:

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

`action_id` is plugin-owned, control-free UTF-8 of at most 64 bytes and is echoed in the result. `tab_id` and `pane_id` are the stable, current-window IDs returned by the API v6 inventory; they are not tab indices. Termeric validates the entire target before changing either tab or pane focus, so a missing, closing, or closed target fails without partially retargeting the window.

At most one host operation—one query or one workspace action—is accepted in a single response. Informational `show-message` or `show-notice` actions may coexist when separately granted. Workspace actions are rejected unless the manifest uses API v7, the handler is persistent, and `workspace-actions` was both requested and granted. Process-per-event handlers and API v1 through v6 cannot acquire this authority.

Termeric applies the operation only after confirming that the originating plugin generation is still loaded in the exact current window, then sends a correlated result through that plugin's persistent handler:

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

Successful `focus-pane` results instead contain `pane_id`, `tab_changed`, and `pane_changed`. A validation failure is still a normal correlated result with `ok: false`, a bounded control-safe `error`, null change fields, and an `error_truncated` flag. This lets a handler react without parsing a toast or log.

A query result retains workspace-action and API-v8 topology authority so the bounded useful flow is `external event → query → one mutation → result`. A workspace-action result clears every query/navigation/topology authority; its response may contain only separately granted informational UI actions. Plugin-origin navigation also suppresses the derived `tab-switched` subscription event. Together these rules prevent a handler from manufacturing a navigation loop, while ordinary user and authenticated-control tab switches continue to emit the lifecycle event. Subscribed events already queued for the serialized handler may interleave, so authors must correlate with `action_id` and transport `request_id` rather than assuming adjacent messages.

API v7 itself does not add tab/pane creation, closure, movement, resize, terminal input, terminal text access, process-global navigation, cross-window targeting, or arbitrary commands.

## Manifest API v8 non-destructive topology actions

API v8 preserves all earlier contracts and adds `workspace-topology`, separate from query, navigation, UI, and terminal-text authority. It requires an API-v8 persistent handler:

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

`move-tab` uses a stable local tab ID plus a final zero-based visual index. The index must be below the protocol's 256-tab ceiling and the window's current tab count. Termeric rejects missing, closing, live-domain, or pinned-boundary crossing targets before changing order. Reordering preserves the exact active tab rather than selecting whatever lands at its prior index.

`resize-divider` uses stable tab-local divider identity from the v6 inventory. Ratios are integer thousandths from 100 through 900. The operation rejects missing or closing tabs, unsplit or zoomed layouts, missing dividers, and live-domain topology rather than guessing at a sibling or optimistically acknowledging daemon state. Successful local changes recalculate pane geometry and request a redraw without changing tab or pane focus.

Both operations return the existing correlated `workspace-action-result` event. A tab result contains `destination_index` and `changed`; a divider result contains `divider_id`, `previous_ratio_millis`, `ratio_millis`, and `changed`. An invalid current target is a bounded `ok: false` result with null result fields and a control-safe error. Static protocol violations such as a 99‰ ratio or an index at/above the 256-tab ceiling reject the handler response itself.

At most one query, navigation action, or topology action is accepted per response. A query result may perform one separately granted topology action; the resulting event clears query, navigation, and topology authority. Neither operation emits a derived tab lifecycle hook, so response-driven topology changes cannot manufacture an event loop. Already queued subscribed events may still interleave, and authors must correlate logical and transport IDs.

API v8 does not permit tab/pane creation or closure, pane movement/splitting, cross-window or process-global topology, live-domain mutation, terminal input, terminal content, or arbitrary commands. `workspace-topology` can be granted without `workspace-query` when a plugin already has a current stable ID, but Termeric does not make stale IDs succeed.

## Manifest API v9 handler commands

API v9 preserves every earlier permission and response boundary while allowing an event handler to add ordinary command-palette entries. It adds no permission and no host action: commands require the existing `process-spawn` and `event-subscribe` grants, an `[event_handler]`, and an explicit subscription to `command-invoked`.

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

Each local ID becomes `<plugin-id>.<command-id>`; the example contributes `command-tool.show-status` under the plugin's palette category. IDs use the same lowercase identifier grammar as profiles. Names are capped at 80 bytes, descriptions at 512 bytes, and a manifest may declare at most 32 handler commands. Together with the existing 32-profile ceiling, a plugin can never exceed the manager's 64-command boundary. Duplicate local IDs and collisions with generated `open-<profile-id>` commands fail validation.

Selecting the entry sends only its owning plugin a normal bounded handler request. No other plugin observes the invocation:

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

The optional coarse `context` member follows the existing `workspace-query` grant. Responses use the handler's existing schema and permissions; API v9 does not implicitly grant UI, workspace query, navigation, topology, terminal text, or any other authority. Both process-per-event and persistent handlers are valid. Undeclared IDs, another plugin's namespace, missing subscriptions, API v8 or older manifests, and namespace/profile conflicts fail closed. Enabled [coding agents](agents.md) register first, so a plugin whose id matches one is refused.

## Manifest API v10 typed settings

API v10 preserves all earlier permissions and adds a bounded non-secret configuration schema. Settings do not grant capabilities and cannot read host state; they are values the user explicitly supplies to the already reviewed profile or handler executable.

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

A manifest may declare at most 32 unique setting IDs. Names are capped at 80 bytes, descriptions at 512 bytes, text values at 512 bytes, and choice lists at 32 unique non-empty values of at most 80 bytes each. Text is single-line and NUL-free. Integer manifests require `min <= default <= max`; configured values must remain inside that inclusive range. Effective values, including defaults, must fit one 8 KiB compact JSON object. Unknown types/fields, duplicate IDs, wrong value types, unknown choices, oversized values, and API v9 or older manifests fail closed.

Configure one override per atomic command:

```bash
termeric plugin configure configurable-command --set greeting "Hello team"
termeric plugin configure configurable-command --set audience terminal
termeric plugin configure configurable-command --set include-command-id true
termeric plugin configure configurable-command --unset greeting
termeric plugin configure configurable-command --reset-settings
```

The same four setting types can be edited under Settings → Plugins → select the plugin → Configure settings. Boolean and choice values use bounded option lists; integer and text values use a visible single-line editor. `Ctrl+D` restores the selected setting's manifest default while an input is active.

`--unset` restores one manifest default; `--reset-settings` restores every default. The registry stores only explicit overrides. `plugin show ID` and its JSON form expose effective values deliberately, so this is not a secret store: tokens, passwords, credentials, and other sensitive values must not be placed in plugin settings. Registry type tampering makes the plugin fail discovery rather than coercing a value.

API v10 profiles receive a second compact environment variable independent of the optional workspace context:

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

This document is passed as `TERMERIC_PLUGIN_SETTINGS_JSON`. Handler requests receive the same effective map as a top-level `settings` member. No setting is interpolated into argv, a shell command, paths, permissions, or environment variable names. Existing structured framing and request bounds still apply.

Update and `--replace` preserve compatible explicit overrides. If a new manifest removes a configured key, changes its type, narrows its range, removes its selected choice, or otherwise makes the stored value invalid, replacement fails before changing the installed manifest or registry. The user can unset or reset the old overrides and retry; Termeric never silently drops or guesses a migration.

## Manifest API v11 presented-terminal query

API v11 preserves all earlier capabilities and adds the independent `terminal-query` permission. Only a persistent event handler may use it. A typical user-invoked command first requests the API v6 workspace inventory, selects the stable IDs of its active tab and focused pane, and then returns one query action:

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

The manifest must use API v11, request `terminal-query`, use `mode = "persistent"`, and receive an explicit `--allow terminal-query` grant. `query_id` is a control-safe identifier of at most 64 bytes. `history_lines` may be 0 through 256 and `max_bytes` may be 1 through 8,192. The tab and pane IDs are scoped to the exact originating window; missing, closing, or stale targets return an error for those IDs and are never retargeted to whichever pane is active later.

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

The primary screen may include only the requested bounded primary history. An active alternate screen is viewport-only and never crosses into primary history. Text comes from the exact presentation state: output hidden inside an unfinished synchronized-output batch is not disclosed. The result is UTF-8, reports its actual included history and text-byte count, and marks clipping. If JSON escaping would exceed the persistent request ceiling, Termeric reduces the text budget and reports `truncated = true`; the complete source event still fits below 16 KiB.

One response may contain at most one host operation across workspace query, terminal query/input, navigation, topology, pane/tab creation, pane/tab closure, pane movement, and pane layout, though bounded UI notices may coexist. A workspace-query result retains separately granted terminal-query, pane-create, tab-create, pane-close, tab-close, pane-move, terminal-input, and pane-layout authority so one external command can discover current state and then perform one granted operation. After a terminal-query result, all query and workspace-mutation authority is cleared. A later external subscribed event may start another independently bounded request. API v11 does not expose cell styles, images, selection, clipboard, environment, raw PTY bytes, live output subscriptions, terminal input, other windows, or process-global terminal discovery.

## Manifest API v12 ordinary-pane creation

API v12 preserves every earlier contract and adds the independent `pane-create` permission. Only a persistent handler may use it. Install the sample only after reviewing its ordinary Python process and both host-data and pane-creation grants:

```bash
termeric plugin validate examples/plugins/pane-builder
termeric plugin install examples/plugins/pane-builder \
  --allow process-spawn --allow workspace-query \
  --allow pane-create --allow event-subscribe --allow ui-actions --enable
```

The sample contributes **Pane Builder: Split Active Pane** to the command palette. It first uses the existing bounded current-window inventory to obtain stable IDs, then returns exactly one operation:

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

`direction` is exactly `horizontal` (panes stacked vertically) or `vertical` (panes side by side). IDs are resolved when the operation executes. The target must be a live, non-closing ordinary shell/profile/plugin pane in the originating window. Missing IDs, serial tabs, live-domain tabs, the per-tab pane limit, an unavailable renderer for a first split, and PTY spawn failures return bounded errors without retargeting another pane.

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

The pane inherits the target tab's frozen split launch and configured cwd policy. No executable, argv, environment, cwd, title, or input field exists in this action. Creation counts as the response's sole host operation, and its result clears every host-operation authority. A later external event may begin a new bounded request. Tab creation requires the separate API v13 grant and pane closure requires the separate API v14 grant. Tab closure requires the separate API v15 grant; arbitrary commands, live-domain mutation, and cross-window placement remain unsupported powers.

## Manifest API v13 ordinary-tab creation

API v13 preserves every earlier contract and adds the independent `tab-create` permission. Only a persistent handler may use it. Install the sample only after reviewing its ordinary Python process and tab-creation grant:

```bash
termeric plugin validate examples/plugins/tab-builder
termeric plugin install examples/plugins/tab-builder \
  --allow process-spawn --allow tab-create \
  --allow event-subscribe --allow ui-actions --enable
```

The sample contributes **Tab Builder: New Task Tab** to the command palette and returns exactly one operation:

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

The action always targets the handler's originating native window and creates the same ordinary local shell tab as the in-app New Tab action. It accepts no window ID, executable, argv, environment, cwd, title, profile, domain, or input. The new tab becomes active and uses the window's current validated default shell configuration. If the tab manager is unavailable, the active tab belongs to a live domain, the per-window limit is reached, or PTY launch fails, creation returns a bounded error without a fabricated identity.

Success returns the new stable window-local tab ID; failure returns it as `null`:

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

Creation counts as the response's sole host operation. Its result clears query, navigation, topology, terminal-query/input, pane-create, tab-create, pane-close, tab-close, pane-move, and pane-layout authority. Termeric deliberately suppresses the derived `tab-created` hook for this plugin-originated action, so neither the requester nor another plugin can turn one creation into a lifecycle feedback loop. A later independent external event may begin another bounded request. Arbitrary launch selection, live-domain creation, and cross-window placement remain unsupported. Closure requires the separate API v15 grant.

## Manifest API v14 ordinary-pane closure

API v14 preserves every earlier contract and adds the independent `pane-close` permission. Only a persistent handler may use it. Because closure destroys a running pane and its process, review both the executable and this grant before installing the sample:

```bash
termeric plugin validate examples/plugins/pane-pruner
termeric plugin install examples/plugins/pane-pruner \
  --allow process-spawn --allow workspace-query \
  --allow pane-close --allow event-subscribe --allow ui-actions --enable
```

The sample contributes **Pane Pruner: Close Sibling Pane** to the command palette. It queries the bounded current-window inventory, selects one non-focused sibling in the active ordinary tab, and returns one exact action:

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

The stable IDs are resolved again when the action executes. Termeric refuses a missing or already closed pane, a closing tab, a live-domain or serial tab, an unsplit tab, and any operation that would remove the tab's last pane. It does not implicitly focus or select the target. Closing a non-active pane therefore leaves the active tab unchanged. If one pane remains after a successful close, the split manager collapses back to the ordinary single-terminal layout.

Success reports the remaining count and resulting focused pane; failure keeps those fields `null` and returns a bounded, sanitized error:

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

Pane closure is the response's sole host operation. Its result clears query, navigation, topology, terminal-query/input, pane-create, tab-create, pane-close, tab-close, pane-move, and pane-layout authority. A later external event may begin another bounded request. API v14 does not itself grant tab closure, live-domain mutation, terminal input, cross-window targeting, arbitrary process signaling, or a force/confirmation bypass. Tab closure requires the separate API v15 grant.

## Manifest API v15 ordinary-tab closure

API v15 preserves every earlier contract and adds the independent `tab-close` permission. Only a persistent handler may use it. Tab closure terminates the tab's PTY/process tree after the normal close animation, so review both the executable and this grant before installing the sample:

```bash
termeric plugin validate examples/plugins/tab-pruner
termeric plugin install examples/plugins/tab-pruner \
  --allow process-spawn --allow workspace-query \
  --allow tab-close --allow event-subscribe --allow ui-actions --enable
```

The sample contributes **Tab Pruner: Close Background Tab** to the command palette. It queries the bounded current-window inventory, selects one visible non-active non-live-domain tab, and returns one exact action:

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

The stable ID is resolved again when the action executes. Shell, profile, plugin, pinned, and serial tabs are eligible because they are process-local. Termeric refuses a missing, closed, already-closing, or live-domain target. It also counts only non-closing tabs and refuses the final survivor, even if other tab records are still completing their animations. This grant cannot close a window implicitly.

Closing a background tab preserves the current selection. Closing the active tab uses the ordinary tab manager's deterministic next-tab selection. Any copy mode owned by that tab is dismissed and focused terminal surfaces are rebound. Success means the normal animated close was accepted and scheduled; it does not claim that autosave, animation, PTY shutdown, or physical removal has already completed:

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

Failures return `scheduled: false`, null remaining/active fields, and a bounded sanitized error. Tab closure is the response's sole host operation, and its result clears query, navigation, topology, terminal-query/input, creation, both closure authorities, pane-move, and pane-layout authority. Termeric suppresses the derived `tab-closed` hook for a plugin-originated close, making the correlated result the only follow-up and preventing lifecycle-driven close loops. A later independent external event may begin another bounded request. API v15 does not grant final-tab/window closure, live-domain mutation, cross-window targeting, force-kill/signals, terminal input, or a confirmation bypass.

## Manifest API v16 ordinary-pane movement

API v16 preserves every earlier contract and adds the independent `pane-move` permission. Only a persistent handler may use it. Install the sample only after reviewing its ordinary Python process plus the workspace-query and pane-move grants:

```bash
termeric plugin validate examples/plugins/pane-mover
termeric plugin install examples/plugins/pane-mover \
  --allow process-spawn --allow workspace-query \
  --allow pane-move --allow event-subscribe --allow ui-actions --enable
```

The sample contributes **Pane Mover: Move Sibling Pane** to the command palette. It queries the bounded current-window inventory, chooses a non-focused pane in the active split tab and an exact anchor in another ordinary tab, then returns:

```json
{
  "schema_version": 2,
  "request_id": 16,
  "actions": [{
    "type": "move-pane",
    "action_id": "move-sibling-pane",
    "source_tab_id": 4,
    "pane_id": 1,
    "destination_tab_id": 7,
    "target_pane_id": 0,
    "direction": "vertical"
  }]
}
```

Every ID is resolved again in the exact originating native window when the action executes. The source and destination tabs must be distinct, ordinary, present, and non-closing. The source must be split and keep at least one pane; the selected pane must exist, remain open, and not own a tab-scoped plugin process. The destination must permit splits, the anchor must exist, and its pane, divider, and stable-ID capacities must remain available. Live-domain sources or destinations and serial destinations fail before detachment.

`horizontal` stacks the moved pane below the destination anchor; `vertical` places it beside the anchor. The existing atomic transfer path validates both halves before mutation, detaches the live owner, and attaches it under a fresh destination-local stable pane ID. The PTY process, terminal state, scrollback, launch identity, and retained metadata are not recreated. Callers must use the returned identity rather than assuming the source pane ID survived.

A successful transfer activates the destination tab and focuses the moved pane. It exits copy mode, disables broadcast, clears an active terminal mouse drag, refreshes focused terminal surfaces, and requests a redraw. The result reports that exact state:

```json
{
  "schema_version": 1,
  "plugin_id": "pane-mover",
  "event": {
    "type": "workspace-action-result",
    "action_id": "move-sibling-pane",
    "action": "move-pane",
    "ok": true,
    "source_tab_id": 4,
    "pane_id": 1,
    "destination_tab_id": 7,
    "target_pane_id": 0,
    "direction": "vertical",
    "destination_pane_id": 3,
    "source_remaining_panes": 1,
    "active_tab_id": 7,
    "focused_pane_id": 3,
    "active_changed": true,
    "error": null
  }
}
```

Failure keeps the destination/state fields `null` and returns a bounded, sanitized error without detaching the source. Pane movement is the response's sole host operation. Its result clears query, navigation, topology, terminal-query/input, pane/tab creation, pane/tab closure, pane-move, and pane-layout authority. The correlated result is the only follow-up; plugin-originated movement does not synthesize tab lifecycle hooks. A later independent external event may begin another bounded request.

API v16 does not grant same-tab layout-tree rearrangement, cross-window movement, live-domain mutation, window or final-tab closure, terminal input, arbitrary launch selection, or access to another window's inventory.

Ordinary plugin-owned panes are valid move sources: their complete live terminal, profile, recorder, and diff state transfer without relaunching. The last-source pane rule and destination capacity/ID checks still apply. This does not grant terminal input or launch authority; see [plugin panes](plugin-panes.md).

## Manifest API v17 terminal input

API v17 preserves every earlier contract and adds the independent `terminal-input` permission. Only a persistent handler may use it. This is a powerful capability: review both the ordinary Python handler and its grants, then explicitly opt into external terminal input before installing the sample:

```toml
[security]
allow_gui_control_input = true
```

```bash
termeric plugin validate examples/plugins/terminal-writer
termeric plugin install examples/plugins/terminal-writer \
  --allow process-spawn --allow workspace-query \
  --allow terminal-input --allow event-subscribe --allow ui-actions --enable
```

The historical setting name also covers plugin-originated input so one global, disabled-by-default policy controls all external terminal writes. A manifest grant alone never overrides it. The sample contributes **Terminal Writer: Draft Cargo Test** to the command palette. It queries the bounded current-window inventory, selects the exact focused pane in the active tab, and requests:

```json
{
  "schema_version": 2,
  "request_id": 17,
  "actions": [{
    "type": "send-text",
    "action_id": "draft-cargo-test",
    "tab_id": 4,
    "pane_id": 1,
    "text": "cargo test",
    "paste": false,
    "enter": false
  }]
}
```

`text` is limited to 8 KiB and may contain printable UTF-8, tab, and newline, but never NUL, carriage return, escape, or arbitrary control bytes. Empty text is valid only with `enter: true`. `paste: true` consults the exact target terminal's live bracketed-paste mode; `enter: true` appends exactly one carriage return after the text or completed paste frame. The action does not change the active tab, focus, selection, copy mode, broadcast targets, or clipboard.

Execution revalidates the exact current-window tab and stable pane ID. Missing, closed, closing, terminal-less, read-only, and already-pending-confirmation targets fail without writing. Destructive-command matching is based on the plugin origin, not the destination kind: even an ordinary shell tab captures the exact encoded bytes and origin behind Termeric's visible guardrail. Focus changes cannot retarget that confirmation, cancellation revokes the bytes immediately, and no input is written unless the user confirms.

A successful request returns either `sent` or `pending_confirmation`:

```json
{
  "schema_version": 1,
  "plugin_id": "terminal-writer",
  "event": {
    "type": "workspace-action-result",
    "action_id": "draft-cargo-test",
    "action": "send-text",
    "ok": true,
    "tab_id": 4,
    "pane_id": 1,
    "input_bytes": 10,
    "wire_bytes": 10,
    "paste": false,
    "enter": false,
    "disposition": "sent",
    "confirmation_id": null,
    "error": null
  }
}
```

The response is the request's sole host operation. Its result clears query, navigation, topology, terminal-query/input, creation, closure, movement, and layout authority. A later independent external event may begin another bounded request. API v17 does not grant raw byte input, raw terminal-output events, process-global or cross-window targets, focus changes, arbitrary process signals, filesystem/clipboard/network access, or confirmation bypass.

## Manifest API v18 same-tab pane layouts

API v18 preserves every earlier contract and adds the independent `pane-layout` permission. Only a persistent handler may use it. Review and install the sample with exactly its requested grants:

```bash
termeric plugin validate examples/plugins/pane-arranger
termeric plugin install examples/plugins/pane-arranger \
  --allow process-spawn --allow workspace-query \
  --allow pane-layout --allow event-subscribe --allow ui-actions --enable
```

The sample contributes **Pane Arranger: Apply Grid Layout**. It queries the bounded current-window inventory and applies a preset to the exact active split tab. API v18 accepts exactly one of these host operations per response:

```json
{"type":"swap-panes","action_id":"swap","tab_id":4,"first_pane_id":1,"second_pane_id":2}
{"type":"balance-panes","action_id":"balance","tab_id":4}
{"type":"apply-pane-layout","action_id":"grid","tab_id":4,"preset":"grid"}
```

`preset` is strictly `columns`, `rows`, `grid`, `tall`, or `fat`. Tall makes the focused pane the full-height left master; Fat makes it the full-width top master. Both use a 50% master area and preserve the other panes' relative visual order. `swap-panes` preserves the existing divider tree and exchanges only the two named stable pane identities. `balance-panes` resets every divider to one-half and reports whether anything changed. A preset rebuilds the tree from current visual pane order and clears view-only zoom because the hidden geometry was replaced. All operations preserve the live PTY/terminal owner, focused pane identity, active tab, and current-window stable pane IDs; swap and balance also preserve zoom ownership.

Execution resolves the tab and panes again in the originating native window. The tab must be an ordinary, non-closing split with at least two live panes and no closed records awaiting cleanup. Missing/stale IDs, duplicate swap IDs, unsplit, serial, and daemon-authoritative live-domain tabs fail atomically. Background ordinary tabs are valid targets and are not activated.

A success returns the resulting visual `pane_order`, `focused_pane_id`, and nullable `zoomed_pane_id`; balance additionally returns `changed`, while a preset echoes its name. Failures return those state fields as `null` plus a bounded sanitized error. The correlated result clears every host-operation authority, so it cannot chain another query or mutation. API v18 does not grant pane creation/closure/movement, cross-tab or cross-window changes, live-domain topology control, terminal input/output, process signals, or arbitrary layout trees.

## Manifest API v19 exact window closure

API v19 preserves every earlier contract and adds the independent `window-close` permission. It is intentionally narrower than ordinary lifecycle subscriptions: only an API v19 persistent handler subscribed to `command-invoked`, with at least one declared command and the exact retained grant, can receive closure authority. Review and install the sample with:

```bash
termeric plugin validate examples/plugins/window-closer
termeric plugin install examples/plugins/window-closer \
  --allow process-spawn --allow workspace-query \
  --allow window-close --allow event-subscribe --enable
```

The sample contributes **Window Closer: Close Current Window**. Its first response queries the invoking window, and the query result may return exactly this host operation:

```json
{"type":"close-window","action_id":"close-current-window"}
```

There is deliberately no `window_id`. The command dispatch and persistent continuation already belong to one exact `App` and native window, preventing active-window drift and cross-window selection. App-ready, tab lifecycle, and command-completed invocations always receive `allow_window_close = false`, even when the plugin has the grant. The same applies to undeclared or differently owned commands.

Termeric revalidates that the window has initialized tab owners, is not already closing, and has no pending plugin closure. A success result has `action = "close-window"`, `scheduled = true`, and the current `tab_count`. The window remains alive while this bounded event is submitted to the same persistent handler. Only the exact plugin ID, plugin generation, and returned worker request ID can finish the schedule. After that callback succeeds or fails, Termeric marks the exact window for its ordinary teardown path; sibling windows remain alive and the process exits only if none survive. Informational or host actions returned by this final callback are ignored because teardown has begun.

If the result event cannot be encoded or submitted, closure is not armed and the window stays open. Once submission succeeds, later plugin unload or generation cancellation cannot strand an accepted close: the matching failed callback still completes teardown. Duplicate requests and already-closing or uninitialized windows return `scheduled = false` with a bounded sanitized error. The result clears every host-operation authority, so it cannot close a second window, query again, or create a mutation loop. API v19 does not grant process-global targeting, arbitrary window IDs, background lifecycle closure, terminal-input confirmation bypass, or control of another application window.

## Manifest API v20 process-wide inventory

API v20 preserves every earlier contract and adds the independent `process-query` disclosure grant. Review and install the sample with:

```bash
termeric plugin validate examples/plugins/process-inspector
termeric plugin install examples/plugins/process-inspector \
  --allow process-spawn --allow process-query \
  --allow event-subscribe --allow ui-actions --enable
```

The sample contributes **Process Inspector: Inspect Process Windows**. Its command response requests one process snapshot:

```json
{"type":"query-process","query_id":"all-windows"}
```

Only an API v20 persistent handler with the retained `process-query` grant may emit that action. Termeric resolves it in the process-level `Application` registry rather than an active-window approximation, preserves native-window creation order, omits windows already closing, and returns the stable ID of the exact originating window:

```json
{
  "schema_version": 2,
  "request_id": 43,
  "plugin_id": "process-inspector",
  "event": {
    "type": "process-query-result",
    "query_id": "all-windows",
    "snapshot_schema_version": 1,
    "process_id": 12345,
    "origin_window_id": 7,
    "windows": [],
    "truncated": false
  }
}
```

Window IDs are stable only for the lifetime of this GUI process. Tab IDs remain window-local, and pane/divider IDs retain their documented tab-local scopes. The result exposes at most 8 windows, 16 total tabs, 32 total panes, 31 total dividers, and 2 KiB of aggregate metadata inside the existing 16 KiB persistent-event ceiling. Control characters are removed, UTF-8 is not split, and every affected window/tab plus the process envelope reports truncation. If even the bounded shape cannot be encoded within the frame ceiling, the query fails instead of sending a partial or oversized event.

The shape reuses the typed window/tab/pane/divider inventory documented for API v6, adding only process-local stable window IDs and process scope. It never contains presented terminal cells, scrollback, raw PTY bytes, command text, selection, environment, clipboard, filesystem contents, or secrets. The `process-query` grant does not broaden `workspace-query` for existing plugins. For API v20, the result clears process/current-window/terminal query authority and every mutation authority before delivery. An API v21+ handler may retain only separately granted `process-actions`, `process-terminal-text`, `process-terminal-input`, `process-tab-create`, `process-pane-create`, `process-pane-move`, `process-tab-move`, `process-pane-close`, `process-tab-close`, `process-pane-layout`, `process-window-close`, and (for API v32) `process-window-focus` authorities for the single bounded query-to-navigation, query-to-text, query-to-input, query-to-tab-creation, or query-to-pane-creation/movement/closure/layout, query-to-tab-movement/closure, or query-to-window-closure/focus sequences documented below. Those remain separate grants; process inventory itself authorizes no native activation, input, topology, movement, creation, or closure.

## Manifest API v21 cross-window workspace navigation

API v21 preserves every earlier contract and adds the independent `process-actions` mutation grant. Review and install the sample with:

```bash
termeric plugin validate examples/plugins/process-navigator
termeric plugin install examples/plugins/process-navigator \
  --allow process-spawn --allow process-query \
  --allow process-actions --allow event-subscribe \
  --allow ui-actions --enable
```

The sample contributes **Process Navigator: Focus First Pane in Another Window**. After requesting the API v20 inventory, it may emit exactly one of:

```json
{"type":"select-process-tab","action_id":"results","window_id":7,"tab_id":12}
{"type":"focus-process-pane","action_id":"shell","window_id":7,"tab_id":12,"pane_id":3}
```

Only an API v21 persistent handler with the retained `process-actions` grant may emit either action. `window_id` is the process-lifetime identity returned by API v20, `tab_id` is local to that window, and `pane_id` is local to that tab. Termeric revalidates the originating plugin, window, generation, persistent continuation, and exact authority before touching the target, then validates the complete target path immediately before mutation. It never falls back to the originating, active, or focused window/tab/pane when a target is stale, missing, closed, closing, or uninitialized.

`select-process-tab` changes only the selected tab in the target application window. `focus-process-pane` first validates the exact pane and then changes tab and pane focus atomically. Both reuse the non-recursive plugin navigation path, so a derived `tab-switched` subscription is not dispatched in the target window. Neither action activates, raises, shows, restores, minimizes, resizes, or otherwise manipulates the native window. This keeps the contract portable across X11 and Wayland and avoids background focus stealing.

Success returns a correlated result to the originating handler:

```json
{
  "schema_version": 2,
  "request_id": 44,
  "plugin_id": "process-navigator",
  "event": {
    "type": "process-action-result",
    "action_id": "shell",
    "action": "focus-process-pane",
    "ok": true,
    "process_id": 12345,
    "origin_window_id": 2,
    "window_id": 7,
    "tab_id": 12,
    "pane_id": 3,
    "tab_changed": true,
    "pane_changed": true,
    "error": null
  }
}
```

A target failure is still a normal result with `ok: false`, the exact target IDs, null change fields, a control-safe error of at most 1 KiB, and an `error_truncated` flag. At most one host operation is accepted per handler response. Informational UI actions may coexist when separately granted. For API v21, a process-query result retains only separately granted `process-actions` authority; the process-action result clears every query and mutation authority, so it cannot navigate, query, or mutate again. Existing API v7 navigation remains current-window-only and unchanged.

## Manifest API v22 bounded cross-window terminal text

API v22 preserves every earlier contract and adds the independent `process-terminal-text` disclosure grant. Review and install the sample with:

```bash
termeric plugin validate examples/plugins/process-terminal-reader
termeric plugin install examples/plugins/process-terminal-reader \
  --allow process-spawn --allow process-query \
  --allow process-terminal-text --allow event-subscribe \
  --allow ui-actions --enable
```

The sample contributes **Process Terminal Reader: Read Sibling Pane Text**. After requesting the API v20 inventory and choosing one exact pane, it emits:

```json
{"type":"query-process-terminal","query_id":"sibling-output","window_id":7,"tab_id":12,"pane_id":3,"history_lines":20,"max_bytes":2048}
```

Only an API v22 persistent handler with the retained `process-terminal-text` grant may emit this action. The process-lifetime `window_id`, window-local `tab_id`, and tab-local `pane_id` must be exact. Termeric revalidates the originating plugin, window, generation, persistent continuation, and authority before resolving the complete target path. A missing, stale, closed, closing, or uninitialized target produces a correlated failure; no active or focused target is substituted.

The query returns only the terminal's presentation-safe text snapshot: the current primary or alternate screen plus at most 256 requested primary-screen history lines and at most 8 KiB of UTF-8 text. Pending synchronized-output updates, raw PTY bytes, selection state, transcripts, environment, clipboard, and filesystem contents are excluded. JSON framing is also capped by the existing persistent-event ceiling; the text budget is reduced and `truncated` is set if escaping or envelope metadata would otherwise exceed that ceiling.

```json
{
  "schema_version": 2,
  "request_id": 45,
  "plugin_id": "process-terminal-reader",
  "event": {
    "type": "process-terminal-query-result",
    "query_id": "sibling-output",
    "text_schema_version": 1,
    "ok": true,
    "process_id": 12345,
    "origin_window_id": 2,
    "window_id": 7,
    "tab_id": 12,
    "pane_id": 3,
    "screen": "primary",
    "requested_history_lines": 20,
    "requested_max_bytes": 2048,
    "history_lines": 20,
    "text_bytes": 731,
    "text": "bounded presented text",
    "truncated": false,
    "error": null
  }
}
```

A target failure returns `ok: false`, the same correlation and target fields, null text/screen/history/truncation fields, zero `text_bytes`, and a bounded control-safe error with `error_truncated`. The operation never selects a tab, focuses a pane, changes scroll position, raises or activates a native window, or dispatches navigation hooks.

At most one host operation is accepted per response. An API v22+ process-query result may retain only separately granted `process-actions`, `process-terminal-text`, `process-terminal-input`, `process-tab-create`, `process-pane-create`, `process-pane-move`, `process-tab-move`, `process-pane-close`, `process-tab-close`, `process-pane-layout`, `process-window-close`, and (for API v32) `process-window-focus` authority, allowing one inventory-to-navigation, inventory-to-text, inventory-to-input, inventory-to-tab-creation, inventory-to-pane-creation/movement, or inventory-to-tab-movement, inventory-to-pane/tab/window-closure, window-focus, or inventory-to-pane-layout sequence. A process-terminal-query result clears every host query and mutation authority before delivery, so it cannot read a second pane, navigate, send input, or otherwise chain. Existing API v11 `terminal-query` remains exact-current-window-only and unchanged.

## Manifest API v23 guarded cross-window terminal input

API v23 preserves every earlier contract and adds the independent `process-terminal-input` mutation grant. Review the destination-sensitive program and enable external input explicitly before installing the sample:

```toml
[security]
allow_gui_control_input = true
```

```bash
termeric plugin validate examples/plugins/process-terminal-writer
termeric plugin install examples/plugins/process-terminal-writer \
  --allow process-spawn --allow process-query \
  --allow process-terminal-input --allow event-subscribe \
  --allow ui-actions --enable
```

The sample contributes **Process Terminal Writer: Draft Text in Sibling Pane**. It deliberately pastes safe text without Enter. After requesting the API v20 inventory and choosing one exact pane, it emits:

```json
{"type":"send-process-text","action_id":"draft-safe-text","window_id":7,"tab_id":12,"pane_id":3,"text":"hello from the process terminal writer","paste":true,"enter":false}
```

Only an API v23 persistent handler with the retained `process-terminal-input` grant may emit this action. The process-lifetime `window_id`, window-local `tab_id`, and tab-local `pane_id` must be exact. Termeric revalidates the originating plugin, generation, continuation, and authority, then resolves the complete destination path immediately before encoding and writing. A stale, missing, closed, closing, terminal-less, or read-only destination fails without fallback or partial input.

The destination window must independently have `security.allow_gui_control_input = true`; the default remains deny. Input is limited to 8 KiB of UTF-8 containing printable characters, tab, and newline, and the request must include text or Enter. Paste encoding and the final Enter byte use the exact destination terminal's current mode snapshot. Raw bytes, key events, mouse input, signals, broadcast, and cross-process targets remain unavailable.

Every process-plugin write uses destructive-command detection. A match creates one request-scoped confirmation in the exact destination window and returns `disposition: "pending_confirmation"` without writing. It does not activate, raise, select, or focus that window/tab/pane. Confirming later writes only the captured bytes to the captured terminal owner; cancellation or stale ownership cannot retarget a replacement terminal.

A successful or pending write returns a bounded correlated result:

```json
{
  "schema_version": 2,
  "request_id": 46,
  "plugin_id": "process-terminal-writer",
  "event": {
    "type": "process-action-result",
    "action_id": "draft-safe-text",
    "action": "send-process-text",
    "ok": true,
    "process_id": 12345,
    "origin_window_id": 2,
    "window_id": 7,
    "tab_id": 12,
    "pane_id": 3,
    "input_bytes": 38,
    "wire_bytes": 50,
    "paste": true,
    "enter": false,
    "disposition": "sent",
    "confirmation_id": null,
    "error": null
  }
}
```

`input_bytes` describes accepted UTF-8 plus a requested Enter; `wire_bytes` truthfully includes any bracketed-paste framing. It does not claim that a shell parsed or completed a command. Failure preserves correlation and exact target IDs while returning null byte/disposition fields plus a bounded control-safe error and `error_truncated` flag. The input result clears every host query and mutation authority, so it cannot write, read, navigate, or query again. Existing API v17 `terminal-input` remains exact-current-window-only and unchanged.

## Manifest API v24 cross-window ordinary-tab creation

API v24 preserves every earlier contract and adds the independent `process-tab-create` mutation grant. Review and install the sample with:

```bash
termeric plugin validate examples/plugins/process-tab-builder
termeric plugin install examples/plugins/process-tab-builder \
  --allow process-spawn --allow process-query \
  --allow process-tab-create --allow event-subscribe \
  --allow ui-actions --enable
```

The sample contributes **Process Tab Builder: New Tab in Another Window**. After requesting the API v20 inventory and choosing one exact sibling window, it emits:

```json
{"type":"create-process-tab","action_id":"new-sibling-task","window_id":7}
```

Only an API v24 persistent handler with the retained `process-tab-create` grant may emit this action. `window_id` is the process-lifetime stable identity returned by API v20. Termeric revalidates the originating plugin, generation, continuation, exact authority, and target window immediately before creation. It never substitutes the focused, active, originating, or most recently used window when the ID is stale, missing, closing, or uninitialized.

The operation uses the same ordinary local-shell tab path as current-window API v13 creation. It enforces the per-window and process tab caps and refuses creation while the target window's selected tab belongs to daemon-authoritative live-domain topology. On success the new tab is selected inside the target window, but the native window is not raised, shown, restored, or activated. The derived `tab-created` lifecycle hook is suppressed so a plugin cannot create a feedback loop from its own mutation.

Success returns a bounded correlated result to the originating handler:

```json
{
  "schema_version": 2,
  "request_id": 47,
  "plugin_id": "process-tab-builder",
  "event": {
    "type": "process-action-result",
    "action_id": "new-sibling-task",
    "action": "create-process-tab",
    "ok": true,
    "process_id": 12345,
    "origin_window_id": 2,
    "window_id": 7,
    "tab_id": 12,
    "selected": true,
    "error": null
  }
}
```

A failure returns `ok: false`, the same action and window correlation, null `tab_id`/`selected` fields, and a control-safe bounded error with `error_truncated`. At most one host operation is accepted per response. An API v24 process-query result may retain the separately granted tab-creation authority; the process-action result clears every query and mutation authority, so it cannot create a second tab or initiate another host operation. API v13 `tab-create` remains exact-originating-window-only and unchanged. API v24 does not grant pane creation, custom launch commands, cross-window closure or topology, native activation, or live-domain tab mutation.

## Manifest API v25 cross-window ordinary-pane creation

API v25 preserves every earlier contract and adds the independent `process-pane-create` mutation grant. Review and install the sample with:

```bash
termeric plugin validate examples/plugins/process-pane-builder
termeric plugin install examples/plugins/process-pane-builder \
  --allow process-spawn --allow process-query \
  --allow process-pane-create --allow event-subscribe \
  --allow ui-actions --enable
```

The sample contributes **Process Pane Builder: Split Pane in Another Window**. After requesting the API v20 inventory and choosing one exact sibling pane, it emits:

```json
{"type":"create-process-pane","action_id":"split-sibling-task","window_id":7,"tab_id":12,"pane_id":3,"direction":"vertical"}
```

Only an API v25 persistent handler with the retained `process-pane-create` grant may emit this action. `window_id` is the process-lifetime stable identity returned by API v20, `tab_id` is local to that window, and `pane_id` is local to that tab. `direction` is exactly `horizontal` or `vertical`. Termeric revalidates the originating plugin, generation, continuation, exact authority, and complete destination path immediately before mutation. It never substitutes an active or focused window, tab, or pane when any identity is stale.

The operation reuses API v12's ordinary split path, including the destination tab's launch/cwd inheritance, renderer geometry, terminal ownership, and per-tab/process pane ceilings. Closing or missing paths, live-domain and serial tabs, terminal-less or renderer-less unsplit tabs, and capacity failures leave the existing layout and ownership unchanged. Success may focus the newly created pane inside its target tab according to ordinary split semantics, but it does not select a different tab or raise, show, restore, or activate the native window.

Success returns a bounded correlated result to the originating handler:

```json
{
  "schema_version": 2,
  "request_id": 48,
  "plugin_id": "process-pane-builder",
  "event": {
    "type": "process-action-result",
    "action_id": "split-sibling-task",
    "action": "create-process-pane",
    "ok": true,
    "process_id": 12345,
    "origin_window_id": 2,
    "window_id": 7,
    "tab_id": 12,
    "pane_id": 3,
    "direction": "vertical",
    "created_pane_id": 4,
    "error": null
  }
}
```

A failure returns `ok: false`, the same action and destination correlation, null `created_pane_id`, and a control-safe bounded error with `error_truncated`. At most one host operation is accepted per response. An API v25 process-query result may retain the separately granted pane-creation authority; the process-action result clears every query and mutation authority, so it cannot split a second pane or initiate another host operation. API v12 `pane-create` remains exact-originating-window-only and unchanged. API v25 does not grant arbitrary commands, tab creation, pane movement/closure/layout presets, native activation, or live-domain mutation.

## Manifest API v26 cross-window ordinary-pane movement

API v26 preserves every earlier contract and adds the independent `process-pane-move` mutation grant. Review and install the sample with:

```bash
termeric plugin validate examples/plugins/process-pane-mover
termeric plugin install examples/plugins/process-pane-mover \
  --allow process-spawn --allow process-query \
  --allow process-pane-move --allow event-subscribe \
  --allow ui-actions --enable
```

The sample contributes **Process Pane Mover: Move Pane Between Windows**. It first requests the API v20 inventory, selects a movable ordinary source pane and an exact sibling-window target, then emits:

```json
{"type":"move-process-pane","action_id":"move-sibling-task","source_window_id":2,"source_tab_id":5,"pane_id":1,"destination_window_id":7,"destination_tab_id":12,"target_pane_id":3,"direction":"vertical"}
```

Only an API v26 persistent handler with retained `process-pane-move` authority may emit this action. Window IDs are process-lifetime stable; tab IDs are local to their windows and pane IDs are local to their tabs. Direction is exactly `horizontal` or `vertical`. Termeric revalidates the plugin generation, continuation, permission, complete source path, and complete destination path immediately before mutation. It never substitutes focused or active targets.

The action uses the same atomic ordinary-pane transfer as the authenticated GUI control surface. Validation completes before detach, the source tab must retain another pane, and live-domain, serial, terminal-less, closed, closing, missing, or uninitialized endpoints are refused. Success preserves the existing PTY, child process, title, history, launch state, and relevant terminal state while attaching it under a fresh destination-local pane ID. The destination pane is focused inside its tab, but no tab or native window is activated.

Success returns a bounded correlated result:

```json
{
  "schema_version": 2,
  "request_id": 49,
  "plugin_id": "process-pane-mover",
  "event": {
    "type": "process-action-result",
    "action_id": "move-sibling-task",
    "action": "move-process-pane",
    "ok": true,
    "process_id": 12345,
    "origin_window_id": 2,
    "source_window_id": 2,
    "source_tab_id": 5,
    "source_pane_id": 1,
    "destination_window_id": 7,
    "destination_tab_id": 12,
    "destination_pane_id": 4,
    "target_pane_id": 3,
    "direction": "vertical",
    "error": null
  }
}
```

Failure preserves the same correlation, returns a null `destination_pane_id`, and includes a bounded control-safe error plus `error_truncated`. At most one host operation is accepted per response. A process-query result may retain the separate movement grant, but the movement result removes every host authority. API v16 `pane-move` remains originating-window-only and unchanged. API v26 does not grant native activation, pane/tab/window closure, whole-tab cross-window movement, arbitrary launch, arbitrary layout trees, or live-domain mutation.

## Manifest API v27 cross-window ordinary-tab movement

API v27 adds the independent `process-tab-move` mutation grant. Review and install the sample with:

```bash
termeric plugin validate examples/plugins/process-tab-mover
termeric plugin install examples/plugins/process-tab-mover \
  --allow process-spawn --allow process-query \
  --allow process-tab-move --allow event-subscribe \
  --allow ui-actions --enable
```

After a bounded process inventory, the sample emits one exact operation:

```json
{"type":"move-process-tab","action_id":"move-sibling-tab","source_window_id":2,"tab_id":5,"destination_window_id":7,"destination_index":1}
```

Only an API v27 persistent handler with retained `process-tab-move` authority may emit it. Termeric revalidates the exact process-lifetime source and destination window IDs, source window-local tab ID, destination index, plugin generation, continuation, and permission immediately before mutation. There is no active/focused target fallback.

Same-window use performs an exact non-activating reorder. Cross-window use reuses the authenticated control surface's validation-before-detach transfer, including pinned-order constraints and live-domain refusal. It preserves the tab's live PTYs, pane tree, plugin-tab state, title, history, launches, and pinning, then returns a fresh destination-local tab ID. It never activates a native window. Unlike the broader control primitive, the plugin operation refuses to move the final non-closing source tab, preventing this movement-only grant from implicitly closing a window.

A success result reports `action: "move-process-tab"`, the original source correlation, `destination_window_id`, fresh `destination_tab_id`, final `destination_index`, `changed`, and `source_window_closing: false`. Failure returns null destination/change/closing fields plus a bounded control-safe error and `error_truncated`, without detaching the source. Every result clears all host authority. API v8 `move-tab` remains originating-window-only. API v27 does not grant tab/window closure, creation, native activation, arbitrary launches, live-domain mutation, or pane/layout mutation.

## Manifest API v28 cross-window ordinary-pane closure

API v28 adds the independent `process-pane-close` mutation grant. Review and install the sample with:

```bash
termeric plugin validate examples/plugins/process-pane-pruner
termeric plugin install examples/plugins/process-pane-pruner \
  --allow process-spawn --allow process-query \
  --allow process-pane-close --allow event-subscribe \
  --allow ui-actions --enable
```

After a bounded process inventory, the sample emits one exact operation:

```json
{"type":"close-process-pane","action_id":"close-sibling-pane","window_id":7,"tab_id":12,"pane_id":3}
```

Only an API v28 persistent handler with retained `process-pane-close` authority may emit it. Termeric revalidates the plugin generation, continuation, permission, stable process-owned window ID, window-local tab ID, and tab-local pane ID immediately before mutation. There is no active or focused target fallback.

The operation reuses API v14's ordinary-pane close path. It refuses the final pane, closing or missing targets, uninitialized tabs, and live-domain or serial tabs. A two-pane layout safely collapses back to its remaining terminal owner. The containing tab and window are never closed, selected, raised, or activated, and no derived lifecycle event is dispatched.

A success result reports `action: "close-process-pane"`, the exact target IDs, `remaining_panes`, and `focused_pane_id`. Failure reports null result fields plus a bounded control-safe `error` and `error_truncated`, without changing the target. Every result clears all host authority. API v14 `close-pane` remains originating-window-only. API v28 does not grant tab/window closure, creation, native activation, arbitrary launches, live-domain mutation, or broader topology/layout mutation.

## Manifest API v29 cross-window ordinary-tab closure

API v29 adds the independent `process-tab-close` mutation grant. Review and install the sample with:

```bash
termeric plugin validate examples/plugins/process-tab-pruner
termeric plugin install examples/plugins/process-tab-pruner \
  --allow process-spawn --allow process-query \
  --allow process-tab-close --allow event-subscribe \
  --allow ui-actions --enable
```

After a bounded process inventory, the sample emits one exact operation:

```json
{"type":"close-process-tab","action_id":"close-sibling-tab","window_id":7,"tab_id":12}
```

Only an API v29 persistent handler with retained `process-tab-close` authority may emit it. Termeric revalidates plugin generation, continuation, permission, stable process-owned window ID, and window-local tab ID immediately before mutation. There is no active or focused target fallback.

The operation reuses API v15's plugin-safe ordinary-tab close path. It refuses the final non-closing tab, closing or missing targets, uninitialized windows, and live-domain tabs. Normal asynchronous teardown is scheduled without closing the containing window, activating its native window, or dispatching a derived `tab-closed` hook.

A success result reports `action: "close-process-tab"`, the target IDs, `remaining_tabs`, `active_tab_id`, and `active_changed`. Failure reports null result fields plus a bounded control-safe `error` and `error_truncated`, without changing the target. Every result clears all host authority. API v15 `close-tab` remains originating-window-only, and authenticated control retains its separate lifecycle semantics. API v29 does not grant window closure, creation, native activation, arbitrary launches, live-domain mutation, or broader topology/layout mutation.

## Manifest API v30 cross-window ordinary-pane layouts

API v30 adds the independent `process-pane-layout` mutation grant. Review and install the sample with:

```bash
termeric plugin validate examples/plugins/process-pane-arranger
termeric plugin install examples/plugins/process-pane-arranger \
  --allow process-spawn --allow process-query \
  --allow process-pane-layout --allow event-subscribe \
  --allow ui-actions --enable
```

After a bounded process inventory, a persistent handler may emit exactly one of these operations:

```json
{"type":"swap-process-panes","action_id":"swap-panes","window_id":7,"tab_id":12,"first_pane_id":3,"second_pane_id":4}
{"type":"balance-process-panes","action_id":"balance-panes","window_id":7,"tab_id":12}
{"type":"apply-process-pane-layout","action_id":"grid-panes","window_id":7,"tab_id":12,"preset":"grid"}
```

`preset` is exactly `columns`, `rows`, `grid`, `tall`, or `fat`, and a swap requires two distinct pane IDs. Only an API v30 persistent handler with retained `process-pane-layout` authority may emit these actions. Termeric revalidates plugin generation, continuation authority, stable process-owned window ID, window-local tab ID, and pane IDs immediately before mutation; it never falls back to an active or focused target.

The operations reuse API v18's ordinary split-layout path. They preserve live terminal owners, stable pane IDs, focused pane, and active-tab selection. Closing, missing, unsplit, live-domain, and serial targets fail without mutation. Applying a preset retains API v18's zoom behavior, and none of the operations activates or raises a native window or dispatches lifecycle hooks.

Success reports the exact target, resulting `pane_order`, `focused_pane_id`, and `zoomed_pane_id`; balancing also reports `changed`. Failure returns null result fields plus a bounded control-safe `error` and `error_truncated`. Every result clears all host authority. API v18 `pane-layout` remains originating-window-only. API v30 does not grant pane creation, movement, or closure; tab/window mutation; arbitrary layout trees; live-domain layout mutation; native activation; or arbitrary launches.

## Manifest API v31 exact process-window closure

API v31 adds the independent `process-window-close` mutation grant. Review and install the sample with:

```bash
termeric plugin validate examples/plugins/process-window-closer
termeric plugin install examples/plugins/process-window-closer \
  --allow process-spawn --allow process-query \
  --allow process-window-close --allow event-subscribe \
  --allow ui-actions --enable
```

After a bounded process inventory, a persistent handler may emit exactly one operation:

```json
{"type":"close-process-window","action_id":"close-sibling-window","window_id":7}
```

Only an API v31 persistent handler with retained `process-window-close` authority may emit it. Termeric revalidates plugin generation, continuation authority, the exact process-lifetime window ID, non-closing and initialized tab-owner state, and the absence of another pending plugin closure. There is no fallback to an active, focused, newest, or replacement window.

At least one other live process-owned window without pending teardown must survive. The process-wide grant therefore cannot close the final live window; API v19 remains the explicit command-scoped mechanism for closing the invoking window when application termination is intended.

A success result reports `action: "close-process-window"`, the exact origin and target IDs, `target_is_origin`, `scheduled`, `tab_count`, and `remaining_windows`. Failure reports `scheduled: false`, null count fields, and a bounded control-safe `error` plus `error_truncated`, without mutation. Termeric submits this result before arming teardown. A sibling target is then marked for ordinary teardown independently of the surviving origin. When the target is the origin, only the matching plugin ID, generation, and result request callback completes the close; callback failure still completes an already accepted schedule, while unrelated callbacks cannot.

Every result clears all host authority. The operation never activates, raises, focuses, minimizes, or retargets a native window and does not dispatch a derived plugin lifecycle event. It grants no tab/pane closure, process navigation, native activation, arbitrary lifecycle control, or implicit final- window termination. API v19 `window-close` remains unchanged and current- window command-scoped.

## Manifest API v32 exact process-window focus requests

API v32 adds the independent `process-window-focus` mutation grant. Review and install the sample with:

```bash
termeric plugin validate examples/plugins/process-window-focuser
termeric plugin install examples/plugins/process-window-focuser \
  --allow process-spawn --allow process-query \
  --allow process-window-focus --allow event-subscribe \
  --allow ui-actions --enable
```

After a bounded process inventory, a persistent handler may emit exactly one operation:

```json
{"type":"focus-process-window","action_id":"focus-sibling-window","window_id":7}
```

Only an API v32 persistent handler with retained `process-window-focus` authority may emit it. Termeric resolves only the supplied process-lifetime window ID and revalidates plugin generation, continuation authority, target existence, non-closing state, and native-window ownership. It never substitutes the originating, currently focused, newest, or only window.

The operation reuses authenticated `control focus-window` policy. An already- focused target succeeds without another native request. A supported backend receives one native focus request; hidden and minimized targets fail rather than being restored. Wayland fails explicitly because the compositor controls activation. A successful request means only that Termeric submitted it—not that the desktop transferred focus. Scripts may use a later process inventory to observe the reported focus state.

A success result reports `action: "focus-process-window"`, exact origin and target IDs, `already_focused`, and `requested`. Failure reports `requested: false`, null `already_focused`, and a bounded control-safe `error` plus `error_truncated`, without fallback or unrelated mutation. Every result clears all host authority and emits no derived lifecycle event.

This grant does not select tabs or panes, unhide or unminimize windows, send an attention request, close or resize anything, reorder desktop windows, or grant arbitrary desktop control. Process tab/pane selection and window focus remain separate host operations and therefore cannot be combined in one response.

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
| `pane-move` | API v16+, after `--allow pane-move`; permits moving one exact ordinary pane between distinct tabs in the originating window through a persistent handler, preserves its live PTY, and returns the fresh destination-local identity. |
| `terminal-input` | API v17+, after `--allow terminal-input`; permits at most 8 KiB of bounded UTF-8 text to one exact current-window terminal through a persistent handler, additionally requires `security.allow_gui_control_input = true`, and preserves destructive-command confirmation. |
| `pane-layout` | API v18+, after `--allow pane-layout`; permits exact same-tab pane swap, divider balancing, and columns/rows/grid/tall/fat presets for one ordinary current-window split through a persistent handler, preserving live terminal owners and focus. |
| `window-close` | API v19+, after `--allow window-close`; permits only a declared command invocation to schedule closure of its exact originating window through a persistent handler, after delivery of one correlated final result. |
| `process-query` | API v20+, after `--allow process-query`; exposes one bounded, creation-ordered read-only inventory of the live windows owned by this Termeric process, including the exact originating stable window ID, through a persistent handler. |
| `process-actions` | API v21+, after `--allow process-actions`; permits one exact non-activating tab selection or pane focus in any live process-owned window through a persistent handler, with atomic full-path validation and a correlated non-chainable result. |
| `process-terminal-text` | API v22+, after `--allow process-terminal-text`; permits one exact, non-activating read of at most 256 history lines and 8 KiB of presentation-safe UTF-8 text from any live process-owned pane through a persistent handler, with a correlated non-chainable result. |
| `process-terminal-input` | API v23+, after `--allow process-terminal-input`; permits one exact, non-activating write of at most 8 KiB of validated UTF-8 to any live process-owned pane through a persistent handler, additionally requiring the destination window's disabled-by-default external-input policy and preserving destructive confirmation. |
| `process-tab-create` | API v24+, after `--allow process-tab-create`; permits creation and in-window selection of one ordinary local shell tab in one exact live process-owned window through a persistent handler, without native activation or lifecycle redispatch. |
| `process-pane-create` | API v25+, after `--allow process-pane-create`; permits one exact horizontal or vertical ordinary-pane split in one exact live process-owned window through a persistent handler, preserving ordinary launch/cwd inheritance without native activation. |
| `process-pane-move` | API v26+, after `--allow process-pane-move`; permits moving one exact ordinary pane between exact live process-owned tabs or windows through a persistent handler, atomically preserving its live PTY and returning a fresh destination-local identity without native activation. |
| `process-tab-move` | API v27+, after `--allow process-tab-move`; permits exact same-window tab reordering or movement of one exact ordinary tab into an exact process-owned destination window, preserving complete live tab state without native activation or implicit source-window closure. |
| `process-pane-close` | API v28+, after `--allow process-pane-close`; permits closure of one exact ordinary pane in an exact live process-owned window while preserving its containing tab/window and refusing the final pane, without selection or native activation. |
| `process-tab-close` | API v29+, after `--allow process-tab-close`; permits scheduled closure of one exact ordinary tab in an exact live process-owned window while preserving the window and refusing the final non-closing tab, without native activation or lifecycle feedback. |
| `process-pane-layout` | API v30+, after `--allow process-pane-layout`; permits exact pane swap, divider balancing, or columns/rows/grid/tall/fat presets in one exact ordinary split tab in an exact live process-owned window, preserving terminal identity, focus, and active-tab selection without native activation. |
| `process-window-close` | API v31+, after `--allow process-window-close`; permits result-gated closure of one exact non-final live process-owned window through a persistent handler, without identity fallback, native activation, or implicit application termination. |
| `process-window-focus` | API v32+, after `--allow process-window-focus`; permits one truthful native focus request for an exact live process-owned window through the existing authenticated control policy, without identity fallback, restoration, or compositor-confirmation claims. |
| `secrets` | Reserved and denied; there is no secret API or manifest interpolation. |

Requested and granted sets are independent. Installation may retain a disabled manifest with denied capabilities for inspection, but enabling fails until all requests are explicitly granted. API v1 refuses every capability except `process-spawn`; API v2 refuses every capability except `process-spawn` and `workspace-query`; API v3 additionally permits `event-subscribe` and `ui-actions`; API v4 through v32 may also request `terminal-text` for `command-completed`. API v5 and v6 add no new grant; API v6 gives the existing `workspace-query` grant a separately gated live-query operation. API v7 adds only `workspace-actions`, independently of query, UI, or terminal-text grants. API v8 similarly adds only the independent `workspace-topology` grant. API v9 adds no grant; handler commands reuse `event-subscribe` and whatever separate response/disclosure permissions the manifest already requests. API v10 also adds no grant; typed settings are explicit user-authored input and are always visible through plugin diagnostics. API v11 adds only `terminal-query`, independently of `terminal-text`, workspace mutation, UI, filesystem, network, clipboard, or secret access. API v12 adds only `pane-create`, independently of query, navigation, topology, terminal text, UI, filesystem, network, clipboard, or secret access. API v13 adds only `tab-create`, independently of pane creation, query, navigation, topology, terminal text, UI, filesystem, network, clipboard, or secret access. API v14 adds only `pane-close`, independently of pane/tab creation, query, navigation, topology, terminal text, UI, filesystem, network, clipboard, or secret access. API v15 adds only `tab-close`, independently of pane/tab creation, pane close, query, navigation, topology, terminal text, UI, filesystem, network, clipboard, or secret access. API v16 adds only `pane-move`, independently of creation, closure, query, navigation, topology, terminal text, UI, filesystem, network, clipboard, or secret access. API v17 adds only `terminal-input`, independently of query, navigation, topology, terminal text, creation, closure, movement, UI, filesystem, network, clipboard, or secret access. API v18 adds only `pane-layout`, independently of query, navigation, topology, terminal text/input, creation, closure, movement, UI, filesystem, network, clipboard, or secret access. API v19 adds only `window-close`, independently of query, navigation, topology, terminal text/input, pane/tab lifecycle, movement, layout, UI, filesystem, network, clipboard, or secret access. Runtime authority is further limited to exact-owner declared command invocations. API v20 adds only `process-query`, independently of current-window query, terminal text/input, navigation, topology, lifecycle, movement, layout, UI, filesystem, network, clipboard, or secret access. API v21 adds only `process-actions`, independently of process/current-window query, terminal text/input, native activation, topology, lifecycle, movement, layout, creation, closure, UI, filesystem, network, clipboard, or secret access. API v22 adds only `process-terminal-text`, independently of process/current- window inventory, navigation, terminal input, native activation, topology, lifecycle, movement, layout, creation, closure, UI, filesystem, network, clipboard, or secret access. API v23 adds only `process-terminal-input`, independently of process/current- window inventory, navigation, terminal text, native activation, topology, lifecycle, movement, layout, creation, closure, UI, filesystem, network, clipboard, or secret access. Runtime execution additionally requires the destination window's external-input policy and destructive confirmation. API v24 adds only `process-tab-create`, independently of process/current-window inventory, navigation, terminal text/input, native activation, pane creation, custom launches, topology, closure, movement, layout, UI, filesystem, network, clipboard, or secret access. API v25 adds only `process-pane-create`, independently of process/current- window inventory, navigation, terminal text/input, native activation, tab creation, arbitrary launches, movement, closure, broader topology/layout, UI, filesystem, network, clipboard, or secret access. API v26 adds only `process-pane-move`, independently of process/current-window inventory, navigation, terminal text/input, native activation, creation, closure, arbitrary layout/topology, UI, filesystem, network, clipboard, or secret access. API v27 adds only `process-tab-move`, independently of process/current-window inventory, navigation, terminal text/input, native activation, lifecycle, creation, pane/layout mutation, UI, filesystem, network, clipboard, or secret access. API v28 adds only `process-pane-close`, independently of process/current-window inventory, navigation, terminal text/input, native activation, lifecycle, tab or window closure, creation, movement, layout, UI, filesystem, network, clipboard, or secret access. API v29 adds only `process-tab-close`, independently of process/current-window inventory, navigation, terminal text/input, native activation, lifecycle, window closure, creation, movement, layout, UI, filesystem, network, clipboard, or secret access. API v30 adds only `process-pane-layout`, independently of process/current-window inventory, navigation, terminal text/input, native activation, lifecycle, pane/tab/window creation, movement, or closure, arbitrary layout trees, UI, filesystem, network, clipboard, or secret access. API v31 adds only `process-window-close`, independently of process/current- window inventory, navigation, terminal text/input, native activation, pane/tab mutation, final-window termination, arbitrary lifecycle control, UI, filesystem, network, clipboard, or secret access. API v32 adds only `process-window-focus`, independently of process/current- window inventory, tab/pane navigation, terminal text/input, window restoration, attention requests, lifecycle/topology mutation, UI, filesystem, network, clipboard, or secret access. Backend acceptance is a native focus request, not proof of compositor focus transfer. Updates that add a permission do not silently migrate the old grant or replace the working installed copy, even while the plugin is disabled; the operator must use the explicit reviewed reinstall/replace flow.

`process-spawn` is intentionally a powerful grant: the selected executable is a normal user process and may have the ambient filesystem, environment, and network access that the operating system gives it. Termeric passes argv as separate values and does not invoke a shell unless the manifest explicitly selects one, but API permissions are not an OS sandbox. Do not grant launch permission to an untrusted manifest or executable.

Profile hooks perform only bounded in-memory lookup and action construction. A selected profile executable is invoked exactly once as the ordinary PTY child; Termeric does not synchronously probe it with `--version` or another side-effecting command. API v3+ handler programs run only on the bounded worker described above, never in Termeric's UI/PTY/render threads. A handler executable is still ordinary native code with the user's OS authority despite its cleared environment; the API permission model is disclosure/action gating, not a kernel sandbox. WASM/in-process modules, network hooks supplied by Termeric, raw PTY or continuous terminal-output subscriptions, raw-byte input, and render actions remain unsupported. API v5+'s persistent newline protocol is deliberately narrower than a bidirectional JSON-RPC service.

## Author testing and upgrades

Before distributing a local directory:

1. Run `plugin validate` and inspect the source/fingerprint/permission output.
2. Install disabled first, then inspect `plugin show ID --json`.
3. Enable only with the exact requested grant and invoke every palette profile.
4. Test missing executables, immediate child failure, a long-running child, Unicode argv, active/inherited cwd, workspace save/restore, disable, and remove.
5. Change the version and profile set in the source, run `plugin update`, and verify configured/default and archived named profiles. Removed configured profiles are cleared; removed archived profiles fail visibly at restore.
6. Add a reserved capability in a fixture and prove the update/enable is denied.
7. For API v3+, feed every subscribed event to the handler over stdin; test an empty response, every allowed UI action, malformed/oversized output, nonzero exit, timeout, queue pressure, reload while pending, and operation without `workspace-query` or `ui-actions`.
8. For API v4, test successful and failed multiline commands, split-pane source IDs, long/control-bearing text, evicted regions, and denial without the exact `terminal-text` grant. Treat captured command/output as sensitive data.
9. For API v5, send multiple requests through one process and test retained state, exact ID echo, crash and restart, timeout, oversized/unterminated lines, stderr flooding, queue/process ceilings, reload/disable teardown, and behavior when `mode` is omitted.
10. For API v6, test granted and denied `query-workspace` responses, logical and transport correlation, asynchronous interleaving, topology and metadata clipping, control-bearing titles/IDs, stale generation refusal, and the no-query-from-result rule. Treat cwd, titles, workspace IDs, and domain IDs as potentially sensitive metadata.
11. For API v7, test granted and denied `select-tab`/`focus-pane`, invalid and closing stable IDs, query-to-action correlation, one-operation rejection, action-result authority removal, lifecycle-hook suppression, stale plugin generations, and interleaved subscribed events.
12. For API v8, test granted and denied `move-tab`/`resize-divider`, pinned and live-domain refusal, invalid/stale IDs, index and ratio bounds, idempotence, active-focus preservation, one-operation rejection, result authority removal, and interleaved subscribed events.
13. For API v9, test palette discovery, exact namespace routing, duplicate and profile-generated command conflicts, undeclared IDs, both handler modes, missing subscriptions/grants, live reload, and local IDs matching a compiled-in command suffix without invoking the built-in action.
14. For API v10, test every setting type/default/bound, set/unset/reset, effective-value diagnostics, profile and handler delivery, live reload, registry type tampering, compatible update preservation, and refusal of removed or narrowed configured values.
15. For API v11, test exact grant/version/mode denial, workspace-to-terminal correlation, invalid/closing stable IDs, primary and alternate screens, hidden synchronized output, UTF-8/JSON clipping, history/byte limits, one-host-operation rejection, and complete authority removal after the result. Treat all returned terminal text as potentially sensitive.
16. For API v12, test exact grant/version/mode denial, typed split direction, stale and unsupported stable-ID targets, workspace-to-create correlation, one-host-operation rejection, returned pane identity, inherited launch/cwd policy, and complete authority removal after the result.
17. For API v13, test exact grant/version/mode denial, action-ID and schema bounds, current-window stable identity, unavailable/live/capacity/launch failures, lifecycle-hook suppression, one-host-operation rejection, and complete authority removal after the result.
18. For API v14, test exact grant/version/mode denial, strict action shape, stale/closed/closing/unsupported stable-ID targets, non-active target focus preservation, split collapse and last-pane refusal, one-host-operation rejection, correlated result fields, and complete authority removal.
19. For API v15, test exact grant/version/mode denial, strict action shape, stale/closed/closing/live-domain stable-ID refusal, local serial acceptance, background/active selection behavior, final-non-closing-tab protection, scheduled result fields, copy-mode cleanup, lifecycle-hook suppression, one-host-operation rejection, and complete authority removal.
20. For API v16, test exact grant/version/mode denial, strict action shape and direction, complete stable-ID source/destination validation, same-tab, missing, closed, closing, last-pane, live-domain, serial-destination, plugin-owned-source, invalid-anchor, and capacity refusal, live PTY and launch-identity preservation, fresh destination-local identity, focus and mode cleanup, one-host-operation rejection, and complete authority removal.
21. For API v17, test exact grant/version/mode and global-policy denial, strict action shape, UTF-8/control/8-KiB bounds, exact current-window stable-ID routing, missing/closed/closing/terminal-less/read-only refusal, live bracketed-paste encoding, no focus change, destructive input confirmation for ordinary destinations, exact confirmation cancellation/ownership, correlated byte/disposition fields, one-host-operation rejection, and complete authority removal.
22. For API v18, test exact grant/version/mode denial, all three strict action shapes, invalid/duplicate/stale IDs and presets, background-tab targeting, unsplit/closed/closing/live-domain/serial refusal, PTY/focus/zoom identity, divider balance idempotence, preset geometry, one-host-operation rejection, correlated state fields, and complete result-authority removal.
23. For API v19, test exact grant/version/mode/declared-command denial, command-only authority, strict targetless action shape and action-ID bounds, unavailable/already-closing/duplicate refusal, no close before result submission, exact plugin/generation/request correlation, stale and unloaded final callback handling, final success/error teardown, sibling preservation, one-host-operation rejection, and complete result-authority removal.
24. For API v20, test exact grant/version/mode denial, strict query shape and identifier bounds, current-process stable window identity and creation order, exact origin identity, closing-window exclusion, aggregate window/tab/pane/divider/metadata and frame ceilings, control-safe UTF-8 clipping, one-host-operation rejection, and complete result-authority removal. Treat titles, workspace IDs, and domain IDs as sensitive metadata.
25. For API v21, test exact grant/version/mode denial, both strict action shapes, malformed IDs, duplicate and mixed host-operation rejection, same- and sibling-window routing, atomic invalid tab/pane refusal, stale/closed/closing/uninitialized targets, non-activation, hook suppression, correlated bounded errors and change fields, process-query authority reduction, and complete process-action result authority removal.
26. For API v22, test exact grant/version/mode denial, strict query shape, malformed IDs, history/byte bounds, duplicate and mixed host-operation rejection, same- and sibling-window reads, primary/alternate screen and hidden synchronized-output behavior, stale/closed/closing/uninitialized targets, UTF-8/JSON clipping, non-activation, correlated bounded errors, process-query authority reduction, and complete terminal-result authority removal. Treat every returned text snapshot as potentially sensitive.
27. For API v23, test exact grant/version/mode and destination-policy denial, strict input shape and 8-KiB/control bounds, malformed IDs, duplicate and mixed host-operation rejection, same- and sibling-window exact routing, stale/closed/closing/terminal-less/read-only targets, destination-mode paste/Enter encoding, destructive confirmation ownership and cancellation, non-activation/focus preservation, bounded correlated results, process-query authority reduction, and complete input-result authority removal. Use inert destinations and never execute destructive fixtures.
28. For API v24, test exact grant/version/mode denial, strict action shape and action-ID bounds, malformed window IDs, duplicate and mixed host-operation rejection, same- and sibling-window exact routing, stale/closing/ uninitialized/live-domain/capacity/launch refusal, ordinary local-shell identity, in-window selection without native activation, lifecycle-hook suppression, bounded correlated results, process-query authority reduction, and complete process-action result authority removal.
29. For API v25, test exact grant/version/mode denial, strict action shape, malformed IDs/directions, duplicate and mixed host-operation rejection, same- and sibling-window exact routing, stale/closed/closing/ uninitialized/live-domain/serial/terminal-less/renderer-less/capacity refusal, atomic layout/PTY preservation, ordinary launch/cwd inheritance, destination focus without tab/native activation, bounded correlated results, process-query authority reduction, and complete process-action result authority removal.
30. For API v26, test exact grant/version/mode denial, strict action shape and action-ID bounds, malformed source/destination IDs and directions, duplicate and mixed host-operation rejection, same- and sibling-window exact routing, stale/closed/closing/uninitialized/live-domain/serial/ terminal-less/last-pane refusal, validation-before-detach atomicity, live PTY/title/history/launch preservation, fresh destination-local identity, destination focus without tab/native activation, bounded correlated results, process-query authority reduction, and complete result-authority removal.
31. For API v27, test exact grant/version/mode denial, strict action/index/ID shape, duplicate and mixed host-operation rejection, exact same-window reorder and sibling-window routing, stale/closing/uninitialized/live-domain/ pinning/index/final-source-tab refusal, validation-before-detach atomicity, full live tab/PTY/pane/plugin/title/history/launch/pinning preservation, fresh destination-local identity, non-activation, bounded correlated results, process-query authority reduction, and complete result-authority removal.
32. For API v28, test exact grant/version/mode denial, strict action/ID shape, duplicate and mixed host-operation rejection, exact same- and sibling- window routing, stale/closed/closing/uninitialized/live-domain/serial/ final-pane refusal, safe two-pane collapse, containing-tab/window preservation, non-selection and non-activation, bounded correlated results, process-query authority reduction, and complete result-authority removal.
33. For API v29, test exact grant/version/mode denial, strict action/ID shape, duplicate and mixed host-operation rejection, exact same- and sibling- window routing, stale/closed/closing/uninitialized/live-domain/final-tab refusal, containing-window preservation, lifecycle-hook suppression, non-activation, bounded correlated results, process-query authority reduction, and complete result-authority removal.
34. For API v30, test exact grant/version/mode denial, all three strict action shapes and preset values, distinct swap IDs, duplicate and mixed host- operation rejection, exact same- and sibling-window routing, stale/closed/ closing/uninitialized/unsplit/live-domain/serial refusal, terminal-owner, stable-ID, focus, active-tab, and zoom-policy preservation, non-activation, bounded correlated results, process-query authority reduction, complete result-authority removal, retained-grant updates, and the checked-in process pane arranger.
35. For API v31, test exact grant/version/mode denial, strict action/ID shape, duplicate and mixed host-operation rejection, same- and sibling-window exact routing, stale/missing/closing/uninitialized/already-pending/final- window refusal, result-before-teardown ordering, sibling independence, exact origin plugin/generation/request callback gating, callback failure, non-activation, bounded correlated results, process-query authority reduction, complete result-authority removal, retained-grant updates, and the checked-in process window closer.
36. For API v32, test exact grant/version/mode denial, strict action/ID shape, duplicate and mixed host-operation rejection, exact same-/sibling-window routing, stale/closing/native-less refusal, already-focused behavior, Wayland/hidden/minimized policy, one native request on supported backends, truthful request-versus-confirmation reporting, bounded correlated results, process-query authority reduction, complete result-authority removal, retained-grant updates, and the checked-in process window focuser.

Termeric's own tests cover schema/API mismatch, permission denial, forbidden capabilities, fingerprint and symlink substitution, administrative temporary file safety, resource ceilings, child crash/timeout isolation, profile/command conflicts, explicit upgrades, lifecycle operations, and exact workspace/command actions. API v3 adds hermetic local-process stdin/stdout, response-schema, disclosure, action-grant, and bounds coverage. API v4 adds cross-layer semantic region routing, source IDs, permission denial, stale-content refusal, UTF-8 and JSON bounds, control sanitization, and a checked-in command observer. API v5 adds stateful single-process reuse, correlation, restart, cancellation, framing, and process-count tests plus a checked-in persistent counter. API v6 adds independent permission denial, non-chainable query responses, bounded current- window snapshot, control sanitization, same-process round-trip, and checked-in workspace inspector coverage. API v7 adds exact permission/version/mode denial, query/action/result authority reduction, stable-ID success/failure routing, non-retargeting invalid targets, hook-loop suppression, same-process reuse, and a checked-in non-invasive workspace navigator. None of these tests launches a provider process. API v8 adds exact topology grant/version/mode denial, static index/ratio bounds, cross-kind one-operation enforcement, atomic reorder/resize success and failure routing, focus preservation, same-process correlation, and a checked-in non-invasive workspace organizer. API v9 adds manifest/version/subscription/resource validation, generated-ID conflicts, exact-owner manager routing, process-per-event and persistent invocation construction, third-party reserved-suffix isolation, and a checked- in command tool. No provider process is launched. API v10 adds typed schema and aggregate bounds, atomic override commands, default merging, profile/handler disclosure, status visibility, tamper refusal, compatible preservation, incompatible-update rollback, and a checked-in configurable command. These tests construct launch/invocation requests but do not execute the sample or a provider. API v11 adds exact permission/version/mode gating, response-schema and static limit validation, stable-ID success/failure routing, presentation-safe hidden- output exclusion, JSON-envelope clipping, same-process correlation, host- operation loop prevention, and a checked-in terminal reader. The tests use only in-memory terminals and inert local shell/Python fixtures; no provider runs. API v12 adds exact permission/version/mode gating, typed direction parsing, stable-ID creation and failure routing, ordinary/live/serial target distinction, same-process correlation, host-operation loop prevention, and a checked-in pane builder. Its PTYs run only inert local shell fixtures; no provider runs. API v13 adds exact permission/version/mode gating, strict action parsing, ordinary stable-ID creation, unavailable/live/capacity/launch failure routing, lifecycle-hook suppression, same-process correlation, authority removal, and a checked-in tab builder. Its PTYs run only inert local shell fixtures; no provider runs. API v14 adds exact permission/version/mode gating, strict stable-ID action parsing, ordinary/live/serial/closing/last-pane refusal, non-active targeting, split collapse, bounded correlated results, same-process query-to-close correlation, one-operation enforcement, authority removal, and a checked-in pane pruner. Its PTYs run only inert local shell fixtures; no provider runs. API v15 adds exact permission/version/mode gating, strict stable-ID action parsing, local serial acceptance, missing/live/closing/final-tab refusal, background focus preservation, active handoff, copy-mode cleanup, scheduled correlated results, lifecycle-hook suppression, same-process query-to-close correlation, one-operation enforcement, authority removal, and a checked-in tab pruner. Its PTYs run only inert local shell fixtures; no provider runs. API v16 adds exact permission/version/mode gating, typed strict source, destination, anchor, and direction parsing, atomic unsupported-target refusal, live PTY identity preservation, fresh destination-local identity, correlated source/focus state, copy-mode and broadcast cleanup, same-process query-to-move correlation, one-operation enforcement, authority removal, and a checked-in pane mover. Its PTYs run only inert local shell fixtures; no provider runs. API v17 adds exact permission/version/mode and disabled-by-default policy gating, strict control-safe input parsing and bounds, stable-ID success/failure routing, live bracketed-paste encoding, ordinary-tab destructive confirmation, focus preservation, correlated byte/disposition results, same-process query-to-input correlation, one-operation enforcement, authority removal, and a checked-in non-executing terminal writer. Its PTYs run only inert local shell fixtures; no provider runs or AI tokens are consumed. API v18 adds exact permission/version/mode gating, strict swap/balance/preset parsing, atomic unsupported-target refusal, background-tab routing, PTY/focus/ zoom identity preservation, balanced and rebuilt geometry, correlated state, same-process query-to-layout reuse, one-operation enforcement, authority removal, and a checked-in pane arranger. Its PTYs run only inert local shell fixtures; no provider runs or AI tokens are consumed. API v19 adds exact permission/version/mode/command gating, strict targetless closure parsing, command-only runtime authority, bounded scheduled results, submission-before-teardown behavior, exact final-callback correlation, stale/unloaded/error completion, duplicate refusal, one-operation enforcement, authority removal, and a checked-in window closer. All coverage is hermetic; no provider runs or AI tokens are consumed. API v20 adds exact permission/version/mode gating, strict process-query parsing, stable origin and creation-ordered process-window IDs, closing-window exclusion, aggregate inventory and encoded-frame ceilings, metadata sanitization, one- operation enforcement, complete query/mutation authority removal, retained- grant install/update behavior, and a checked-in process inspector. All coverage is hermetic; no provider runs or AI tokens are consumed. API v21 adds exact permission/version/mode gating, strict process-action parsing, same- and sibling-window stable-ID routing, atomic target validation, stale/closed/closing/uninitialized refusal, non-activation, lifecycle-hook suppression, bounded correlated results, query-to-action authority reduction, one-operation enforcement, complete action-result authority removal, retained- grant install/update behavior, and a checked-in process navigator. All coverage is hermetic; no provider runs or AI tokens are consumed. API v22 adds exact permission/version/mode gating, strict bounded query parsing, same- and sibling-window exact stable-ID reads, presentation-safe text extraction, stale/closing target refusal, non-activation, UTF-8 and encoded- frame ceilings, bounded correlated errors, query-to-text authority reduction, one-operation enforcement, complete result-authority removal, retained-grant install/update behavior, and a checked-in process terminal reader. Its tests use only in-memory terminals and inert local shell/Python fixtures; no provider runs or AI tokens are consumed. API v23 adds exact permission/version/mode and destination-policy gating, strict bounded input parsing, same- and sibling-window exact stable-ID writes, destination-mode paste/Enter encoding, stale/read-only target refusal, non-activation and focus preservation, request-scoped destructive confirmation, bounded correlated results, query-to-input authority reduction, one-operation enforcement, complete result-authority removal, retained-grant install/update behavior, and a checked-in process terminal writer. Its tests use only inert local terminals and shell/Python fixtures; no provider runs, destructive command executes, or AI tokens are consumed. API v24 adds exact permission/version/mode gating, strict bounded action parsing, exact stable-window routing, stale/closing/live-domain target refusal, ordinary local-shell creation and in-window selection without native activation, lifecycle-hook suppression, bounded correlated results, query-to-tab-creation authority reduction, one-operation enforcement, complete result-authority removal, retained-grant install/update behavior, and a checked-in process tab builder. Its tests use only inert local shell/Python fixtures; no provider runs or AI tokens are consumed. API v25 adds exact permission/version/mode gating, strict typed direction and stable-path parsing, same- and sibling-window exact routing, stale/closing/ live-domain/serial target refusal, atomic ordinary split creation, launch/cwd inheritance, fresh destination-local identity, non-activation, bounded correlated results, query-to-pane-creation authority reduction, one-operation enforcement, complete result-authority removal, retained-grant install/update behavior, and a checked-in process pane builder. Its tests use only inert local shell/Python fixtures; no provider runs or AI tokens are consumed. API v26 adds exact permission/version/mode gating, strict source/destination path and direction parsing, same- and sibling-window exact routing, atomic refusal before detach, live PTY and metadata preservation, fresh destination- local identity, non-activation, bounded correlated results, query-to-pane- movement authority reduction, one-operation enforcement, complete result- authority removal, retained-grant install/update behavior, and a checked-in process pane mover. Its tests use only inert local shell/Python fixtures; no provider runs or AI tokens are consumed. API v27 adds exact permission/version/mode gating, strict stable-path/index parsing, same- and sibling-window exact routing, final-source-tab and stale target refusal before detach, complete live tab-state preservation, fresh destination-local identity, non-activation, bounded correlated results, query-to-tab-movement authority reduction, one-operation enforcement, complete result-authority removal, retained-grant install/update behavior, and a checked-in process tab mover. Its tests use only inert local shell/Python fixtures; no provider runs or AI tokens are consumed. API v28 adds exact permission/version/mode gating, strict stable-path parsing, same- and sibling-window exact routing, final-pane and stale target refusal, safe ordinary-pane collapse, containing-tab/window preservation, non-selection/non-activation, bounded correlated results, query-to-pane-close authority reduction, one-operation enforcement, complete result-authority removal, retained-grant install/update behavior, and a checked-in process pane pruner. Its tests use only inert local shell/Python fixtures; no provider runs or AI tokens are consumed. API v29 adds exact permission/version/mode gating, strict stable-path parsing, same- and sibling-window exact routing, final-tab and stale target refusal, normal asynchronous close scheduling, containing-window preservation, lifecycle-hook suppression, non-activation, bounded correlated results, query-to-tab-close authority reduction, one-operation enforcement, complete result-authority removal, retained-grant install/update behavior, and a checked-in process tab pruner. Its tests use only inert local shell/Python fixtures; no provider runs or AI tokens are consumed. API v30 adds exact permission/version/mode gating, strict stable-path/action/ preset parsing, same- and sibling-window exact routing, stale and unsupported target refusal, ordinary layout and terminal-identity preservation, non-selection/non-activation, bounded correlated results, query-to-pane-layout authority reduction, one-operation enforcement, complete result-authority removal, retained-grant install/update behavior, and a checked-in process pane arranger. Its tests use only inert local shell/Python fixtures; no provider runs or AI tokens are consumed. API v31 adds exact permission/version/mode gating, strict stable-window/action parsing, same- and sibling-window routing, stale/final/pending target refusal, result-before-teardown ordering, exact origin callback gating, sibling independence, non-activation, bounded correlated results, query-to-window-close authority reduction, one-operation enforcement, complete result-authority removal, retained-grant install/update behavior, and a checked-in process window closer. Its tests use only inert local shell/Python fixtures; no provider runs or AI tokens are consumed. API v32 adds exact permission/version/mode gating, strict stable-window/action parsing, same- and sibling-window exact routing, stale/closing/native-less refusal, shared authenticated-control focus policy, already-focused and Wayland/hidden/minimized decisions, truthful requested-versus-confirmed result semantics, bounded correlated results, query-to-window-focus authority reduction, one-operation enforcement, complete result-authority removal, retained-grant install/update behavior, and a checked-in process window focuser. Its tests use only inert local shell/Python fixtures; no provider runs or AI tokens are consumed. Native compositor observation remains separate operator acceptance.

Install also rejects a symlinked plugin root. Removal first atomically renames a real installation into a same-root quarantine, then commits the registry change; a registry-write failure restores the original directory. Cleanup failure can therefore leave only an inactive, visibly named quarantine rather than an unregistered plugin at its executable location.

## Built-in integration boundary

[Coding agents](agents.md) are compiled-in privileged integrations. Each enabled agent registers as a built-in plugin, so its launches pass through the same `PluginManager`, `PluginAction::OpenTab`, PTY ownership, pane launch, and workspace restore dispatch as declarative plugins. Agent commands, such as diff review, context, transcript, and export, are application commands that use internal types the public APIs intentionally do not expose. Moving them onto the public layer would either remove those reviewed features or prematurely grant third-party access to arbitrary terminal state, filesystem mutation, clipboard, and secrets. API v32's public surface includes current-window inventory, navigation, topology, exact presented-text reads, ordinary pane/tab lifecycle, pane movement, bounded terminal input, same-tab pane layouts, command-scoped exact-window closure, process-wide read-only inventory, persistent handlers, non-activating cross-window tab/pane navigation, bounded cross-window presentation-safe terminal reads, guarded cross-window UTF-8 terminal input, cross-window ordinary local-tab and pane creation, cross-window ordinary pane and tab movement, cross-window exact ordinary pane and tab closure, cross-window exact ordinary pane layout operations, exact non-final process-window closure, exact process-window native focus requests, namespaced commands, non-secret settings, and narrowly granted completed-region snapshots. Those capabilities are not enough to represent the privileged workflows; broader migration waits for a separately designed sandboxed API.

## Compatibility policy

`api_version = 1`, `api_version = 2`, `api_version = 3`, `api_version = 4`, `api_version = 5`, `api_version = 6`, `api_version = 7`, `api_version = 8`, `api_version = 9`, `api_version = 10`, `api_version = 11`, `api_version = 12`, `api_version = 13`, `api_version = 14`, `api_version = 15`, `api_version = 16`, `api_version = 17`, `api_version = 18`, `api_version = 19`, `api_version = 20`, `api_version = 21`, `api_version = 22`, `api_version = 23`, `api_version = 24`, `api_version = 25`, `api_version = 26`, `api_version = 27`, `api_version = 28`, `api_version = 29`, `api_version = 30`, `api_version = 31`, and `api_version = 32` are separately supported exact contracts; registry `schema_version = 1` remains exact. API v1 preserves its process-only behavior, while API v2 adds the explicitly granted structured context described above, API v3 adds the bounded lifecycle-handler contract, API v4 adds the separately granted semantic command-completion event, API v5 adds the opt-in correlated persistent lifetime, API v6 adds the separately gated live current-window workspace inventory query, and API v7 adds separately granted stable-ID tab selection and pane focus with correlated results. API v8 adds independently granted local tab ordering and divider-ratio adjustment with the same correlated-result and authority-reduction contract. API v9 adds bounded exact-owner handler commands without adding a permission or host-action surface. API v10 adds bounded typed non-secret settings and explicit migration refusal, again without adding a permission or host-action surface. API v11 adds the separately granted, bounded exact-pane presented-terminal query and terminal-result authority reduction described above. API v12 adds separately granted exact ordinary-pane creation and returns the new stable pane identity before removing every host-operation authority. API v13 adds separately granted ordinary-tab creation in the originating window, returns the new stable tab identity, and suppresses lifecycle feedback before removing every host-operation authority. API v14 adds separately granted exact ordinary-pane closure, refuses the final pane and unsupported tab kinds, and returns the remaining/focused pane state before removing every host-operation authority. API v15 adds separately granted exact ordinary-tab closure scheduling, protects the final non-closing tab, returns the remaining/active tab state, and suppresses lifecycle feedback before removing every host-operation authority. API v16 adds separately granted exact ordinary-pane movement between distinct current-window tabs, preserves the live PTY, returns the fresh destination-local pane identity and resulting focus state, and removes every host-operation authority from the result. API v17 adds separately granted bounded terminal input to one exact current-window pane, requires the global disabled-by-default external-input policy, preserves target-mode encoding and destructive-command confirmation, and removes every host-operation authority from the result. API v18 adds separately granted exact same-tab pane swapping, balancing, and columns/rows/grid/tall/fat presets for ordinary split tabs, preserves live owners and focus, returns resulting pane/zoom state, and removes every host-operation authority from the result. API v19 adds separately granted exact current-window closure only to declared command invocations. It has no target ID, returns a bounded scheduled result, waits for the exactly correlated final handler callback, and then uses normal window teardown while preserving siblings. API v20 adds separately granted process-wide read-only inventory with stable process-local window IDs, explicit origin identity, deterministic ordering, bounded metadata and topology, closing-window exclusion, and complete query/mutation authority removal from its correlated result for API v20. API v21 adds separately granted exact non-activating tab selection and pane focus in any live process-owned window. Process-query results may retain only that independently granted mutation authority, while process-action results remove every host authority. API v22 adds separately granted exact non-activating presentation-safe terminal text reads in any live process-owned window. Process-query results may retain that independently granted disclosure authority, while terminal-query results remove every host authority. API v23 adds separately granted exact guarded terminal input in any live process-owned window. Process-query results may retain that independently granted mutation authority, while input results remove every host authority. API v24 adds separately granted ordinary local-tab creation in any exact live process-owned window. Process-query results may retain that independently granted mutation authority, while process-action results remove every host authority. API v25 adds separately granted exact ordinary-pane creation in any live process-owned window. Process-query results may retain that independently granted mutation authority, while process-action results remove every host authority. API v26 adds separately granted exact ordinary-pane movement between exact live process-owned tabs or windows. Process-query results may retain that independently granted mutation authority, while process-action results remove every host authority. API v27 adds separately granted exact ordinary-tab movement between live process-owned windows plus same-window reorder. Process-query results may retain that independently granted mutation authority, while movement results remove every host authority. Cross-window movement refuses the final non-closing source tab so it cannot implicitly close a window. API v28 adds separately granted closure of one exact ordinary pane in any live process-owned window. Process-query results may retain that independently granted mutation authority, while closure results remove every host authority. It preserves the containing tab/window, refuses the final pane and unsupported tab kinds, and never selects or activates a target. API v29 adds separately granted scheduled closure of one exact ordinary tab in any live process-owned window. Process-query results may retain that independently granted mutation authority, while closure results remove every host authority. It preserves the containing window, refuses the final non-closing tab and unsupported targets, suppresses lifecycle feedback, and never activates a native window. API v30 adds separately granted pane swapping, divider balancing, and columns/rows/grid/tall/fat presets for one exact ordinary split tab in any live process- owned window. Process-query results may retain that independently granted mutation authority, while layout results remove every host authority. It preserves terminal owners, stable pane IDs, focus, and active-tab selection, refuses stale or unsupported targets, and never activates a native window. API v31 adds separately granted closure of one exact non-final live process- owned window. Process-query results may retain that independently granted mutation authority, while closure results remove every host authority. The result is submitted before teardown; origin closure waits for its exact callback, sibling closure remains independent, stale or pending targets fail, and the operation never activates a native window or terminates the final live process window. API v32 adds a separately granted native focus request for one exact live process-owned window. Process-query results may retain that independently granted authority, while focus results remove every host authority. The operation shares authenticated GUI control policy: already-focused windows do not receive another request; stale, closing, native-less, hidden, minimized, and Wayland targets fail explicitly; and a successful request never claims the compositor confirmed activation. Any other API or registry value fails before registration. Additive optional fields require a documented Termeric release; removals or semantic changes require a new API version plus an explicit migration tool. Termeric does not guess at forward compatibility because permissions, argv, input, and disclosed context are security boundaries.
