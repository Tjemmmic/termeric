# Live domains (experimental)

A live domain is a set of tabs and panes whose shells belong to a background Termeric daemon instead of a window. Close every Termeric window, or even let Termeric crash, and the shells in a domain keep running; attach again later and you get the same tabs, layout, scrollback, and running programs back. It works much like a tmux or screen session, with Termeric's own windows as the view. You can also drive a domain entirely from scripts, and reach a domain on another computer over SSH.

> **Experimental.** There is nothing to turn on: the daemon starts on its own the first time you use it. Domains live only as long as the daemon, so they don't survive a reboot or a daemon crash, and upgrading Termeric means ending your domains first. Agent status doesn't work in domains, notifications and bells from while you were detached are not replayed, and a window showing a domain can't be saved as a [workspace](workspaces-and-sessions.md).

## Getting started

```bash
# Create a domain with one shell; it prints the domain's ID
termeric domain create work

# See your domains
termeric domain list

# Open a window on it
termeric domain attach domain-…
```

Close the window when you are done: that only detaches it. Closing a single tab or pane, on the other hand, ends its programs in the domain. Run `termeric domain attach` again, from any terminal or a desktop launcher, to pick up where you left off.

You can also attach from inside Termeric: **Attach Live Domain…** in the command palette lists this computer and each [saved SSH host](#saved-hosts), then that host's domains, and opens the one you choose in a new window. It appears once the daemon is running or you have saved a host.

## Working in an attached window

An attached window behaves like any other. New tabs, splits, closing panes and tabs, renaming, pinning, reordering tabs, dragging dividers, swapping panes, and layout presets all happen in the domain, so every window attached to it sees the change. Zooming a pane stays local to your window.

Several windows can attach to the same domain at once, and they share its panes rather than copying them. The most recent window to resize a pane sets its size.

When the connection to the daemon drops, the window says which domain is affected, keeps retrying, and tells you when it is back.

Dropping a file or folder onto a domain pane asks you to confirm an upload. Termeric then copies it into that pane's working directory, never overwriting anything, and types the quoted path into the pane without pressing Enter. **Cancel Upload** in the command palette stops an upload in progress.

## Controlling a domain from scripts

Every change you can make in a window has a command. Pane, tab, and split IDs come from `termeric domain show DOMAIN --json`, which describes the whole layout. Add `--json` to most commands for script-friendly output.

```bash
# Create a domain running a command, with a size and title
termeric domain create build --cwd ~/src/app --title watch --size 120x40 -- cargo watch -x check

# Add a tab and split a pane
termeric domain spawn DOMAIN --title logs -- tail -f app.log
termeric domain split DOMAIN 0 vertical --cwd ~/src/app

# Type into a pane and read what it shows
termeric domain send DOMAIN 1 'make test' --enter
termeric domain get-text DOMAIN 1 --bytes 65536

# Rearrange
termeric domain layout-tab DOMAIN 0 grid
termeric domain move-pane DOMAIN 1 2
termeric domain rename-tab DOMAIN 0 "Build"
```

`send` types up to 64 KiB of text; unlike [typing into ordinary windows](control.md#typing-into-a-pane), it needs no setting. `get-text` returns up to 1 MiB of the pane's newest text. [Command line](command-line.md#termeric-domain) lists every `termeric domain` command and its options.

To copy a file or directory into a pane's working directory from a script:

```bash
termeric domain upload DOMAIN 1 ./dist.tar.gz
termeric domain upload DOMAIN 1 ./assets --name static --retain
```

An upload never overwrites an existing file. It is limited to 64 MiB, and a directory to 4096 entries. With `--retain`, an interrupted upload is kept for up to 24 hours and prints an ID you can pass to `--resume` to continue it; `termeric domain uploads` lists kept uploads and `termeric domain cancel-upload ID` deletes one.

## Closing panes, tabs, and domains

| Command | What it does |
| --- | --- |
| `close-pane DOMAIN PANE` | Ends a pane's program and removes the pane. A tab's only pane is closed with `close-tab`. |
| `close-tab DOMAIN TAB` | Ends every program in a tab and removes it. A domain's only tab can't be closed this way. |
| `kill DOMAIN` | Ends every program in the domain and removes it. |
| `stop-server` | Stops the daemon once no domains remain. |

When a program exits on its own, its pane is removed, and its tab too if that was the tab's last pane. The last pane of a domain's last tab stays, showing its final output, until you kill the domain. Exited programs are never restarted.

To upgrade Termeric, kill your domains and run `termeric domain stop-server`, so the next command starts the new daemon. If versions don't match, commands refuse to connect; `termeric domain protocol-version` shows which protocol a `termeric` executable speaks.

## Domains on another computer

Any `termeric domain` command can run against a domain on another computer over SSH: put `--host` and an SSH destination right after `domain`.

```bash
termeric domain --host me@build.example list
termeric domain --host me@build.example create build -- cargo watch -x check
termeric domain --host me@build.example attach DOMAIN
```

Termeric runs `ssh` for you and, on the other computer, `termeric domain proxy-stdio`, which connects to the daemon there. It never opens a network port of its own. For this to work:

- the other computer has a compatible `termeric` on its `PATH`;
- its host key is already in your `known_hosts`, since Termeric never asks you to accept a new key;
- you can log in without typing a password, with a key or your SSH agent.

Ports, keys, jump hosts, and other connection details come from your `~/.ssh/config`, as they would for plain `ssh`. An attached remote window stays connected to the computer it was opened from.

### Saved hosts

Give hosts you use often a name in your configuration:

```toml
[domains.ssh_hosts.build]
destination = "me@build.example"
description = "Build server"
```

Then use `--host-profile` instead of `--host`, and find it under **Attach Live Domain…**:

```bash
termeric domain hosts
termeric domain --host-profile build attach DOMAIN
```

A saved host holds only a name, a destination, and an optional description. Keep everything else in `~/.ssh/config`.

## Limits

- The daemon holds at most 16 domains, 16 tabs per domain, and 16 panes across all domains.
- Scrollback is limited to 8 MiB per pane.
- A daemon crash or reboot ends every domain's programs; nothing is saved to disk.
- Clipboard access, notifications, and bells work only while a window is attached, and nothing from a detached period is replayed. There is no remote clipboard.
- Agents running in a domain can't report [agent status](agents.md#agent-status).
- A window showing a domain can't be saved as a workspace; attach the domain instead.
