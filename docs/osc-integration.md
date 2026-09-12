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

OSC 0 sets both the application window label and application icon label, OSC 1
sets only the icon label, and OSC 2 sets only the window label. The application
window label drives the visible tab title. The active tab title is also
reflected in the native window title as `<tab> — Termeric` (or the quick-access
variant); inactive-pane/tab updates cannot retitle the current window.
Semicolons inside the title are
preserved, invalid UTF-8 is rejected, control characters are removed, and the
raw limit is 4,096 bytes per label. An explicit non-empty OSC window label
takes precedence over working-directory and foreground-process discovery
across subsequent output. An empty OSC 0/2 title clears that precedence and
immediately returns to the normal shell-derived title (or the stable `Tab N`
fallback).

For pane-owned terminals, the latest valid OSC title from that tab is the tab's
visible title. The stable pane origin is still retained while routing, so an
inactive tab can update its own title but never another tab's. A stale or
unknown pane origin is rejected without changing the tab, and an empty title
also clears pane-local title metadata so an explicit empty value is not saved.

The application-owned labels—not the displayed tab or native-window title—are
also the complete data source for these xterm-compatible operations:

- `CSI 20 t` reports the icon label as `OSC L label ST`;
- `CSI 21 t` reports the window label as `OSC l label ST`;
- `CSI 22 ; target ; slot t` saves and `CSI 23 ; target ; slot t` restores,
  where target 0 selects both labels, 1 selects the icon, and 2 selects the
  window;
- omitted slot 0 pushes or pops the xterm ring, while slots 1 through 10
  directly store or retrieve without moving its position; and
- `CSI # S` reports `CSI position ; 10 # S`.

The fixed 10-slot ring preserves xterm's partial-entry fallback: restoring both
labels from a slot that saved only one searches older slots for the other. A
push replaces the next ring entry, a pop clears its entry, and a direct
retrieval does not. Current labels and saved entries are global to the terminal
owner and survive alternate-screen changes, DECSTR, and RIS. At the 4 KiB
per-label limit, current and saved strings retain at most 90,112 payload bytes.

Every reply uses the requesting terminal's S7C1T/S8C1T mode and stays with that
terminal's PTY. A query cannot reveal a user-renamed tab, derived process name,
working directory, native title, or another pane. Unsupported targets, slots,
extra query parameters, and other window/title mutations are ignored. This
behavior follows xterm's
[control-sequence contract](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html).

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

## Desktop notifications

Legacy OSC 9 messages and OSC 777 `notify` title/body requests emit the same
typed application event. Their fields are escape-safe UTF-8, preserve body
semicolons, and are capped at 4 KiB for a title and 16 KiB for a body. Invalid
UTF-8, C0/C1 controls, and oversized fields are rejected before reaching the
desktop helper.

Termeric also implements a bounded subset of Kitty's
[OSC 99 notification protocol][kitty-notifications]. Metadata keys `i`, `p`,
`d`, `e`, `w`, `c`, and `a` provide strict identifiers,
title/body/close/alive/buttons selection, completion, plain or RFC 4648 base64
payloads, expiry policy, and close/activation-report requests. The `f`, `t`,
and `n` keys add base64-encoded
application identity, ordered notification types, and ordered icon-name
candidates; `u=0/1/2` is retained as typed low/normal/critical urgency. Plain
chunks are limited to 2,048 bytes;
encoded chunks are limited to 4,096 bytes and accept Kitty's encode-before-
chunking and encode-after-chunking forms, including an unpadded final chunk.
Assembled titles and bodies retain at most 4 KiB and 16 KiB respectively.
Button payloads retain at most 4 KiB before decoding into at most eight
non-empty labels of 256 UTF-8 bytes each, separated by U+2028.

Up to 16 identified notifications may be incomplete at once under a 256 KiB
aggregate payload ceiling. Interleaved identifiers remain isolated, the least
recently updated assembly is evicted when either ceiling is crossed, and an
invalid continuation retires only its own assembly rather than allowing stale
text to be completed later. Pending state is global across screen changes and
DECSTR, while RIS clears it. A completed body-only message promotes its body to
the title as Kitty specifies; an empty message is ignored.

Each decoded application/type/icon metadata value is limited to 256 bytes.
At most eight unique notification types and eight unique icon candidates are
retained per request, in first-seen order; repeated values are deduplicated.
Metadata allocations count toward the aggregate pending-state ceiling. Invalid
base64, UTF-8, C0/C1 controls, invalid urgency, duplicate singular keys, and
per-list overflow fail closed and retire only the affected identified assembly.
Expiry accepts exactly `-1` for desktop policy, `0` for no automatic expiry, or
a positive millisecond value no greater than `2147483647`. A completed,
payload-free `p=close` request requires an identifier and also retires any
incomplete parser assembly carrying that identifier. `c` accepts only `0` or
`1`. The `a` value is a strict comma-separated combination of `report`,
`-report`, `focus`, and `-focus`; duplicate, contradictory, empty, or unknown
actions are rejected. Only report is advertised and implemented. A
payload-free `p=alive` query is completed immediately; payload-bearing or
continued alive queries are rejected. Button labels reject empty entries and
C0/C1 controls.

`OSC 99 ; i=id:p=? ; ST` receives an owner-routed response under the requesting
terminal's S7C1T/S8C1T framing. Linux responds with
`p=title,body,close,alive,buttons:a=report:c=1:o=always:u=0,1,2:w=1`; other targets respond with
`p=title,body:o=always`. The platform split keeps lifecycle and urgency
discovery truthful.
Application/type/icon-name metadata has no capability-query key. Non-`always`
occasion requirements are rejected. Native focus, application-side filtering,
sound selection, transmitted icon data, and icon caching are not advertised or
claimed.

