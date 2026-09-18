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

The generated Bash and Zsh emitters percent-encode every pathname byte except ASCII
letters/digits and `/._~-` before framing OSC 7. Ordinary ASCII paths use a fast
path; encoding uses shell builtins with function-local byte-oriented locale and
does not spawn a helper or change the caller's locale. This prevents BEL, ESC,
and raw C1 bytes in directory names from terminating the report or becoming
other terminal commands. An isolated Bash reproduction previously turned a
synthetic directory value into an independent title-change event. Zsh has the
same real-shell regression. Its encoder additionally localizes shell options
with `emulate -L zsh` and disables multibyte interpretation within the helper;
the caller's indexing, splitting, multibyte options, and locale are restored.
This also replaces the old Zsh percent-substitution expression, which appended
`%25` even to ordinary paths because an unescaped `%` anchors its pattern at the
end of the value. See the upstream [Zsh expansion reference](https://zsh.sourceforge.io/Doc/Release/Expansion.html)
and [local option semantics](https://zsh.sourceforge.io/Doc/Release/Options.html).

Bash/Zsh raw input and encoded output are bounded to 4,089 bytes, leaving seven
bytes for `file://` within the receiver's 4-KiB URI ceiling. Empty, relative, or
oversized reports are omitted as a whole, never truncated; prompt/command
markers still proceed. Percent encoding does not make control-bearing or
invalid-UTF-8 directories eligible for cwd inheritance: the receiver retains
its existing rejection policy and last valid report. Long encoded Unicode paths
can reach the URI ceiling earlier than ASCII paths.

Fish uses its builtin [`string escape --style=url`](https://fishshell.com/docs/current/cmds/string-escape.html)
before emitting the report. A 4,089-character input precheck bounds encoding
work; Fish's string length is character-based, so the authoritative 4,089-byte
check is performed on the ASCII encoded result. There is no truncated or
partially emitted URI. The helper does not change the caller's locale. Real
Fish 3 and Fish 4 tests use temporary directories containing controls, URI
punctuation, UTF-8, and every nonzero Unix pathname byte; they also verify that
other prompt handlers continue when a real directory's encoded report is too
long. Fish 3's OSC 133 prompt marks remain intact; Fish 4's native OSC 133
producer is outside these non-interactive event tests.

Elvish's opt-in module now applies the same 4,089-byte raw/encoded ceiling in
both its directory-change and before-readline hooks. It keeps an ASCII fast
path and percent-encodes other bytes using builtin string/numeric operations;
individual non-ASCII bytes are never incorrectly decoded as standalone UTF-8.
See the upstream [byte conversion reference](https://elv.sh/ref/str.html#strto-utf8-bytes)
and [byte-count semantics](https://elv.sh/ref/builtin.html#count).
Real-shell encoder tests load the unchanged module with a stub editor-hook
registry and synthetic cwd values, covering both hooks, existing user hooks,
all nonzero Unix bytes, URI/Unicode round trips, and exact size boundaries.
A separate isolated Elvish 0.21.0 real-PTY test verifies the actual opt-in
startup/editor wiring and complete command regions; this is not desktop or
user-configuration acceptance.

These producer fixes cover **Bash, Zsh, Fish, and opt-in Elvish**. Nushell uses
its own native producer and has the confirmed gaps below. This does not sanitize
arbitrary user prompt code or output from other applications.

### Elvish launch and opt-in module handling

Termeric exposes its module only to recognized rc-loading Elvish REPLs.
The check follows [Elvish's invocation modes](https://elv.sh/ref/command.html)
and [Go flag syntax](https://go.dev/src/flag/flag.go): one or two leading dashes,
separate/`=` string values, typed booleans/integers, last boolean value wins,
and a final `--`. Values that look like flags remain values. An operand after
`--` is still a script filename; `-i` does not turn a script into a REPL.

Active help/version/buildinfo, language-server/daemon, and no-rc modes run with
unchanged argv/environment and no generated integration files. Unknown,
grouped, abbreviated, or malformed options also bypass integration instead of
guessing their meaning. Later `=false` values can restore eligibility.
With no operands, native Elvish still runs its REPL with `-c`, `-compileonly`,
or `-json`; these flags do not by themselves disable module exposure.

The opt-in contract is unchanged: add `use termeric-integration` to the rc file
Elvish actually loads, including a custom `-rc` file. Termeric does not rewrite
that file or its arguments and does not claim automatic integration. An empty
rc option uses Elvish's native default; multiple rc options use the last value.
Module lookup/handoff policy is separate from argument classification.

Two routing tests verify exact pass-through and absence of generated files
for rejected launches. Fifty typed-option cases are compared with Elvish
0.21.0's native parser, including numeric bases, separators, range limits, and
boolean spellings. Five additional isolated real-PTY variants verify custom rc
selection, boolean overrides, flag-looking paths, first-prompt cwd, and complete
multiline command regions. These do not establish arbitrary user-configuration
or desktop acceptance.

### Elvish module-search environment handoff

Launch preparation records both the original `XDG_DATA_DIRS` presence/value
and the exact temporary value it installs. A nested launch unwraps that value
only while it still matches the recorded transport. A profile's different
value takes precedence over stale markers. The user path order and repeated
entries are retained; both unset and explicitly empty values retain Elvish's
[default system module roots](https://elv.sh/ref/command.html#module-search-directories),
while their distinct environment presence is saved for restoration.

At module import, the original environment is restored only if the current
value is still that same transport. Rc-directed replacement, emptying, or
unsetting wins. Importing the generated module without transport markers no
longer unsets an unrelated user path. All three private transport markers are
cleared before later child commands. Elvish's already-initialized module search
list remains available to the current interpreter: this is an environment
handoff, not a rewrite of its module registry. Children launched **before** the
explicit import can still inherit the unconsumed transport.

Older two-marker launches are recovered during preparation only when their
exact layout matches the known current integration root. An unidentified old
root or a markerless import is left intact rather than guessed away; this can
retain an old path until a fresh launch without that inherited transport.

Three pure environment tests cover profile precedence, nested/legacy recovery,
path order, and empty/unset defaults. Ten isolated real-Elvish module executions
check exact restoration, rc mutations, markerless imports, hook installation,
and child environment cleanup. Three actual editor/PTY variants additionally
import the module after rc path replacement/emptying/unsetting, verify a child
process, and require first-prompt cwd and complete multiline command regions.
These tests use isolated configuration, not the user's desktop session.

### Fish launch and startup-command handling

Fish integration is limited to recognized REPL launches. Grouped
interactive/login/private flags, diagnostic/profiling options, and separate or
attached startup-command values follow [Fish's invocation syntax](https://fishshell.com/docs/current/cmds/fish.html).
Option values are opaque: a filename or command named `--no-config` is not a
second option. A final `--` is preserved; any script operand, including `-` or
`login`, bypasses integration. Command, no-config/no-execute, utility,
feature-grammar, abbreviated, malformed, and unknown options run unchanged
without injected arguments or environment changes. This is an eligibility
allowlist, not a complete CLI validator or a ban on those shell options.

Termeric appends its own separate `--init-command` after all user startup
commands, before a final end marker. It neither concatenates user source nor
rewrites configuration. Native rc loading and the order and exact text of user
commands remain intact. Setup preserves the preceding scalar `$status` and
removes its temporary status helper/local variable; this does not promise
preservation of the full `$pipestatus` vector or arbitrary custom shell state.
User code that exits or replaces Fish before setup still prevents integration.

Two routing regressions, six native startup comparisons per tested version
(Fish 3.7.1 and 4.9.3), and four Fish 4.9.3 real-PTY variants cover this boundary.
They check startup order, punctuation/comments, scalar statuses 0/1/23, user
hooks, first-prompt cwd, and multiline success/failure regions. They do not
replace real user-configuration or desktop acceptance.

### Bash launch and custom-rc handling

Termeric uses an rc wrapper for recognized interactive **non-login** Bash launches.
Commands/scripts, POSIX/no-rc modes, debugger/restricted/privileged modes,
and unknown argument forms keep their exact program, argv, and environment
without generating an rc file. Presence of `POSIXLY_CORRECT` (even empty), or
`posix` in effective `SHELLOPTS`, also leaves the launch untouched: these modes
do not use normal `.bashrc` startup. They report integration unavailable,
not a successful hook installation. Recognized login shells use the separate
[login bootstrap](#bash-login-shell-integration) described below.

The classifier preserves Bash's long-options-before-short-options rule,
recognized grouped flags, separate `-o`/`-O` values, interactive/stdin mode,
and `-`/`--` boundaries. With `-s`, positional parameters are not reinterpreted
as options or scripts. Common editing, history, globbing, tracing, and
unset-variable options are recognized; this is not a complete Bash CLI
validator. Unrecognized forms still run unchanged, without integration.

For an explicit `--rcfile FILE` or `--init-file FILE`, only the **last** rc value
is replaced with the generated wrapper; all other argv words retain their
positions and contents. The requested path is carried as data, not interpolated
into shell code. The wrapper clears its private exported markers before
loading user configuration at file scope. Relative paths are pinned to the
startup working directory so `source` cannot select a different file from
`PATH`. Empty/missing custom paths do not fall back to HOME. Re-preparing an
unconsumed launch recovers its original custom/default selection instead of
recursively sourcing a generated wrapper.

These rules were checked against the installed Bash 5.3.15 documentation and
binary; see also the upstream [startup-file contract](https://www.gnu.org/software/bash/manual/html_node/Bash-Startup-Files.html).
Pure routing/reentry cases, seven isolated startup comparisons against plain
Bash, and four real-PTY variants cover native file selection, shell-punctuation
filenames, a PATH decoy, child-environment cleanup, unchanged rc bytes, exact
stdin parameters, first-prompt cwd, and multiline success/failure regions.
Arbitrary startup-environment/option combinations and older Bash runtime
versions are not established by these new tests. The separate existing Bash
prompt-array/version fallback tests remain in place.

Hook installation temporarily disables `allexport` (`set -a`) and restores it
before returning to the user. This prevents function definitions and prompt
composition from accidentally exporting private hooks or previously unexported
`PROMPT_COMMAND`/`PS0`. Existing explicit export attributes on user prompt
variables remain intact; their values still include the installed integration,
so explicitly exporting prompts is not a promise of clean nested-shell prompt
composition. Private function export attributes inherited from older wrappers
are removed, inherited command-seen state is discarded, and subsequent prompt
updates keep that state unexported even if the user enables `set -a` later.
Eight isolated child-environment cases and scalar/array real-PTY comparisons
cover these boundaries on Bash 5.3.15. This does not prevent user code from
explicitly exporting private names or reassigning prompt variables later.

### Bash login-shell integration

Recognized interactive `--login`, `-l`, grouped `-il`, and `+l` launches now
receive OSC 7/133 integration. Bash treats `+l` as a login request, not a way
to undo `-l`. Command/script, explicit POSIX, no-rc, restricted/privileged,
and unknown modes retain their original launch; `--noprofile` is supported.

The generated bootstrap uses Bash's POSIX `ENV` startup entry point, then
restores ordinary Bash mode **before** loading `/etc/profile` and the first
readable regular user profile in `.bash_profile`, `.bash_login`, `.profile`
order. These files run at file scope, not inside a helper function. Login
identity and native `.bash_logout` behavior remain intact. `--noprofile`
suppresses profile loading, and login `--rcfile` arguments remain natively
ignored rather than redirecting `.bashrc` startup.

The broad temporary-POSIX approach is also used by
[kitty](https://raw.githubusercontent.com/kovidgoyal/kitty/master/shell-integration/bash/kitty.bash)
and [Ghostty](https://raw.githubusercontent.com/ghostty-org/ghostty/main/src/shell-integration/bash/ghostty.bash).
Termeric keeps its existing prompt/cwd hooks shared between both startup paths.
Its bootstrap restores `ENV` presence/value, native history defaults/export
state, and inherited `inherit_errexit` state. Transport metadata is removed
before profiles run. Parameter indirection keeps generated paths containing
dollar signs or command-substitution syntax literal. Re-preparing the same
unconsumed login launch retains one bootstrap and the original `ENV`; manual
unprepared sourcing does not rerun login startup.

Eighteen isolated Bash 5.3.15 startup comparisons cover profile priority and
fallback, no-profile mode, logout, file-scope declarations, `ENV`, history,
inherited errexit behavior, and punctuation-bearing paths. Three
environment-cleared real-PTY variants require the first cwd report and complete
multiline success/failure regions. Startup uses the host's standard
`/etc/profile` layout; custom-compiled system-profile paths, arbitrary rc code,
older runtimes, and native desktop acceptance are not proven by these cases.

### Zsh configuration-directory handoff

Before generating startup files, Termeric checks recognized Zsh invocation
forms in argument order. Command/script, help/version, emulation/privileged,
startup-disabled, and unknown forms keep their program, argv, and environment
unchanged and report integration unavailable. No generated directory is
created for these launches. This fixes the previous `-f`/`+o rcs` behavior,
which redirected `ZDOTDIR` even though Zsh would never read the wrapper.

The check follows the [native Zsh invocation grammar](https://zsh.sourceforge.io/Doc/Release/Invocation.html)
and [option-name inversion rules](https://zsh.sourceforge.io/Doc/Release/Options.html):
it handles grouped short flags, `+` inversions, separate/attached `-o` values,
case-insensitive names with ignored underscores, GNU-style hyphenated names,
and end markers (`--`, `+-`, `-`, `+`, `-b`, and a final grouped `-`). Later
options can restore eligibility, as with `-f +f` or `+o rcs -o rcs`.
With `-s`, subsequent operands are positional parameters, not script names;
option-looking words after an operand/end marker are not parsed again.
Recognized options cover interactive/login, RCS/global-RCS, stdin input,
tracing/verbosity, unset-variable handling, and ZLE. This is not a complete
Zsh CLI validator: other options still run unchanged, without injected
integration, pending explicit support. Bash and Fish have their own handling
described above.

Pure routing cases verify exact pass-through, no generated files on rejection,
option ordering, and operand boundaries. Four isolated real-PTY variants verify
short/long/named re-enabling plus exact `-s` positional arguments, user rc
loading, first-prompt cwd, and complete multiline success/failure regions.

Launch preparation captures `ZDOTDIR` from the effective profile environment,
falling back to the inherited environment only when the profile does not set
it. It then uses a generated directory temporarily to forward `.zshenv`, login
`.zprofile`, and `.zshrc`. Each wrapper restores the user's value (or unsets it)
and removes the exported transport markers **before** sourcing user configuration
at file scope. Ordinary `typeset` declarations therefore remain global, and
child shells launched by that user code do not inherit the transport. A custom
directory with no readable `.zshrc` does not fall
back to `$HOME/.zshrc`; [Zsh uses HOME only when ZDOTDIR is unset](https://zsh.sourceforge.io/Doc/Release/Files.html).

Explicitly empty and unset values remain distinct. If a nested Termeric launch
inherits an unconsumed generated directory, it recovers the recorded original
instead of stacking transports. A different profile path overrides stale
markers. Older single-marker launches are recovered when their original path
is available; if an older launch already overwrote it with the generated path,
the original cannot be recovered and HOME is used rather than recursively
sourcing Termeric's wrapper. Early stages also retain whether the user's
`ZDOTDIR` is exported. Changes to its value/export state are carried into the
next stage; after `.zshrc`, normal Zsh processing loads `.zlogin`/`.zlogout`
from the resulting user directory. The wrappers do not manually source global
startup files or reorder login/logout execution.

Pure regressions cover profile precedence, unique environment entries,
unset/empty/custom recovery, and legacy recursion prevention. Real Zsh startup
subprocess comparisons against plain Zsh cover custom and missing rc files,
unset `ZDOTDIR`, rc-directed path changes, and nested child startup. These
fixtures use isolated configuration and append `-c` after launch preparation
to inspect actual interactive startup; they do not prove command-argument
eligibility or visual/PTY acceptance. The existing real-PTY shell probe checks
ordinary prompt and command-region behavior separately. Nine additional
startup-chain comparisons cover interactive/login order, directory redirects
from `.zshenv` and `.zprofile`, global declaration scope, login/logout files,
disabled `RCS`, readonly/unset/non-exported `ZDOTDIR`, and `nounset`. An isolated
real-PTY login probe loads a user `.zprofile` that enables `nounset`, verifies
the exact cwd at the first input-ready prompt, and completes multiline
success/failure commands with the expected regions. The first prompt no longer
reads an unset internal seen-command variable under `nounset`.

If a user stage disables `RCS` or makes `ZDOTDIR` readonly, forwarding stops and
leaves native startup at the user's directory; semantic hooks may consequently
be absent. A remaining boundary is **global startup code**: between forwarded
stages it can still observe the temporary directory, redirect it, or disable
reading before the next wrapper is reached. These tests disable optional global
startup files with `-d`; arbitrary system startup scripts and all command-line
option combinations are not accepted by this evidence. Full configuration
acceptance remains incomplete.

### Nushell launch eligibility

Termeric injects its pre-REPL setup only for recognized interactive Nushell
argument forms. It preserves program, argument boundaries, and environment
exactly for commands/scripts, help/version, IDE/LSP/MCP modes, user `--execute`
preludes, unknown options, and ambiguous variable-arity options. These launches
still run normally; Harvest reports integration unavailable. No generated
startup files are written for them. This avoids assuming that every argument
beginning with `-` describes an interactive shell.

Recognized REPL options include interactive/login/no-config/history/stdlib
switches and scalar configuration, include-path, plugin, logging, table, and
error-style options. `--config-home` and `--log-file` now consume their values
correctly. Long `--name=value`, grouped `-iln`, and value-taking short forms
such as `-im rounded`, `-im=rounded`, and `-I library` preserve the original argv.
A final bare `--` is allowed; anything after it is a script operand, even if it
looks like a flag. Unknown and variable-arity forms are passed through without
integration instead of guessing where their values end. This is an integration
eligibility check, not a replacement for Nushell's validation of option values.

The argument grammar is checked against [Nushell 0.115.1's CLI implementation](https://github.com/nushell/nushell/blob/0.115.1/src/command.rs)
and the actual binary. Pure routing regressions cover rejected modes without
starting services or plugins. Three isolated real-PTY variants cover custom
configuration paths with spaces, separate/equals values, grouped short flags,
and the end-of-options delimiter, including first-prompt cwd and complete
success/failure command regions. Older-version `--threads` scalar routing is
retained but not runtime-tested on 0.115.1, where that option was removed.

### Nushell native producer: confirmed gaps

An isolated Nushell 0.115.1 real-PTY probe passes ordinary-directory startup,
custom rc preservation, multiline execution, terminal-query replies, and
success/failure Harvest regions. Stronger directory checks found two problems:

- A literal `%2F` in a real directory name is transmitted without escaping its
  percent sign; strict URI decoding turns it into `/`, changing the path.
- **Fixed:** with `shell_integration.osc7 = false` in user config, the prelude
  previously enabled later reports but not the first prompt's report.
  Termeric now emits one bounded, correctly encoded hostless initial report
  when the native initial-report snapshot is disabled, then enables native
  reporting for subsequent commands. An already enabled native initial report
  is not duplicated. No startup file or user hook is replaced.

These observations match the [Nushell 0.115.1 REPL implementation](https://github.com/nushell/nushell/blob/0.115.1/crates/nu-cli/src/repl.rs):
its OSC 7 encoder escapes controls but not literal percent signs, and startup
OSC flags are captured before the pre-REPL command runs. The terminal's strict
URI parser is not at fault and must not be relaxed to accept ambiguous paths.

The literal-percent `nushell_native_cwd_` regression remains an explicitly
ignored **known failure**, not accepted behavior or green release evidence.
The first-prompt regression now passes and is part of the ordinary Unix suite
(it explicitly self-skips without `nu`, or fails if Nushell acceptance is
required). With `nu` available, reproduce the remaining native failure using:

```bash
cargo test --workspace --locked nushell_native_cwd_ -- --ignored --nocapture
```

The supplemental encoder accepts absolute Unix paths, enforces 4,089-byte raw
and encoded limits, percent-encodes controls and URI punctuation, and restores
only encoded path separators. It omits empty, relative, or oversized reports
as a whole. Real Nushell helper tests verify Unicode/percent round trips,
injection prevention, exact size limits, and conditional/non-duplicated output;
the real PTY test checks the first actual prompt after disabled user config.

This fixes the missing initial report, **not** Nushell's native percent encoder.
The next implementation must preserve custom rc definitions/hooks and launch
options, avoid duplicate or transiently incorrect native reports, and remove
the remaining ignore only when the percent test passes. A validated upstream
producer or Termeric-owned startup/reporting path are possible approaches;
emitting a late correction alone does not satisfy first-prompt correctness.
The remaining gap does not block unrelated implementation or deferred testing.

Normal interactive Bash, Zsh, and Fish launches emit a hostless OSC 7 report at
each prompt, with literal percent signs escaped before transmission. An
unambiguous Nushell REPL launch enables Nushell's native OSC 7 and OSC 133
settings after the user's normal configuration loads; its ordinary-directory
real-PTY probe passes, with the cwd limitations above. Nushell scripts, command strings, language-server/IDE
modes, and existing pre-REPL commands are preserved without injection. The
generated Elvish module emits the same hostless OSC 7 and OSC 133 lifecycle
after the user adds `use termeric-integration` to `~/.config/elvish/rc.elv`.
Termeric only exposes that module to rc-loading interactive Elvish launches;
scripts, command strings, daemon/language-server modes, and `-norc` launches
remain untouched when their final flag values select those modes. Isolated
Elvish 0.21.0 editor/PTY probes pass; user-configuration and native desktop
acceptance remain separate. See [Elvish launch handling](#elvish-launch-and-opt-in-module-handling). The
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
`d`, `e`, `w`, `c`, `a`, `s`, `o`, and `g` provide strict identifiers,
title/body/close/alive/buttons/icon selection, completion, plain or RFC 4648 base64
payloads, expiry policy, and close/activation-report requests. The `f`, `t`,
and `n` keys add base64-encoded
application identity, ordered notification types, and ordered icon-name
candidates; `u=0/1/2` is retained as typed low/normal/critical urgency. Plain
chunks are limited to 2,048 bytes;
encoded chunks are limited to 4,096 bytes and accept Kitty's encode-before-
chunking and encode-after-chunking forms, including an unpadded final chunk.
The singular `s` value is bounded base64 UTF-8 and accepts `system`, `silent`,
`error`, `warn`, `warning`, `info`, or `question`.
Assembled titles and bodies retain at most 4 KiB and 16 KiB respectively.
Button payloads retain at most 4 KiB before decoding into at most eight
non-empty labels of 256 UTF-8 bytes each, separated by U+2028.
`p=icon` is encoded-only and retains at most 192 KiB of decoded binary input;
plain icon data is rejected. Its optional `g` value is a strict 128-byte cache
identifier. Completed-but-undrained terminal events retain at most 384 KiB of
icon bytes; excess events keep their notification text and non-icon metadata
but discard the binary icon payload and its cache update instead of reusing a
stale cache value.

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
actions are rejected. Report and focus are independent, and focus defaults on
for completed OSC 99 requests as the protocol specifies. Legacy OSC 9/777
requests do not acquire this focus behavior. A
payload-free `p=alive` query is completed immediately; payload-bearing or
continued alive queries are rejected. Button labels reject empty entries and
C0/C1 controls.

The singular `o` value accepts exactly `always` (the default), `unfocused`, or
`invisible`. Termeric resolves the request's exact originating tab or pane and
filters before rate limiting, icon work, native delivery, or tracked lifecycle
state is created. `unfocused` presents unless that exact origin is the focused
terminal in the active, focused, non-occluded native window. `invisible`
additionally requires the origin to be hidden: an inactive tab, a pane hidden
by zoom, or an occluded/minimized window. A visible sibling pane is unfocused
but not invisible. Closed or stale origins fail closed rather than falling
back to whichever terminal is focused.

`OSC 99 ; i=id:p=? ; ST` receives an owner-routed response under the requesting
terminal's S7C1T/S8C1T framing. Linux responds with
`p=title,body,close,alive,buttons,icon:a=report,focus:c=1:o=always,unfocused,invisible:s=system,silent,error,warn,warning,info,question:u=0,1,2:w=1`; other targets respond with
`p=title,body:o=always,unfocused,invisible`. The platform split keeps lifecycle and urgency
discovery truthful.
Application/type/icon-name metadata has no capability-query key.

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
action-key/label pairs in the D-Bus `Notify` call. Either `a=report` or
`a=focus` registers the required `default` action. A matching `ActionInvoked`
is accepted once for that displayed generation. Report policy emits the exact
OSC 99 activation reply to the originating terminal, with an empty payload for
the default action or the one-based button number for a button. Focus policy
atomically validates and selects that same tab/pane before requesting native
window activation; stale origins never fall back to active content.

Notification keys retain the stable tab-local pane identity, not the event's
current tab-versus-pane route. Showing/replacing, explicit close, alive queries,
pending close reports, and delayed action/lifecycle callbacks all retain that
same owner through split/collapse, including a nonzero surviving pane. Removing
the owner never transfers its notification authority, focus request, or reply
bytes to the replacement survivor. Closed/closing owners are rejected before
native activation or protocol writes, and focus selection is revalidated before
requesting native activation. Report-only actions leave focus unchanged; replies
remain protocol-only, including in read-only panes or while broadcast is armed.

`a=-focus` leaves tab, pane, and native focus unchanged while preserving an
independent requested report. Unknown, stale, oversized, and duplicate actions
are ignored. Activation does not consume close-report state, so a following
native close still produces its requested report. When `c=1`
was requested, native user, service-policy,
explicit, and expiry closure emits exactly one `p=close` report to the original
tab or pane using the framing captured by that request. Replacement adopts only
the newest successful request's close policy; replacing a revision does not
report the old revision closed. Explicit/expiry closes wait up to five seconds
for native confirmation and then fail visibly and answer with `untracked`
instead of leaving the client waiting forever. `p=alive` returns a sorted,
comma-separated snapshot of named live IDs owned by that exact terminal;
anonymous and already-closing entries are excluded. Stale terminal origins are
dropped with no focused-terminal fallback. Freedesktop `ActivationToken`
strings are treated as opaque, bounded to 4 KiB, retained only by the matching
live notification generation, and never logged, exposed to the PTY, or reused.
Winit does not
currently expose applying a daemon-supplied token to an existing window, so
Termeric uses Winit's supported activation path: direct focus on X11 and a
user-attention request backed by `xdg_activation_v1` on Wayland. The compositor
has final authority, so this is a focus request rather than a focus guarantee.

Non-default sound policy also selects the tracked Linux path. `silent` becomes
the boolean freedesktop `suppress-sound` hint; error, warning, information, and
question become the corresponding `dialog-*` sound-theme name. `system` emits
no override and leaves policy to the notification service. These hints are
advisory at the native boundary, and no separate audio helper is launched.

Transmitted icon data also selects the tracked Linux path. The decoder accepts
only PNG, JPEG, and GIF, takes one static frame, and converts it to consistent
RGBA pixels. Input is rejected above 512 pixels in either dimension or 1 MiB
of decoded pixel data. The process-wide `g` namespace is shared by all Termeric
windows for the process lifetime and uses deterministic least-recently-used
eviction at either 64 entries or 16 MiB of retained pixel data. A valid fresh
icon atomically replaces its cache entry; invalid replacement data leaves the
previous valid entry usable. A `g`-only request reuses and refreshes an existing
entry, while a miss or decode failure preserves the notification and reports a
bounded warning.

Ordered local `n` icon candidates take priority over transmitted data. If none
resolves, the decoded pixels are sent directly in the standard freedesktop
`image-data` `(iiibiiay)` tuple with no temporary file. Client-supplied
path-like icon names are rejected; a safe application name remains only a
legacy fallback when no transmitted icon exists. Theme lookup and decoding
happen on the bounded worker, not the render/input path. Linux capability
replies therefore include `p=icon`; other targets continue to omit it.
Animated GIF timing is intentionally not retained or played.

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
authority, focus-only and combined activation, bounded generation-scoped
tokens, close signals, alive snapshots, timeouts, shutdown, exact inactive
tab/pane selection, stale-origin rejection, and exact delayed-reply routing
without a session bus. Real-compositor permission, presentation, focus grants,
and daemon action/signal behavior remain manual acceptance.

### Ordinary notification/audio helper lifetime

Ordinary notification and audio helper delivery shares one process-wide worker,
with a 16-request queue and at most eight retained direct children. With no
children, the worker blocks on the request channel without a periodic wakeup.
With live children, it checks for exits at a 50 ms receive timeout; incoming
requests wake it sooner. Each request gets its own receive/reap turn, including
while the queue remains busy. Completed children are reaped again immediately
before admission, so an exit during a blocked receive can free its slot.

Each launch result has one buffered acknowledgement. A descheduled caller or
discarded result receiver cannot hold the worker in a rendezvous send or skip
child cleanup. Spawn acknowledgement still is not proof of native delivery;
the caller waits for the spawn attempt, whose OS latency is not deadline-bound.
Capacity/spawn failures keep the existing visible, bounded, deduplicated errors;
later unsuccessful exits and wait failures are logged.

If all request senders disconnect, already queued requests are processed before
the worker waits for its remaining children instead of dropping their handles.
It does not kill a running desktop helper, so this worker-only teardown can
wait indefinitely for a hung child. The production sender is process-global:
normal process exit still relies on OS child/orphan handling, not this channel
disconnect path. There is no new process-exit wait or process-tree cleanup
claim. The separate hyperlink opener budget is unchanged.

Tests observe the real receive boundary across multiple former idle polling
intervals, show a live gated child switching back to untimed idle after reaping,
and verify individual queue turns, unread/disconnected acknowledgements, and
actual Unix child reaping at shutdown. Only temporary fixture processes run;
no sound player, desktop notification service, or provider is invoked. These
checks establish worker scheduling and child ownership, not a measured
whole-application CPU/power reduction or native delivery acceptance.

Raw-PTY layout-transition regressions cover lifecycle replies, report-only,
focus-only, and combined actions in S7/S8 mode. A test-only receiver captures the
real notification frontend's Show/Close/Alive commands across layout changes
without starting a worker or contacting a desktop service. Stable keys continue
through the existing in-memory worker generation, expiry, close, and action
tests. See [#121](https://github.com/Tjemmmic/termeric/issues/121).

[kitty-notifications]: https://sw.kovidgoyal.net/kitty/desktop-notifications/

## Kitty terminal file transfer

OSC 5113 is the only supported OSC path that can create terminal-host
filesystem entries. Termeric implements only consent-gated Kitty `send`
sessions on Linux. Approval is scoped to the exact originating shell or
attached live-domain pane; focus changes cannot redirect replies, and detached
domains fail closed. Payloads are streamed through bounded staging and
published without overwrite only at `finish`. Host-to-child reads, quiet
modes, bypass credentials, and rsync/deltas are unsupported. This authority is
independent of the similarly named Kitty graphics file-read setting. The exact
limits, link policy, lifecycle, and deferred native acceptance are documented
in [`kitty-file-transfer.md`](kitty-file-transfer.md).

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
