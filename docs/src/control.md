# Controlling a running Termeric

`termeric control` lets scripts work with Termeric windows that are already open. You can list windows, tabs, and panes, read the text in a pane, run a program in a new tab and wait for its exit code, arrange and close things, and, if you allow it, type into a pane. Use it to open a build in a background tab from a script, collect a test run's output, or drive a layout from a window manager keybinding.

> **Experimental.** The control commands work whenever Termeric is running; there is nothing to turn on. Typing into panes stays off until you enable it (see [Typing into a pane](#typing-into-a-pane)). The commands and their JSON output may still change between releases.

Every running Termeric listens on a private socket in your runtime directory (`$XDG_RUNTIME_DIR/termeric`, or `/tmp/termeric-UID` without one). Only programs running as your user can connect. The `termeric control` commands find it on their own and never open a window.

## Finding windows, tabs, and panes

```console
$ termeric control list
process 48213
  window 1 1600x1000 [focused]
    tab 0 [shell] title="~/src/termeric" [active]
      pane 0 88x52 [focused]
      pane 1 88x52
      divider 1 vertical ratio=500
    tab 1 [shell] title="htop"
```

Each command names what it acts on by these IDs: the Termeric process ID, then a window, tab, pane, or divider. A tab's kind is `shell`, `profile` (a terminal profile), `plugin` (an agent or plugin tab), or `live-domain`. Markers such as `[active]`, `[pinned]`, `[activity]`, and `[hidden]` show state.

For scripts, use `--json`:

```bash
termeric control list --json | jq '.processes[0].windows[0].tabs[] | {id, title}'
```

The JSON has a top-level `schema_version` and a `processes` array. Each process has `process_id` and `windows`; each window has `id`, `focused`, `visible`, `pixel_width`, `pixel_height`, and `tabs`; each tab has `id`, `title`, `kind`, `active`, `pinned`, `panes`, and `dividers`; each pane has `id`, `focused`, `cols`, and `rows`. If more than one Termeric process is running, each is listed.

IDs are stable while the process runs: a window keeps its ID until it closes, even as other windows close. Tab IDs belong to their window and pane IDs to their tab, so a tab moved to another window gets a new tab ID. A tab that has never been split has a single pane `0`. IDs do not survive a restart of Termeric, so look them up with `list` rather than storing them.

`list` never shows what is in a terminal, its working directory, its command line, or its environment. Titles are the only text from your terminals it includes.

## Reading a pane's text

```bash
termeric control get-text 48213 1 0 1
termeric control get-text 48213 1 0 1 --history-lines 100 --bytes 16384 --json
```

`get-text` prints the text on the pane's screen as plain UTF-8. `--history-lines N` adds up to 256 lines of scrollback above it, and `--bytes N` caps the output (at most 64 KiB, the default). When the cap is reached, the oldest text is dropped. If a full-screen program such as an editor is running, only its screen is returned, never the scrollback behind it.

## Running programs

`launch-tab` runs a program in a new tab of a window, and `launch-window` in a new window. Everything after `--` is the program and its arguments, passed exactly as given and never run through a shell. To use shell syntax such as `&&`, run a shell yourself, as in the second example:

```bash
termeric control launch-tab 48213 1 --cwd ~/src/app --title tests -- cargo test --workspace
termeric control launch-tab 48213 1 -- /bin/sh -c 'make && make install'
termeric control launch-window 48213 1 --cwd /srv/logs -- tail -f app.log
```

| Option | What it does |
| --- | --- |
| `--cwd ABSOLUTE_DIRECTORY` | Start the program in this directory. `launch-window` otherwise uses the source window's focused tab's directory. |
| `--title TITLE` | Set the tab's title. |
| `--background` | `launch-tab` only: open the tab without selecting it, so what you are doing isn't interrupted. |
| `--env NAME=VALUE` | Set an environment variable. Repeatable. |
| `--unset-env NAME` | Remove an inherited variable. Repeatable. |
| `--clear-env` | Start from a minimal environment instead of Termeric's. |
| `--env-stdin` | Read variables from standard input as a JSON object. |
| `--wait [--timeout SECONDS]` | Wait for the program to exit, then exit with its code. |

`launch-tab` is refused while the window's selected tab belongs to a [live domain](live-domains.md); use `termeric domain spawn` to run a program there.

The program inherits Termeric's environment unless you change it. With `--clear-env`, Termeric still sets the terminal's own variables such as `TERM`, so give an absolute program path or set `PATH` yourself. Environment changes apply only to this one program; they are not saved in workspaces or passed to later splits.

Values given with `--env` can end up in your shell history and are visible to other programs in the process list. For anything sensitive, put the variables in a private file and pass it with `--env-stdin`:

```bash
termeric control launch-tab 48213 1 --env-stdin -- ./deploy.sh < ~/.config/deploy/env.json
```

The file holds one JSON object of names to string values, such as `{"API_URL": "https://example.test", "DEBUG": ""}`. It must be a regular file or a pipe of at most 256 KiB, and must end within five seconds.

### Waiting for a program to finish

With `--wait`, the command waits until the program exits and then exits with the program's exit code, so a script can react to the result:

```bash
if termeric control launch-tab 48213 1 --background --wait --timeout 900 -- make check; then
    notify-send "Checks passed"
else
    notify-send "Checks failed"
fi
```

Without `--wait`, the launch prints a request ID. Pass it to `launch-status` to check on the program later, or to `wait-launch` to wait for it:

```bash
termeric control launch-status 48213 3f9c0d2e8a4b4c1e9d7f6a5b4c3d2e1f
termeric control wait-launch 48213 3f9c0d2e8a4b4c1e9d7f6a5b4c3d2e1f --timeout 300
```

A few things to know about waiting:

- The exit code is the program's own, from 0 to 255. A program killed by a signal reports 1.
- An error, such as a timeout, also exits with a nonzero status but prints its message on standard error and nothing on standard output. With `--json`, the result on standard output tells you what the program actually did.
- A timeout or Ctrl+C stops only the waiting; the program keeps running in its tab.
- Only the program Termeric started is tracked, not commands it runs in turn.
- Termeric remembers the last 256 launches for as long as it runs.

## Arranging windows, tabs, and panes

| Command | What it does |
| --- | --- |
| `new-window PROCESS SOURCE_WINDOW` | Open a window like the source window, in its focused tab's directory. |
| `new-tab PROCESS WINDOW` | Open a shell tab and print its ID. |
| `split-pane PROCESS WINDOW TAB PANE horizontal\|vertical` | Split a pane (`horizontal` adds one below, `vertical` to the right) and print the new pane's ID. |
| `select-tab PROCESS WINDOW TAB` | Select a tab, without raising its window. |
| `focus-pane PROCESS WINDOW TAB PANE` | Select a tab and focus a pane. |
| `focus-window PROCESS WINDOW` | Ask the desktop to focus a window. On Wayland the compositor decides, so this is refused. |
| `move-tab PROCESS SOURCE_WINDOW TAB DESTINATION_WINDOW INDEX` | Move a tab to a position (counting from 0), in the same or another window, and print its ID there. |
| `move-pane PROCESS SOURCE_WINDOW SOURCE_TAB PANE DESTINATION_WINDOW DESTINATION_TAB TARGET_PANE horizontal\|vertical` | Move a running pane next to a pane in a different tab. |
| `resize-divider PROCESS WINDOW TAB DIVIDER RATIO` | Set a split's ratio in thousandths, from 100 to 900. |
| `resize-window PROCESS WINDOW WIDTH HEIGHT` | Ask for a window size in physical pixels. The window manager may adjust it; check `list` for the result. |
| `close-pane PROCESS WINDOW TAB PANE` | Close a pane. A tab's last pane can't be closed this way; close the tab. |
| `close-tab PROCESS WINDOW TAB` | Close a tab. |
| `close-window PROCESS WINDOW` | Close a window. |

Moves keep the program running. Pinned tabs stay in the pinned group, so a move across the boundary is refused. Tabs and panes of a live domain can be split, focused, closed, and rearranged within that domain too, but not moved to another window or mixed with ordinary tabs, and a domain's last tab and pane can be removed only with `termeric domain kill`.

## Typing into a pane

`send-text` types text into a pane as if you had typed it:

```bash
termeric control send-text 48213 1 1 0 'cargo test' --enter
termeric control send-text 48213 1 1 0 "$(cat snippet.txt)" --paste
```

`--enter` presses Enter after the text, and `--paste` sends it as a paste, so a shell or editor that understands bracketed paste won't run each line as it arrives. Text is limited to 64 KiB of printable characters, tabs, and newlines; escape sequences and other control characters are refused.

Typing is off by default, because it lets any program running as you run commands in your shells. Turn it on in your configuration:

```toml
[security]
allow_gui_control_input = true
```

The same setting lets [`termeric mcp`](agents.md#letting-an-agent-use-termeric-termeric-mcp) send prompts to coding agents and lets [plugins](plugins.md) you granted input permission type into terminals. Text sent to an agent or plugin tab that looks like a destructive command, such as `rm -rf`, opens a confirmation dialog first, and nothing is typed unless you confirm. A read-only pane refuses all input.

## Retrying safely

Every command that changes something carries a request ID, which Termeric generates and shows in its output and in any error. If a command times out and you can't tell whether it worked, run it again with `--request-id ID` and the same ID. Termeric recognizes the repeat and returns the first result instead of opening a second tab or typing the text twice. It remembers the last 256 requests. A `send-text` that failed keeps failing under its ID, since some of the text may have been typed; look at the pane, then use a fresh ID. To be able to retry a command that might be interrupted, pass your own ID from the start: 32 lowercase hexadecimal digits, such as the output of `openssl rand -hex 16`.

## Safety

- The socket accepts only your own user, but any program running as you can use it. It can list your windows, read the text in any pane, and launch programs, even with typing turned off.
- Nothing lets a script read a terminal's environment, command line, working directory, clipboard, or scrollback beyond the 256 lines `get-text` allows.
- Values passed with `--env` can show up in shell history and process listings; use `--env-stdin` from a private file for secrets.
- Don't enable `allow_gui_control_input` on a machine where you run programs you don't trust as your own user.
