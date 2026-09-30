# Plugins

A plugin adds launch profiles, command palette commands, and reactions to what happens in your terminals. Each plugin is a directory holding one manifest, `termeric-plugin.toml`, that declares what the plugin offers and which permissions it needs. Termeric never loads plugin code into itself: a plugin's profiles run as programs in tabs, and its event handler is a separate program that exchanges JSON with Termeric over standard input and output.

A plugin can:

- open a program in a tab or split pane from a **launch profile**, with the plugin's settings and the active terminal's directory in its environment;
- add **commands** to the command palette, which you can also bind to keys;
- run an **event handler** when a command is invoked, a tab opens, closes, or is selected, a window becomes ready, or a shell command finishes;
- from that handler, show messages, inspect windows, tabs, and panes, read terminal text, type into a terminal, and create, close, move, and arrange tabs, panes, and windows.

Nothing takes effect until you install the plugin and grant the permissions it asks for.

## Example plugins

The repository has five example plugins in `examples/plugins/`. Each manifest starts with a comment explaining what it does and the exact command that installs it.

| Plugin | What it does | Shows how to |
| --- | --- | --- |
| [task-runner](https://github.com/Tjemmmic/termeric/tree/main/examples/plugins/task-runner) | Profiles that run your project's test command or dev server in the active terminal's directory, and a clean shell. | Launch profiles, settings, launch context |
| [command-notifier](https://github.com/Tjemmmic/termeric/tree/main/examples/plugins/command-notifier) | Shows a notice with the end of the output when a shell command fails, ignoring exit codes you list. | Reacting to `command-completed` |
| [pane-layouts](https://github.com/Tjemmmic/termeric/tree/main/examples/plugins/pane-layouts) | Commands that split, tile, and balance the panes of the active tab. | Querying the workspace, then acting |
| [window-switcher](https://github.com/Tjemmmic/termeric/tree/main/examples/plugins/window-switcher) | Commands that focus the next window, or select a tab by title in any window. | `:all-windows` permissions |
| [rerun-in-sibling](https://github.com/Tjemmmic/termeric/tree/main/examples/plugins/rerun-in-sibling) | Types the focused pane's last command into the next pane of the tab and runs it. | Remembering state, typing into a terminal |

To try one, clone the repository and install it from its folder, for example:

```bash
termeric plugin install examples/plugins/pane-layouts \
  --allow process-spawn --allow event-subscribe --allow ui-actions \
  --allow workspace-query --allow pane-create --allow pane-layout --enable
```

## Installing and managing plugins

You can manage plugins in Settings or with `termeric plugin`. Both edit the same files, and a running Termeric notices the change, reloads its plugins, and shows "Third-party plugins reloaded".

### In Settings

Open Settings, go to the **Plugins** page, and choose **Manage Third-party Plugins** (the palette command **Configure Plugins** opens the same page). The manager lists installed plugins with their version and whether each is enabled, disabled, or has an error.

To install one, choose **Install local plugin** and enter the absolute path of the plugin's directory. Termeric reads the manifest and shows a review: name, version, the number of profiles and commands, the directory's full path, the manifest's fingerprint, and every requested permission with its scope ("this window" or "all windows"). Select permissions to grant them one at a time, or use **Grant all requested permissions**. Then choose **Install disabled** or **Install and enable**; enabling is offered only when every requested permission is granted. If the manifest changes between the review and your choice, the install is refused and you review it again.

Select an installed plugin to see its source, requested and granted permissions, settings, and any error, and to:

- **Choose default profile**, the profile used when a launch or restore doesn't name one;
- **Configure settings**, one setting at a time, or **Reset settings to defaults**;
- **Enable** or **Disable** it;
- **Update from source** (see [Updating](#updating));
- **Remove installed copy**.

### From the command line

`termeric plugin` works on the files directly; Termeric doesn't need to be running.

| Command | What it does |
| --- | --- |
| `list [--json]` | List installed plugins with their version, source, requested and granted permissions, settings, and any error. |
| `show ID [--json]` | The same for one plugin. |
| `validate DIRECTORY` | Check a plugin directory without installing it, and print its ID, version, fingerprint, and permissions. |
| `install DIRECTORY [--allow PERMISSION]… [--enable] [--replace]` | Install a plugin, granting each permission named with `--allow`. `--enable` turns it on and requires every requested permission. `--replace` reinstalls a plugin that is already installed. |
| `update ID` | Reinstall from the directory it was installed from, keeping whether it's enabled and its grants for the permissions the update still requests. |
| `enable ID` | Turn a plugin on. Every requested permission must already be granted. |
| `disable ID` | Turn a plugin off without removing it. |
| `configure ID --default-profile PROFILE` | Set the default profile. |
| `configure ID --clear-default-profile` | Go back to the first profile in the manifest. |
| `configure ID --set KEY VALUE` | Set a setting. Booleans take `true` or `false`, integers a base-10 number. |
| `configure ID --unset KEY` | Return one setting to its default. |
| `configure ID --reset-settings` | Return every setting to its default. |
| `remove ID` | Remove the installed copy and its registry entry. The source directory is left alone. |

### Granting permissions

You grant permissions when you install. The rules are the same everywhere:

- You can grant only what the manifest requests, at no wider scope. `--allow pane-create:all-windows` is refused for a plugin that requests `pane-create`.
- A plugin can be installed with only some of its permissions granted, but it stays disabled until all of them are.
- To change grants later, use **Update from source** in the manager, or run `termeric plugin install DIRECTORY --replace` with the full set of `--allow` options.

### Updating

Installing copies the manifest, so editing the plugin's source directory has no effect until you update. **Update from source** in the manager shows the installed and updated version, profile and command counts, both fingerprints, and the permissions added and removed. Permissions that stay the same keep their grants; newly requested ones start ungranted. Choose **Update and keep enabled** (or **Update and enable**), which needs every permission granted, or **Update and disable**.

`termeric plugin update ID` does the same without a review, so it refuses an update that requests a permission or a wider scope the installed version didn't; install that version with `--replace` and explicit `--allow` options instead. Grants for permissions the update drops are removed, and a grant whose permission the update narrows to window scope narrows with it.

An update keeps your settings, and is refused if a setting you changed doesn't fit the updated manifest; unset or reset it first. A default profile that the updated manifest drops is cleared.

### Where plugins are stored

| Path | Contents |
| --- | --- |
| `~/.config/termeric/plugins.toml` | The registry: each installed plugin's source directory, manifest fingerprint, grants, enabled state, default profile, and settings. |
| `~/.config/termeric/plugins/ID/termeric-plugin.toml` | The installed copy of each manifest. |

With `XDG_CONFIG_HOME` set, both live under `$XDG_CONFIG_HOME/termeric/`. Only the manifest is copied. If the installed copy is edited or doesn't match its fingerprint, the plugin doesn't load and shows an error until you reinstall it.

## Using a plugin

An enabled plugin adds these commands to the command palette:

- **Open PROFILE NAME** for each launch profile, which opens it in a tab;
- **Split Right with PLUGIN NAME** and **Split Down with PLUGIN NAME**, with and without a named profile, which open a profile in a pane beside the focused one;
- each of its commands.

A command's ID is `PLUGIN-ID.COMMAND-ID`, and a profile's palette command is `PLUGIN-ID.open-PROFILE-ID`. Bind either to a key with a `plugin_command` key binding (see [Keyboard and mouse](keyboard-and-mouse.md#keys-that-type-text-or-run-commands)):

```toml
[[keybindings]]
key = "l"
modifiers = { ctrl = true, alt = true }
action = { type = "plugin_command", command_id = "pane-layouts.dev-layout" }
```

`termeric --profile PLUGIN-ID:PROFILE-ID` opens a profile from the command line. Plugin tabs show as kind `plugin` in `termeric control list`, and [workspaces](workspaces-and-sessions.md) relaunch them from the same plugin and profile.

## Writing a plugin

A plugin is a directory with a `termeric-plugin.toml`. Check it with `termeric plugin validate DIRECTORY` as you work, and install it when it's valid.

### The manifest

| Key | Required | Description |
| --- | --- | --- |
| `api_version` | yes | Must be `1`. |
| `id` | yes | The plugin's ID. `claude-code` and `codex` are reserved. |
| `name` | yes | Display name, one line, at most 80 bytes. |
| `version` | yes | Any one-line text, at most 64 bytes. |
| `description` | no | At most 1024 bytes. |
| `permissions` | yes | The permissions the plugin needs; see [Permissions](#permissions). Must include `process-spawn`. |
| `[[profiles]]` | no | Launch profiles, at most 32. |
| `[[commands]]` | no | Palette commands handled by the event handler, at most 32. |
| `[[settings]]` | no | Settings you can change, at most 32. |
| `[event_handler]` | no | The program that receives events. |

IDs (of the plugin, its profiles, commands, and settings) are 1 to 64 lowercase ASCII letters, digits, and hyphens, starting with a letter. The manifest must be UTF-8, at most 64 KiB, and a regular file rather than a symbolic link. Unknown keys are errors.

A plugin needs at least one profile or an event handler.

### Profiles

| Key | Required | Description |
| --- | --- | --- |
| `id` | yes | Unique within the plugin. |
| `name` | yes | Shown in the palette and used as the tab title, at most 80 bytes. |
| `description` | no | At most 512 bytes. |
| `program` | yes | The program to run. It is started directly, never through a shell. |
| `args` | no | Arguments, passed exactly as written. At most 64, each at most 4 KiB, and 32 KiB with the program. |
| `working_directory` | no | `active` (default) starts in the active terminal's directory; `inherit` doesn't use it. A split pane always follows your `[panes] cwd` setting. |

A profile runs in a terminal with your normal environment, plus:

| Variable | When | Contents |
| --- | --- | --- |
| `TERMERIC_PLUGIN_SETTINGS_JSON` | The plugin declares settings. | `{"schema_version": 1, "plugin_id": "…", "values": {"SETTING-ID": value, …}}` |
| `TERMERIC_PLUGIN_CONTEXT_JSON` | `workspace-query` is granted. | `{"schema_version": 1, "plugin_id": "…", "profile_id": "…", "active_working_directory": "…" or null, "tab_count": N, "active_tab_index": N, "terminal": {"columns": N, "rows": N}}` |

`active_tab_index` is the active tab's position in the tab bar, counting from 0.

### Commands

| Key | Required | Description |
| --- | --- | --- |
| `id` | yes | Unique within the plugin, and not `open-PROFILE-ID` for one of its profiles. |
| `name` | yes | Shown in the palette, at most 80 bytes. |
| `description` | no | At most 512 bytes. |

Invoking a command sends a `command-invoked` event to the event handler, so a plugin with commands needs a handler subscribed to `command-invoked`, and a handler subscribed to it needs at least one command.

### Settings

Every setting has a `type`, an `id`, a `name` (at most 80 bytes), and an optional `description` (at most 512 bytes), plus:

| `type` | Keys | Value |
| --- | --- | --- |
| `boolean` | `default` | `true` or `false` |
| `integer` | `default`, `min`, `max` | A whole number from `min` to `max` |
| `text` | `default` (optional, empty if left out) | One line of at most 512 bytes |
| `choice` | `default`, `choices` (1 to 32 values, each at most 80 bytes) | One of `choices` |

The effective values, your changes over the manifest's defaults, reach profiles in `TERMERIC_PLUGIN_SETTINGS_JSON` and the event handler in each event's `settings`. All values together must fit in 8 KiB of JSON.

## Permissions

Each permission is a capability, and those that act on windows take a scope. Written plainly, as `pane-create`, a permission covers only the window where the event happened, called the **origin window**. With the `:all-windows` suffix, as `pane-create:all-windows`, it covers every window of that Termeric process, including the origin window. A plugin lists each capability once.

| Permission | What it allows | `:all-windows` |
| --- | --- | --- |
| `process-spawn` | Running the plugin's profiles and event handler. Every plugin needs it. | no |
| `event-subscribe` | Receiving the events the handler subscribes to. Required with, and only with, `[event_handler]`. | no |
| `ui-actions` | `show-message` and `show-notice`. Needs an event handler. | no |
| `command-output` | `command-completed` events, including the command and its output. Required with, and only with, a subscription to `command-completed`. | no |
| `window-focus` | `focus-window` on any window. | no; always covers every window |
| `workspace-query` | The launch context for profiles, the `context` field in events, and `query-workspace`. Window scope sees only the origin window. | yes |
| `navigate` | `select-tab` and `focus-pane`. | yes |
| `terminal-read` | `query-terminal`. | yes |
| `terminal-input` | `send-text`. | yes |
| `tab-create` | `create-tab`. | yes |
| `tab-close` | `close-tab`. | yes |
| `tab-move` | `move-tab`. | yes |
| `pane-create` | `create-pane`. | yes |
| `pane-close` | `close-pane`. | yes |
| `pane-move` | `move-pane`. | yes |
| `pane-layout` | `swap-panes`, `balance-panes`, `apply-pane-layout`, and `resize-divider`. | yes |
| `window-close` | `close-window`. | yes |

Every permission from `window-focus` down, except `workspace-query`, is used through handler actions whose results come back to the handler, so it requires `mode = "persistent"`. `window-close` without `:all-windows` also requires at least one command, because it may close the origin window only from a chain started by one of the plugin's commands.

## Event handlers

The `[event_handler]` table names one program that receives every event the plugin subscribes to.

| Key | Required | Description |
| --- | --- | --- |
| `program` | yes | The program to run, started directly. |
| `args` | no | Arguments, with the same limits as a profile's. |
| `mode` | no | `process-per-event` (default) or `persistent`. |
| `events` | yes | One or more of the events below. |

Termeric starts the handler with an empty environment except `PATH`, `TERMERIC_PLUGIN_ID` (the plugin's ID), and `TERMERIC_PLUGIN_EVENT_SCHEMA` (`1`), in Termeric's own working directory. Only the manifest is installed, so give a handler script as an absolute path, or put the code in `args` as the examples do.

Each window runs its handlers separately: in `persistent` mode, every window that has sent the plugin an event has its own handler process, with its own memory. A window runs one handler request at a time, across all plugins, and queues up to 32 more.

### Process-per-event handlers

Termeric starts the program for each event, writes one JSON request to its standard input, and closes it. The program writes one JSON response to standard output and exits with status 0 within 2 seconds. Anything it started is stopped when it exits. These handlers can only show messages and notices.

### Persistent handlers

Termeric starts the program with the first event and keeps it running. Each request is one line of JSON on standard input, with a `request_id`; the program answers each with one line of JSON on standard output carrying the same `request_id`, within 2 seconds, and must print nothing else there. Flush standard output after each line. Standard error is kept for error messages.

A persistent handler receives the results of its queries and actions, so only it can use them. If it fails a request, Termeric stops it and starts a fresh process for the next event. It is also restarted whenever the plugin registry or your configuration is reloaded, and stopped when the plugin is disabled or the window closes.

### Requests

| Field | Contents |
| --- | --- |
| `schema_version` | `1` |
| `plugin_id` | The plugin's ID. |
| `origin_window_id` | The window the event happened in, with the same IDs as `termeric control list`. |
| `event` | The event, with its `type`. |
| `request_id` | Persistent handlers only: the number to echo in the response. |
| `settings` | Event requests only, when the plugin declares settings: `{"SETTING-ID": value, …}`. |
| `context` | Event requests only, when `workspace-query` is granted: `active_working_directory`, `tab_count`, `active_tab_index`, and `terminal` (`columns`, `rows`) for the origin window. |

Result events (below) carry no `settings` or `context`; a persistent handler that needs them later keeps them from the event that started the conversation.

### Events

| Event | Fields | When |
| --- | --- | --- |
| `app-ready` | none | A window has finished setting up its tabs. |
| `tab-created` | `tab_id` | A tab opened. |
| `tab-closed` | `tab_id` | A tab closed. |
| `tab-switched` | `from`, `to` | The active tab changed. These are positions in the tab bar, counting from 0, not tab IDs. |
| `command-completed` | `region_id`, `tab_id`, `pane_id`, `exit_code`, `content_available`, `command`, `output` | A shell command finished. Needs [shell integration](shell-integration.md). |
| `command-invoked` | `command_id` | One of the plugin's commands ran. `command_id` is `PLUGIN-ID.COMMAND-ID`. |

In `command-completed`, `pane_id` is `null` in a tab that isn't split, `exit_code` is `null` when the shell didn't report one, and `command` and `output` are each `{"text", "truncated", "sanitized"}`. Command text is cut to about 2 KiB and output to about 8 KiB; `truncated` says so. Control characters other than tab and newline are replaced with `�` and `sanitized` is `true`. When `content_available` is `false`, both texts are empty.

Tabs, panes, and focus changed by a plugin's own actions raise no events; the action's result is the only reply.

### Responses

```json
{"schema_version": 1, "request_id": 7, "actions": []}
```

A response has `schema_version` 1, the `request_id` for persistent handlers (and none for process-per-event handlers), and up to 8 `actions`. Any number of them may be `show-message` or `show-notice`; at most one may be a query or another action. An empty `actions` list is a valid reply.

Termeric checks the whole response before doing anything: if any action lacks its permission, has an invalid field, or isn't allowed at that point in the conversation, none of it runs and you see an error toast naming the plugin. Which window an action targets is checked only when the action runs, so naming another window without the permission's `:all-windows` scope comes back as a result with `ok` set to `false`, after the response's messages have been shown.

### Actions

Every action has a `type`. All but the two message actions and the two queries also need an `action_id`, and queries need a `query_id`: text of at most 64 bytes that Termeric echoes in the result so you can tell replies apart. Any `window_id`, `source_window_id`, or `destination_window_id` may be left out to mean the origin window; naming another window needs the permission's `:all-windows` scope.

| `type` | Permission | Fields | Result |
| --- | --- | --- | --- |
| `show-message` | `ui-actions` | `message` (at most 512 bytes) | A toast |
| `show-notice` | `ui-actions` | `title` (at most 128 bytes), `lines` (1 to 16, each at most 512 bytes) | A notice dialog |
| `query-workspace` | `workspace-query` | `query_id` | `workspace-query-result` |
| `query-terminal` | `terminal-read` | `query_id`, `window_id`, `tab_id`, `pane_id`, `history_lines` (0 to 256), `max_bytes` (1 to 8192) | `terminal-query-result` |
| `select-tab` | `navigate` | `window_id`, `tab_id` | `changed` |
| `focus-pane` | `navigate` | `window_id`, `tab_id`, `pane_id` | `tab_changed`, `pane_changed` |
| `send-text` | `terminal-input` | `window_id`, `tab_id`, `pane_id`, `text`, `paste`, `enter` | `input_bytes`, `wire_bytes`, `disposition`, `confirmation_id` |
| `create-tab` | `tab-create` | `window_id` | `created_tab_id` |
| `close-tab` | `tab-close` | `window_id`, `tab_id` | `scheduled`, `remaining_tabs`, `active_tab_id`, `active_changed` |
| `move-tab` | `tab-move` | `source_window_id`, `tab_id`, `destination_window_id`, `destination_index` | `destination_tab_id`, `source_window_closing`, `changed` |
| `create-pane` | `pane-create` | `window_id`, `tab_id`, `pane_id`, `direction` | `created_pane_id` |
| `close-pane` | `pane-close` | `window_id`, `tab_id`, `pane_id` | `remaining_panes`, `focused_pane_id` |
| `move-pane` | `pane-move` | `source_window_id`, `source_tab_id`, `pane_id`, `destination_window_id`, `destination_tab_id`, `target_pane_id`, `direction` | `destination_pane_id` |
| `swap-panes` | `pane-layout` | `window_id`, `tab_id`, `first_pane_id`, `second_pane_id` | `pane_order`, `focused_pane_id`, `zoomed_pane_id` |
| `balance-panes` | `pane-layout` | `window_id`, `tab_id` | `changed`, `pane_order`, `focused_pane_id`, `zoomed_pane_id` |
| `apply-pane-layout` | `pane-layout` | `window_id`, `tab_id`, `preset` | `pane_order`, `focused_pane_id`, `zoomed_pane_id` |
| `resize-divider` | `pane-layout` | `window_id`, `tab_id`, `divider_id`, `ratio_millis` (100 to 900) | `previous_ratio_millis`, `changed` |
| `close-window` | `window-close` | `window_id` | `scheduled`, `tab_count`, `remaining_windows` |
| `focus-window` | `window-focus` | `window_id` (required) | `already_focused`, `requested` |

What the actions do, and their rules:

- `select-tab` and `focus-pane` change the selection inside a window without raising it; `focus-window` asks the desktop to raise a window. On Wayland the compositor controls focus, so Termeric refuses `focus-window` unless the window is already focused.
- `send-text` types `text` (at most 8 KiB of printable characters, tabs, and newlines), as a bracketed paste when `paste` is `true`, then presses Enter when `enter` is `true`. It needs `security.allow_gui_control_input = true` in your [configuration](configuration.md) and is refused for a read-only pane. Text that looks like a destructive command, such as `rm -rf ~`, opens a confirmation dialog instead of being typed: `disposition` is `pending_confirmation` and `confirmation_id` identifies the dialog. Otherwise `disposition` is `sent`. While a confirmation is open in that window, further `send-text` actions are refused.
- `create-tab` opens a shell tab. It is refused while the window's active tab belongs to a [live domain](live-domains.md).
- `close-tab` can't close a window's last tab, and `close-pane` can't close a tab's last pane.
- `move-tab` reorders a tab within a window (`destination_index` counts from 0) or moves it to another, but can't take a window's last tab. `move-pane` moves a pane next to `target_pane_id` in another tab.
- `direction` is `vertical` (to the right) or `horizontal` (below). `preset` is `columns`, `rows`, `grid`, `tall`, or `fat`, as in the **Layout** commands (see [Tabs, panes, and windows](tabs-panes-windows.md)).
- `close-window` with `window-close:all-windows` closes any window but the last one left. Plain `window-close` closes only the origin window, only when the conversation began with one of the plugin's commands, and may close the last window. The origin window closes after the handler has answered the result.

### Results

A query or action gets exactly one result, sent to the persistent handler as a request with the same envelope and a fresh `request_id`.

`workspace-query-result` has the `query_id`, a `windows` array, and `truncated`. Each window has `id`, `focused`, `visible`, `quick_access`, `workspace_id`, `pixel_width`, `pixel_height`, `tabs`, and `truncated`; each tab has `id`, `title`, `kind`, `active`, `pinned`, `activity`, `closed`, `domain_id`, `panes`, `dividers`, and `truncated`; each pane has `id`, `title`, `focused`, `closed`, `cols`, and `rows`; each divider has `id`, `kind`, `direction`, and `ratio_millis`. With window scope only the origin window is listed. The result covers at most 8 windows, 16 tabs, 32 panes, and 31 dividers, and `truncated` is `true` when something was left out.

`terminal-query-result` echoes `query_id`, `window_id`, `tab_id`, `pane_id`, `requested_history_lines`, and `requested_max_bytes`, and has `ok`. On success it adds `screen` (`primary` or `alternate`), `history_lines` (the scrollback lines included), `text`, `text_bytes`, and `truncated`. The text is cut further when needed to fit the 16 KiB request.

`action-result` has `action` (the action's type), `ok`, and the action's own fields, with omitted window IDs filled in and `text` left out. On success it adds the result fields from the table; on failure, `error` and `error_truncated`.

### Follow-ups

A conversation is limited so that one event can't set off an endless chain of requests:

1. An event may start one query or one action.
2. A `workspace-query-result` may start one `query-terminal` or one action, but not another workspace query.
3. A `terminal-query-result` or `action-result` ends the conversation; its response may only show messages and notices.

So "inspect, then act" takes three replies: the event asks for `query-workspace`, the result names the action, and the `action-result` reports how it went.

The limit applies to one conversation, not across events. An action can cause a new event that starts a new conversation: a `send-text` with `enter` set runs a command, and its `command-completed` event reaches the handler again. A handler that sends text in reply to `command-completed` must decide for itself when to stop.

### Limits and errors

| Limit | Value |
| --- | --- |
| Time to answer one request | 2 seconds |
| Request size | 16 KiB |
| Response size | 64 KiB |
| Actions per response | 8, with at most one query or action |
| Persistent handlers per window | 8 |
| Queued requests per window | 32 |

A handler that exits with an error, times out, writes invalid JSON, sends the wrong `schema_version` or `request_id`, or asks for something it may not do produces an error toast, "Plugin ID event handler failed", with the reason and, when the handler exited with an error, the start of its standard error. Control characters are removed and the whole message is cut to 1 KiB. An action that Termeric accepts but can't carry out, such as selecting a tab that has closed, comes back as an `action-result` with `ok` set to `false`.

### A complete example

This plugin adds **Split Focused Pane**, which splits the focused pane of the active tab in the direction chosen in its settings. The manifest:

```toml
api_version = 1
id = "split-here"
name = "Split Here"
version = "1.0.0"
description = "Split the focused pane of the active tab"
permissions = ["process-spawn", "event-subscribe", "ui-actions", "workspace-query", "pane-create"]

[[settings]]
type = "choice"
id = "direction"
name = "Direction"
description = "vertical splits to the right, horizontal splits below"
default = "vertical"
choices = ["vertical", "horizontal"]

[[commands]]
id = "split"
name = "Split Focused Pane"

[event_handler]
program = "python3"
args = ["/home/me/plugins/split-here/handler.py"]
mode = "persistent"
events = ["command-invoked"]
```

The handler, `handler.py`:

```python
import json
import sys

settings = {}


def reply(request, actions):
    response = {"schema_version": 1, "request_id": request["request_id"], "actions": actions}
    print(json.dumps(response), flush=True)


def focused_pane(windows, origin):
    for window in windows:
        if window["id"] != origin:
            continue
        for tab in window["tabs"]:
            if tab["active"] and not tab["closed"]:
                for pane in tab["panes"]:
                    if pane["focused"] and not pane["closed"]:
                        return tab["id"], pane["id"]
    return None


for line in sys.stdin:
    request = json.loads(line)
    event = request["event"]
    actions = []
    if event["type"] == "command-invoked":
        # Results carry no settings, so keep them from the command's event.
        settings = request.get("settings", {})
        actions.append({"type": "query-workspace", "query_id": "split"})
    elif event["type"] == "workspace-query-result":
        target = focused_pane(event["windows"], request["origin_window_id"])
        if target is None:
            actions.append({"type": "show-message", "message": "No focused pane to split"})
        else:
            tab_id, pane_id = target
            actions.append({
                "type": "create-pane",
                "action_id": "split",
                "tab_id": tab_id,
                "pane_id": pane_id,
                "direction": settings.get("direction", "vertical"),
            })
    elif event["type"] == "action-result" and not event["ok"]:
        actions.append({"type": "show-message", "message": "Split failed: " + event["error"][:200]})
    reply(request, actions)
```

Install it with:

```bash
termeric plugin install ~/plugins/split-here \
  --allow process-spawn --allow event-subscribe --allow ui-actions \
  --allow workspace-query --allow pane-create --enable
```

## Safety

- Plugins run as programs under your user account. Permissions limit what a plugin can ask Termeric to do, not what its own programs can do: a profile or handler can read and write your files and use the network like anything else you run.
- Install only plugins you trust, and read the manifest first, including its `program` and `args` and any script they run. Termeric installs only from a local directory and never downloads code.
- The fingerprint shows whether a manifest changed since you reviewed it. It is not a signature and says nothing about who wrote the plugin, or about scripts outside the manifest.
- Typing into terminals needs both the `terminal-input` permission and `security.allow_gui_control_input = true`, which also allows [`termeric control send-text`](control.md#typing-into-a-pane). Destructive-looking text always asks for confirmation.
- Grant `:all-windows` only when a plugin needs to reach other windows; window-scoped permissions can't see or change any window but the one you're working in.
