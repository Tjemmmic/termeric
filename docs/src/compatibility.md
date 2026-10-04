# Terminal compatibility

This page describes what programs running inside Termeric see: the terminal type it reports, the environment it sets, and the escape sequences it understands. Use it when a program looks wrong, when you connect to another machine, or when you want to know whether a feature your tools rely on is available.

## Environment

Every shell and program Termeric starts gets these variables:

| Variable | Value |
| --- | --- |
| `TERM` | `xterm-termeric` when its terminfo entry is installed, otherwise `xterm-256color` |
| `COLORTERM` | `truecolor` |
| `TERM_PROGRAM` | `Termeric` |
| `TERM_PROGRAM_VERSION` | The Termeric version |
| `TERMINFO`, `TERMINFO_DIRS` | Set so programs find the `xterm-termeric` entry Termeric found (only with `xterm-termeric`) |
| `TERMERIC_TERMINAL_ID` | An identifier for this terminal, used by [agent status reports](agents.md) |

These names are reserved: you cannot override them with `[shell] env` or a profile's `env`.

## Terminal type

Termeric has its own terminfo entry, `xterm-termeric`, which describes exactly what it supports. The Arch and Debian packages install it. Each time Termeric starts a shell, it looks for the compiled entry and uses `TERM=xterm-termeric` only if it finds one; otherwise it uses `TERM=xterm-256color`, which every system understands. It looks in, in order:

1. `$TERMERIC_TERMINFO_DIR`, then `$TERMINFO`
2. `~/.terminfo`, `$XDG_DATA_HOME/terminfo`, and each directory in `$TERMINFO_DIRS`
3. `share/terminfo` next to the Termeric executable's install prefix
4. `/usr/local/share/terminfo`, `/usr/share/terminfo`, `/lib/terminfo`, `/etc/terminfo`

To choose the terminal type yourself, set `TERMERIC_TERM` in the environment Termeric starts from:

| Value | Effect |
| --- | --- |
| `auto` (default) | `xterm-termeric` if the entry is found, otherwise `xterm-256color` |
| `xterm-256color` or `compatibility` | Always `xterm-256color` |
| `xterm-termeric` | `xterm-termeric`, falling back with a logged warning if the entry is missing |

```bash
TERMERIC_TERM=xterm-256color termeric
```

