# Shell integration

Shell integration lets your shell tell Termeric two things it can't reliably work out on its own: which directory you're in, and where each prompt, command, and its output begin and end. With it, Termeric can jump between prompts in your scrollback, find the commands that failed, open new panes in the right directory, and power the experimental Harvest history.

For Bash, Zsh, Fish, and Nushell this is set up automatically. Elvish needs one line in your rc file.

## What you get

- **Prompt jumping.** Ctrl+Shift+Up and Ctrl+Shift+Down scroll to the previous or next prompt in the focused pane, so you can step back through long output one command at a time.
- **Failed-command jumping.** **Jump to Previous Failed Command** and **Jump to Next Failed Command** in the command palette scroll to commands that exited with a non-zero status. They have no default shortcut; bind one on the **Keyboard** page in Settings.
- **Accurate working directory.** Your shell reports its directory at every prompt. Termeric uses it for the tab title, for the directory of new split panes (see `[panes].cwd` in [Configuration](configuration.md)), for Git branch completion, and when saving [workspaces](workspaces-and-sessions.md).
- **Harvest.** The experimental command history described [below](#harvest) is built entirely from shell integration.

Without integration, Termeric falls back to reading the shell's directory from the system, which is usually right for local shells. Prompt and failed-command jumping and Harvest need integration.

Prompt jumping works in the normal screen only. While a full-screen program such as `vim` or `less` is running, the keys do nothing.

## Setup by shell

Termeric uses the shell from `[shell].program` in your configuration, or `$SHELL` if that's not set. Integration is added when that shell (or a [profile](profiles.md) program) starts as an ordinary interactive shell. Shells started to run a script or a `-c` command are left untouched, because changing their startup could change what the command does.

| Shell | Setup | Your configuration |
| --- | --- | --- |
| Bash | Automatic | `~/.bashrc` (or the file you pass with `--rcfile`) loads first; login shells (`bash -l`) load your profile files as usual |
| Zsh | Automatic | `.zshenv`, `.zprofile`, and `.zshrc` load as usual from your `ZDOTDIR` or home directory |
| Fish | Automatic | Your `config.fish` loads as usual; Fish 4 and newer mark prompts themselves, so Termeric adds only directory reporting |
| Nushell | Automatic | Your config loads first; Termeric then turns on Nushell's own `shell_integration.osc7` and `shell_integration.osc133` settings |
| Elvish | One line | Add `use termeric-integration` to your rc file |

For Bash and Zsh, Termeric writes small startup files under `~/.config/termeric/shell-integration/` that load your own configuration and then add the prompt hooks. Fish and Nushell receive their setup as a startup command, so nothing is written for them. Nothing in your own dotfiles is changed.

Integration is skipped when you ask the shell not to read its startup files, for example `bash --norc`, `zsh -f`, `fish --no-config`, or `elvish -norc`, and for Bash in POSIX mode. It's also skipped for shells Termeric doesn't recognize.

Existing tabs keep the setup they started with. After changing `[shell]` or a profile, open a new tab to pick up the change.

### Elvish

Add this line to the rc file Elvish loads, normally `~/.config/elvish/rc.elv`:

```elvish
use termeric-integration
```

Then open a new tab. Termeric makes the `termeric-integration` module available only to Elvish shells it starts, so the module isn't on Elvish's search path in other terminals.

### Remote shells

Integration covers shells Termeric starts on your own computer. A shell on another machine over `ssh` doesn't report prompts or directories unless you set it up to send the same OSC 7 and OSC 133 sequences. When a remote shell does report its directory, Termeric shows it in the tab title but never treats it as a local path, so new panes and Git branch completion don't use it.

### Known limitations

- In Nushell, a directory whose name contains a percent sign followed by two hex digits, such as `%2F`, can be reported as a different path. This comes from Nushell's own directory reporting.
- Prompts that have been removed from the scrollback, for example with **Clear Scrollback** or because they're older than the scrollback keeps, can no longer be jumped to.

## Check whether integration is on

With Harvest (below) turned on, open a new tab and, before running anything, run **Show Sprout Menu** from the command palette. Termeric replies with either "No completed command yet; bash integration is active" (naming your shell) or the reason integration is off, such as "Zsh integration is disabled for command/script, startup-disabled, or unrecognized arguments". For Elvish the reply always repeats the setup instructions; run a command and open **Show Harvest Cellar** instead, and if the command is listed, integration is working.

## Harvest

> **Experimental.** Harvest is off by default. Turn it on with **Enabled** under **Harvest** on the **Experimental** page in Settings, or set `enabled = true` in `[harvest]`. It needs shell integration, its history isn't saved when you quit, and each window keeps its own history.

Harvest keeps a history of the commands you run in a window, with their exit status and output, and lets you act on any of them.

### Root Cellar

Run **Show Harvest Cellar** from the command palette to slide in the Root Cellar on the left. It lists recent commands from every tab and pane in the window, each marked by whether it succeeded or failed, and opens with the newest one selected. Run the command again to close it.

| Key | Action |
| --- | --- |
| Up / Down | Select a command |
| Page Up / Page Down | Move a page at a time |
| Home / End | Jump to the oldest or newest command |
| Enter | Open the sprout menu for the selected command |
| Escape | Close the Cellar |

You can also click a command to open its sprout menu, scroll the list with the mouse wheel, and click outside the Cellar to close it.

### Sprout menu

The sprout menu lists what you can do with a command. **Show Sprout Menu** in the command palette opens it for the latest command in the focused pane; **Hide Sprout Menu** closes it.

| Action | What it does |
| --- | --- |
| Copy output | Copies the command's output to the clipboard |
| Copy command | Copies the command line |
| Re-run | Types the command into the terminal it came from and presses Enter |
| Search | Opens search limited to that command's output |
| Scroll to | Scrolls the terminal to that command |
| Explain | Sends the command and its output to a [coding agent](agents.md) running in Termeric |

Use Up, Down, Home, and End to choose an action, Enter to run it, and Escape to close the menu.

Harvest keeps the last 64 KiB of each command's output, so **Copy output** and **Explain** still work after the text has scrolled out of the terminal's scrollback. When output was longer than that, its start is left out and the menu labels the action "(truncated)". **Search** and **Scroll to** need the text to still be in the terminal, and are disabled when it isn't. **Re-run** is refused for a command whose text was cut short, for a read-only pane, and while broadcast input is on.

### Growth indicators

While a command runs, a small plant grows in the left margin beside it: a seed, then a sprout, then a full plant for long-running commands. When the command finishes, it blooms if it succeeded and withers if it failed. Turn these off with **Growth Indicators** in Settings or `show_indicators = false`.

### Harvest settings

| Setting | Config key | Default | Description |
| --- | --- | --- | --- |
| Enabled | `harvest.enabled` | `false` | Turn Harvest on and add its commands to the palette |
| Growth Indicators | `harvest.show_indicators` | `true` | Show the plant indicators beside running commands |
| History Limit | `harvest.max_history` | `100` | How many commands the Cellar keeps; older ones are dropped, and saved output is also capped at 8 MiB in total |

**Clear Harvest History** in the command palette empties the Cellar.

```toml
[harvest]
enabled = true
show_indicators = true
max_history = 100
```