Unidentified requests with desktop-default expiry enter the existing
rate-limited helper path. On Linux, requests needing lifecycle semantics use a
dedicated bounded worker and the freedesktop D-Bus `Notify` and
`CloseNotification` methods. Stable desktop IDs provide replacement and exact
close authority scoped to the originating tab/pane; equal client IDs from two
origins cannot affect each other. Positive expiry starts after `Notify` returns,
is also passed to the service, and makes up to three terminal-side close
attempts. The command and completion queues and tracked-handle set are each
capped at 64, as is the pending close-report set. A single ordered signal
listener subscribes to freedesktop notification signals before the first
tracked notification is shown. `p=buttons` becomes revision-scoped private
action-key/label pairs in the D-Bus `Notify` call; `a=report` additionally
registers the required
`default` action. A matching `ActionInvoked` emits exactly one OSC 99 activation
reply to the originating terminal, with an empty payload for the default action
or the one-based button number for a button. Unknown, stale, oversized, and
duplicate actions are ignored. Activation does not consume close-report state,
so a following native close still produces its requested report. When `c=1`
was requested, native user, service-policy,
explicit, and expiry closure emits exactly one `p=close` report to the original
tab or pane using the framing captured by that request. Replacement adopts only
the newest successful request's close policy; replacing a revision does not
report the old revision closed. Explicit/expiry closes wait up to five seconds
for native confirmation and then fail visibly and answer with `untracked`
instead of leaving the client waiting forever. `p=alive` returns a sorted,
comma-separated snapshot of named live IDs owned by that exact terminal;
anonymous and already-closing entries are excluded. Stale terminal origins are
dropped with no focused-terminal fallback. Native `ActivationToken` is
deliberately ignored because compositor-authorized focus is not yet implemented
or advertised.

D-Bus methods have a two-second timeout, and evicting the oldest handle makes a
best-effort close. Repeated worker failures are deduplicated until a successful
operation recovers the path. A stopped signal stream is surfaced once and the
next tracked request may replace it, but no timer-driven reconnect loop or
additional concurrent listener is created. Notification text is markup-escaped
before it reaches the service.

Legacy Linux helper delivery still maps `f`, `t`, the first `n` candidate, and
`u` to separate `notify-send` app-name, category, icon, and urgency arguments.
Kitty's universal semantic icon names are mapped to common freedesktop names;
other names are delegated to the desktop icon resolver. macOS retains
title/body helper behavior but does not advertise lifecycle support. Hermetic
tests use an in-memory backend and do not contact a notification service;
they cover ordered action/close state, default and button reports, replacement
authority, close signals, alive snapshots, timeouts, shutdown, and exact
delayed-reply routing without a session bus. Real-compositor permission,
presentation, and daemon action/signal behavior remain manual acceptance.

[kitty-notifications]: https://sw.kovidgoyal.net/kitty/desktop-notifications/

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

## Mouse pointer shapes

OSC 22 implements the [Kitty pointer-shape protocol][kitty-pointer-shapes] and
its xterm-compatible plain single-name
form. Termeric accepts all 30 required CSS-derived names. A plain name or
`=name` replaces the current stack entry, an empty payload clears the active
stack, `>` atomically pushes a comma-separated list, and `<` pops one entry.
Each terminal owns independent primary- and alternate-screen stacks backed by
fixed 16-entry arrays; pushing past the limit evicts the oldest entry. DECSTR
and RIS clear both stacks.

Queries beginning with `?` return `1` or `0` for supported or unknown valid
names. `__current__` returns the current name or `0`, `__default__` returns
`text`, and `__grabbed__` returns `default`. Replies use ST, follow the
requesting terminal's S7C1T/S8C1T framing, and are consumed by that exact PTY
owner rather than forwarded as terminal text. Payloads above 1,024 bytes,
lists above 64 entries, malformed list members, non-canonical case, invalid
bytes, and unsupported set/push names are rejected atomically.

All required names map one-to-one onto Winit cursor icons. The empty-stack
runtime policy is the text-selection cursor for an ordinary terminal and the
native default cursor while mouse reporting is active. Termeric-owned selection,
pane-divider resizing, and native file drag-and-drop may temporarily override
the requested shape; the terminal shape is restored when that interaction
ends. Only the focused visible terminal may update the OS cursor, while
background tabs and panes retain their own state. Automated tests cover the
parser, stacks, replies, event bridge, and exhaustive mapping. Native
X11/Wayland visual confirmation for every desktop cursor theme remains a
deferred release-acceptance row; legacy X11 cursor-font aliases are not
implemented.

[kitty-pointer-shapes]: https://sw.kovidgoyal.net/kitty/pointer-shapes/

## Verification

Unit tests cover BEL/ST and chunked parsing, empty fields, semicolon titles and
directories, OSC 7 percent decoding/authority policy, invalid
UTF-8/base64/color data, hard limits and recovery, query formatting, security
defaults, reset/generation behavior, theme/override resolution, and bounded
OSC 22 set/push/pop/query and primary/alternate-screen behavior. The app test
rail additionally exhausts all 30 protocol-to-Winit pointer mappings.
Application integration tests apply distinct title and palette operations to
two pane origins on an inactive tab, while the existing real-PTY route fixture
proves an OSC 52 denial reply returns only to its requesting pane.

Native release acceptance must still exercise allowed clipboard reads/writes
and primary selection under the actual Wayland/X11 desktop permission policy;
that environment-dependent row cannot be established by a headless test.