The terminal type belongs to the whole Termeric process. When Termeric is already running with another value, this launch starts a process of its own rather than [joining](command-line.md#joining-the-running-termeric) the running one.

### Installing the terminfo entry

If you built Termeric from source, compile the entry from the source tree into your home directory:

```bash
tic -x -o ~/.terminfo assets/terminfo/xterm-termeric.terminfo
infocmp -x xterm-termeric
```

Or install it for all users with `sudo tic -x -o /usr/local/share/terminfo assets/terminfo/xterm-termeric.terminfo`.

### Remote hosts, sudo, and containers

SSH passes your `TERM` to the remote host, but not the terminfo entry. If the remote host does not have `xterm-termeric`, programs there report `unknown terminal type` or draw incorrectly. Copy the entry from your machine once per host:

```bash
infocmp -x xterm-termeric | ssh host 'mkdir -p ~/.terminfo && tic -x -o ~/.terminfo /dev/stdin'
```

If you cannot install it, use the compatible type for that connection only:

```bash
TERM=xterm-256color ssh host
```

`sudo` usually keeps `TERM` but drops `TERMINFO`, so a copy in your home directory is not visible to root. Install the entry system-wide, or run `sudo env TERM=xterm-256color command`. Container images need the entry compiled inside them, or `TERM=xterm-256color`.

## Colors

Termeric supports the 16 ANSI colors (from your [theme](themes.md)), the 256-color palette, and 24-bit truecolor, in both the `38;2;R;G;B` and colon-separated forms. `COLORTERM=truecolor` and the terminfo entry's `RGB` and `Tc` flags tell programs that truecolor is available.

## Supported sequences

| Family | What Termeric supports |
| --- | --- |
| Text attributes (SGR) | Bold, dim, italic, underline, slow and rapid blink, inverse, hidden, strikethrough. |
| Underline styles | Single, double (`4:2` or `21`), curly (`4:3`), dotted (`4:4`), dashed (`4:5`), and underline color (`58`/`59`). |
| Cursor | Shape and blink with DECSCUSR (`CSI Ps SP q`, Ps 1–6), blink mode 12, visibility mode 25, save and restore. Programs' choices override `[cursor]` until they reset it: Ps 0 (the terminfo entry's `Se`) returns to your `[cursor]` settings, as in foot and kitty, rather than to xterm's blinking block. |
| Screen and editing | Alternate screen (modes 47, 1047, 1049), cursor save mode 1048, scroll regions, left and right margins (mode 69), insert and delete, erase, repeat (REP), tab stops, and the VT400 rectangle operations. |
| Character sets | ASCII, UK, and DEC Special Graphics (line drawing) in G0–G3 with shifts. |
| Bracketed paste | Mode 2004. |
| Focus reporting | Mode 1004. |
| Mouse reporting | Modes 9, 1000, 1002, 1003 with UTF-8 (1005) and SGR (1006) coordinates, and alternate scroll (1007). Hold Shift to select text normally while a program uses the mouse. |
| Synchronized output | Mode 2026. Output is held until the program finishes its update, with a one-second safety limit. |
| Keyboard | Application cursor and keypad modes, xterm `modifyOtherKeys` and the other `modify*Keys` resources, the Kitty keyboard protocol (all five progressive flags), and legacy/VT220 keyboard types (modes 1060 and 1061). |
| Titles (OSC 0, 1, 2) | Set the tab and window title; title reports and the title stack are supported. |
| Colors (OSC 4, 10, 11, 12, 104, 110–112) | Set, query, and reset palette entries and the default foreground, background, and cursor colors, for that terminal only. |
| Working directory (OSC 7) | Used for new tabs and panes, titles, and Git branch completion. See [Shell integration](shell-integration.md). |
| Hyperlinks (OSC 8) | Ctrl+click opens the link. See [Search, copy mode, and links](search-and-copy-mode.md). |
| Notifications (OSC 9, 777, 99) | Desktop notifications, including the Kitty OSC 99 protocol with buttons, icons, urgency, and close reports on Linux. |
| Pointer shape (OSC 22) | Programs can change the mouse pointer over their terminal. |
| Clipboard (OSC 52) | Copying to the clipboard is allowed; reading it is off by default. See `[security.osc52]` in [Configuration](configuration.md). |
| Text sizing (OSC 66) | Larger text blocks. See [Fonts and text](fonts-and-text.md#text-sizing). |
| Shell integration (OSC 133) | Prompt and command marks for prompt navigation and failed-command jumps. See [Shell integration](shell-integration.md). |
| Images | Kitty graphics protocol, Sixel, and iTerm2 inline PNG images (OSC 1337). See [Images and file transfer](images-and-files.md). |
| File transfer (OSC 5113) | Kitty file transfer on Linux, with your approval for each transfer. See [Images and file transfer](images-and-files.md). |
| Multiple cursors | Kitty's multiple-cursor extension for editors that draw extra cursors. |

## Queries and replies

Programs can ask Termeric about itself. Replies go only to the program that asked.

| Query | Reply |
| --- | --- |
| Primary device attributes (`CSI c`) | `CSI ? 1 ; 2 ; 4 ; 8 c`: VT100 with advanced video, Sixel graphics, and user-defined keys. |
| Secondary device attributes (`CSI > c`) | `CSI > 0 ; 1 ; 0 c` |
| XTVERSION (`CSI > q`) | `DCS >\|Termeric(1) ST` |
| XTGETTCAP (`DCS + q`) | Values from the `xterm-termeric` entry, including `TN`, `Co`, and `RGB`. |
| Mode state (DECRQM) | Current state of the modes listed above. |
| Settings (DECRQSS) | Current SGR attributes, margins, cursor style, and related settings. |
| Cursor position and status (DSR) | Standard replies. |
| Window size (XTWINOPS 14, 16, 18) | Text-area size in pixels, cell size in pixels, and size in rows and columns. |
| Kitty keyboard flags (`CSI ? u`) | The active flags. |

Screen checksum reports (DECRQCRA) are off by default because they can be used to read the screen; turn them on with `security.allow_screen_checksum_reports`.

## Known gaps

- Right-to-left and bidirectional text is shown left to right, without reordering.
- Reverse-video screen mode (DECSCNM, mode 5) and overline (SGR 53) are not supported.
- SGR-pixel mouse coordinates (mode 1016) and color-scheme change notifications (mode 2031) are not supported.
- XTWINOPS only reports sizes and titles; programs cannot move, resize, or raise the window.
- iTerm2 file downloads, non-PNG iTerm2 images, and animated iTerm2 images are not supported. (Kitty graphics animation works.)
- The `xterm-termeric` terminfo entry does not list styled underlines (`Smulx`, `Setulc`), cursor shapes (`Ss`, `Se`), clipboard access (`Ms`), or synchronized output. Termeric supports all four, but a program that only checks terminfo will not use them until you enable them in its own settings.
- `xterm-256color` is a compatible name, not a promise of every xterm feature. Prefer `xterm-termeric` wherever you can install it.
