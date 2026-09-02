# Terminal-controlled integration and report security

Termeric implements a bounded, origin-routed subset of xterm operating-system
commands. Both BEL and ST terminators are accepted, parser output becomes typed
`TerminalEvent` values, and every application-mediated reply is written to the
tab or pane that originated the request rather than the currently focused PTY.

## Screen checksum report policy

DECRQCRA lets a terminal application request a checksum of any visible screen
rectangle. Repeated one-cell requests can therefore reveal screen content even
though the response is only a checksum. Termeric recognizes the request but
does not respond by default. Users who trust every process that can write to a
PTY may opt in explicitly:

```toml
[security]
allow_screen_checksum_reports = true
```

The setting applies to existing and future tabs and panes after a live config
reload. A domain server reads it at daemon startup, so an already-running
domain must be restarted after changing it. When enabled, Termeric reports the
base VT420/xterm 16-bit algorithm; VT525 color contributions and XTCHECKSUM
extensions are not enabled.

## Titles

OSC 0 and OSC 2 set the visible tab title. The active tab title is also
reflected in the native window title as `<tab> — Termeric` (or the quick-access
variant); inactive-pane/tab updates cannot retitle the current window.
Semicolons inside the title are
preserved, invalid UTF-8 is rejected, control characters are removed, and the
raw title limit is 4,096 bytes. An explicit non-empty OSC title takes precedence
over working-directory and foreground-process discovery across subsequent
output. An empty OSC 0/2 title clears that precedence and immediately returns to
the normal shell-derived title (or the stable `Tab N` fallback).

For pane-owned terminals, the latest valid OSC title from that tab is the tab's
visible title. The stable pane origin is still retained while routing, so an
inactive tab can update its own title but never another tab's. A stale or
unknown pane origin is rejected without changing the tab, and an empty title
also clears pane-local title metadata so an explicit empty value is not saved.

## Working directories

OSC 7 accepts a bounded `file://host/absolute-path` report under either BEL or
ST termination and 7-bit or 8-bit OSC framing. Semicolons in the path are
reassembled before parsing; `%HH` escapes are decoded strictly; malformed
escapes, invalid UTF-8, control bytes, user-information authorities, non-file
schemes, relative paths, and reports above 4,096 bytes are rejected. A rejected
report leaves the last valid value unchanged. The URI, authority, and decoded
path are owned by the exact terminal that parsed them and survive presentation
resets and single-terminal/pane ownership transfer.

For local filesystem actions, an empty authority, `localhost`, IPv4/IPv6
loopback, or the exact local hostname is required. A different authority may
still supply the directory portion of a tab title, but it is never passed to
local Git inspection, plugins, pane/process spawn, session/workspace capture,
or local-domain control. This prevents a remote shell from making a remote path
look like a valid local cwd.

Normal interactive Bash, Zsh, and Fish launches emit a hostless OSC 7 report at
each prompt, with literal percent signs escaped before transmission. An
unambiguous Nushell REPL launch enables Nushell's native OSC 7 and OSC 133
settings after the user's normal configuration loads; its real-PTY result is
still acceptance-pending. Nushell scripts, command strings, language-server/IDE
modes, and existing pre-REPL commands are preserved without injection. The
generated Elvish module emits the same hostless OSC 7 and OSC 133 lifecycle
after the user adds `use termeric-integration` to `~/.config/elvish/rc.elv`.
Termeric only exposes that module to rc-loading interactive Elvish launches;
scripts, command strings, daemon/language-server modes, and `-norc` launches
remain untouched. Its real-PTY result is acceptance-pending because Elvish is
not installed on the current runner. The
reported local directory takes precedence over Linux `/proc` discovery;
process inspection remains a fallback for direct commands, unsupported shells,
and shells whose integration is disabled. User and OSC 0/2 titles continue to
outrank the derived cwd title.

## OSC 52 clipboard policy

Termeric recognizes clipboard (`c`/`s`/empty) and primary-selection (`p`)
targets, queries (`?`), padded or unpadded standard base64 writes, and empty
writes. Invalid base64, invalid decoded UTF-8, unsupported targets, and payloads
over the hard 1 MiB decoded ceiling are rejected without a clipboard mutation.
Linux uses the distinct clipboard/primary selections exposed by `arboard`;
other platforms map the primary target to their normal system clipboard.

The default security policy permits writes for remote-copy workflows and denies
reads to prevent silent clipboard exfiltration:

```toml
[security.osc52]
allow_read = false
allow_write = true
max_bytes = 1048576
```

`max_bytes` is validated in the range 1–1,048,576 and applies to both clipboard
contents returned by a query and decoded writes. A denied, unavailable, or
oversized read receives an empty `OSC 52` reply terminated by ST so the client
does not hang and no contents leak. Denied/oversized writes are ignored and
logged. The parser also caps and discards the complete raw OSC sequence before
dispatch, then releases its temporary buffer.

## Dynamic colors

The supported color family is:

| OSC | Behavior |
|---|---|
| 4 | Set or query one or more indexed palette entries (0–255) |
| 10 / 11 / 12 | Set or query default foreground, background, or cursor color |
| 104 | Reset named palette entries, or all entries when no index is supplied |
| 110 / 111 / 112 | Reset default foreground, background, or cursor color |

Set values accept `rgb:R/G/B` with one to four hexadecimal digits per component
and `#RGB`, `#RRGGBB`, `#RRRGGGBBB`, or `#RRRRGGGGBBBB`. Query replies use
xterm's 16-bit `rgb:rrrr/gggg/bbbb` form and ST. Malformed indexes/specifications
are ignored.

Overrides live on the originating terminal, including inactive tabs and
unfocused panes. Indexed/default foreground and background affect subsequent
and already-stored cells at render time; a default-background override paints
the pane viewport; OSC 12 affects that terminal's cursor. Each mutation advances
a terminal-local generation used by the row cache. Resets reveal the current
theme, and theme reloads update that base without deleting active overrides.

## Verification

Unit tests cover BEL/ST and chunked parsing, empty fields, semicolon titles and
directories, OSC 7 percent decoding/authority policy, invalid
UTF-8/base64/color data, hard limits and recovery, query formatting, security
defaults, reset/generation behavior, and theme/override resolution.
Application integration tests apply distinct title and palette operations to
two pane origins on an inactive tab, while the existing real-PTY route fixture
proves an OSC 52 denial reply returns only to its requesting pane.

Native release acceptance must still exercise allowed clipboard reads/writes
and primary selection under the actual Wayland/X11 desktop permission policy;
that environment-dependent row cannot be established by a headless test.
