# Keyboard input protocol

Termeric defaults to legacy VT/xterm input and supports application-negotiated
xterm `modifyKeyboard`, `modifyCursorKeys`, `modifyFunctionKeys`, `modifyKeypadKeys`,
reserved `modifyStringKeys`, `modifyModifierKeys`, `modifySpecialKeys`,
`modifyOtherKeys`, and Kitty
progressive keyboard input. The encoder snapshots all mode families
from the focused terminal immediately before writing to that same terminal's
PTY, so tabs and panes cannot leak keyboard state into one another.

## Kitty progressive keyboard input

`CSI ? u` reports the flags active on the current screen. `CSI = flags ; mode u`
sets, adds, or removes flags; `CSI > flags u` pushes the current state and
activates a replacement; and `CSI < count u` pops it. The primary and alternate
screens have independent 16-entry stacks. A full stack retires its oldest
entry, an over-pop resets the current flags, and RIS resets both screens.

All five defined flags are accepted:

- Disambiguated Escape/Control/Alt/Super combinations use CSI-u rather than
  ambiguous legacy control bytes.
- Repeat and release events carry Kitty event types. Enter, Tab, and Backspace
  retain the protocol's recoverable legacy behavior unless all-key reporting
  is active.
- Alternate-key fields include shifted values and a PC-101 base-layout value
  when Winit's logical and physical identities provide them.
- All-key reporting covers text, common functional keys, F1-F35, keypad,
  media/volume, lock, and left/right modifier identities.
- Associated text is encoded as filtered Unicode code points for keyboard
  events and text-only IME commits.

With no negotiated flags, releases remain silent and every existing legacy
encoding path is preserved.

## Xterm modifyOtherKeys

Termeric implements xterm's resource-4 `modifyOtherKeys` profile and both
documented `formatOtherKeys` output formats. Applications
set levels 0 through 3 with `CSI > 4 ; level m`, may attach a four-bit modifier
mask as a subparameter, query with `CSI ? 4 m`, restore the disabled default by
omitting the level, and disable with `CSI > 4 n`. Query replies are
`CSI > 4 ; level m` and obey the requesting terminal's S7C1T/S8C1T framing.
Invalid levels, masks, plural shapes, and unsupported resource categories are
ignored atomically.

The levels have explicit boundaries:

- Level 0 retains the legacy encoder.
- Level 1 preserves familiar single Shift, Control, and Alt character
  mappings, while disambiguating Control+Shift, Super/Meta, modified Enter,
  and modified Tab. Shift+Tab retains its conventional `CSI Z` form.
- Level 2 encodes every modified ordinary character plus Space, Tab, Enter,
  Escape, and Backspace.
- Level 3 also encodes those keys without modifiers.

Applications select the output format with XTFMTKEYS resource 4:

- `CSI > 4 ; 0 f` selects the default function-key-27 form,
  `CSI 27 ; modifier ; Unicode-codepoint ~`.
- `CSI > 4 ; 1 f` selects `CSI Unicode-codepoint ; modifier u`.
- `CSI > 4 f` resets resource 4, while parameterless `CSI > f` resets every
  supported format resource (resources 0-7).
- `CSI ? 4 g` queries the current value and receives the restorable
  `CSI > 4 ; value f` response. `DCS $ q > 4 f ST` provides the equivalent
  DECRQSS status body.

XTFMTKEYS changes only keys selected by `modifyOtherKeys`. Shift, Alt, Control, and
Super/Meta occupy bits 1, 2, 4, and 8 before the required `+1` parameter
offset. A mask causes a matching sole modifier to retain legacy encoding.
Multi-scalar layout text is never truncated into a key code, and explicit
AltGr composition always sends the platform-produced UTF-8 once.
Xterm's two-parameter CSI-u form does not enable Kitty event types, alternate
key fields, associated text, or all-key semantics; those remain controlled by
Kitty's independent negotiation, which takes precedence when active.

This state belongs to one terminal across its primary and alternate screens,
follows focused and broadcast pane routing, and is included in live-domain
attachment snapshots. DECSTR and RIS restore the disabled default. When Kitty
flags are active, Kitty encoding takes precedence; clearing them exposes the
still-current xterm policy.

Invalid format values, plural/subparameter shapes, and unsupported resource
numbers are ignored atomically. Resource-zero `formatKeyboard` is independent
opaque compatibility state, described below.

## Xterm modifyKeyboard

Resource 0 implements the negotiation and lifecycle of xterm's
`modifyKeyboard` and `formatKeyboard` state. Applications set the values with
`CSI > 0 ; value m` and `CSI > 0 ; value f`, reset each resource by omitting
the value, and query them with `CSI ? 0 m` / `CSI ? 0 g` or the corresponding
DECRQSS requests. The complete 16-bit CSI value is retained and reported;
`CSI > 0 n` therefore produces xterm's unsigned representation of `-1`,
`65535`. Parameterless family resets, S7C1T/S8C1T framing, focused-pane
ownership, primary/alternate-screen persistence, live-domain attachment,
DECSTR, and RIS follow the other xterm keyboard resources.

The behavior boundary is important. `modifyKeyboard` is not an alternative
key encoding mode. The normal keyboard type ignores it. `DECSET 1060` selects
xterm's mutually exclusive legacy type and `DECSET 1061` selects its VT220
type; resetting either mode returns to normal. In the two historical types,
bit 1 permits numeric-keypad modifiers, bit 2 cursor/editing-keypad modifiers,
bit 4 function-key modifiers, and bit 8 miscellaneous function-key modifiers.
`CSI ? 1060 s` and `CSI ? 1061 s` save the current type into xterm's shared
keyboard-type slot; restoring either mode with `CSI ? Ps r` restores that
saved type. The `[keyboard] xterm_type` configuration value accepts `normal`,
`legacy`, or `vt220` and supplies the initial type for new tabs, panes, local
domain terminals, and RIS. Changing that setting live applies it immediately;
unrelated reloads preserve any application-selected runtime type.

The legacy type uses CSI 11-14 for F1-F4 and DEL for unmodified Delete. Both
historical types reserve Shift for VT220 UDK behavior and map Control+Fn to
F(n+10); the VT220 type additionally translates PC Home/End to DEC Find/Select
and the portable numeric keypad to its VT220 identities. DECSTR preserves the
type, RIS restores the configured initial type, and local-domain attachment replicates it. Xterm
stores and reports `formatKeyboard`, but does not consult it while encoding;
Termeric does the same.

## Xterm reserved string-key resource

Resource 5 retains xterm's reserved `modifyStringKeys` and `formatStringKeys`
negotiation state. It accepts modes `-1` (via `CSI > 5 n`) and 0 through 4,
formats 0 and 1, direct and DECRQSS queries, resource-specific and parameterless
family resets, S7C1T/S8C1T replies, screen persistence, DECSTR/RIS lifecycle,
and live-domain replication. Its mode default is 0 and its format default is 0.

Xterm does not define an encoder consumer for this reserved resource, so changing
it intentionally does not alter any key bytes. Termeric stores the state for
protocol compatibility instead of assigning speculative behavior to it.

## DEC user-defined keys

Termeric accepts `DECUDK` (`DCS Ps ; Ps | key/hex[;key/hex...] ST`) at DEC
conformance level 2 or later. The first parameter clears all definitions when
zero (the default) or preserves existing definitions when nonzero. Current
xterm accepts but does not enforce the second lock parameter and reports UDKs
as always unlocked; Termeric deliberately matches that behavior. Values are
strict hexadecimal byte strings. Definitions are limited to DEC codes 0-34,
4 KiB per key, and 64 KiB total per terminal.

In the VT220 profile, Shift invokes defined editing keys and F1-F20. Control is
first consumed to select F(n+10), as in xterm. UDK state is terminal-local,
survives DECSTR, clears on RIS, follows focused and broadcast pane routing, and
replicates across local-domain attachment. Negotiated Kitty keyboard reporting
takes precedence. Primary DA parameter 8 advertises the supported profile.

## Xterm modifyCursorKeys

Resource 1 implements xterm's `modifyCursorKeys` and `formatCursorKeys`
contract. `CSI > 1 ; value m` selects a mode, `CSI > 1 m` restores the default
mode 2, `CSI > 1 n` disables the feature, and `CSI ? 1 m` returns
`CSI > 1 ; value m`. The disabled state follows current xterm's unsigned query
representation (`65535`). Parameterless `CSI > m` resets every supported
modifier resource without changing either format resource.

The modes match xterm:

- Disabled strips modifiers and preserves the unmodified DECCKM/tilde form.
- Mode 0 retains the obsolete first-parameter form and may retain SS3 in
  application-cursor mode.
- Mode 1 forces CSI before a modifier parameter.
- Mode 2, the startup default, forces the modifier into the second parameter.
- Mode 3 adds xterm's private `>` introducer.
- Mode 4 emits every covered key, including unmodified keys, in the extended
  format selected by `formatCursorKeys`.

`CSI > 1 ; 0 f` selects function-key-27 output and `CSI > 1 ; 1 f` selects
CSI-u. `CSI > 1 f` resets the format, `CSI ? 1 g` queries it, and
`DCS $ q > 1 m ST` / `DCS $ q > 1 f ST` return restorable DECRQSS bodies.
Invalid values and shapes are ignored atomically; direct replies honor S7C1T
or S8C1T.

The portable Winit identities covered are Left, Right, Up, Down, Home, End,
Page Up, Page Down, Insert, and Delete. Extended mode uses current xterm's
keysym-to-Unicode values (private-use `U+E250` through `U+E263` as applicable,
with Delete encoded as 127) and includes Shift, Alt, Control, and Super/Meta in
the modifier parameter. Winit has no `Begin` named-key identity, so that member
of xterm's editing-key family is an explicit boundary.

Like resource 4, this state belongs to one terminal across both screens,
follows focused and broadcast routing, is replicated by live-domain attachment,
and resets on DECSTR/RIS. Kitty negotiation takes precedence without destroying
the xterm state.

## Xterm modifyFunctionKeys

Resource 2 implements xterm's `modifyFunctionKeys` and `formatFunctionKeys`
contract for Winit's complete F1-F35 range. Set, reset, disable, direct-query,
DECRQSS, S7/S8, pane/domain ownership, and DECSTR/RIS behavior mirror resource
1 using resource number 2. The startup mode is 2; disabled state queries as
`65535`. Both `CSI > 2 n` and xterm's parameterless shorthand `CSI > n`
disable this resource; explicit resource 0 applies to `modifyKeyboard` and is
not treated as the shorthand.

Modes 0-3 apply xterm's SS3/CSI first-parameter, second-parameter, and private
forms to the numbered function-key sequence. Disabled mode reproduces current
xterm's default `ctrlFKeys=10` behavior: Shift adds 10 to the function number,
Control adds 20, both add 30, and remaining Alt/Super state is discarded. The
result uses xterm's DEC-style mapping through F20 and sequential codes 42 and
above thereafter.

Mode 4 encodes every F key, including unmodified keys, in the format selected
by resource-2 XTFMTKEYS. Function-key-27 is the default; CSI-u is value 1.
Current xterm maps the contiguous X11 F1-F35 keysyms to private-use `U+E2BE`
through `U+E2E0`. Shift, Alt, Control, and Super/Meta remain independently
recoverable in the modifier parameter.

## Xterm modifyKeypadKeys

Resource 3 implements xterm's `modifyKeypadKeys` and `formatKeypadKeys`
contract. It uses the same set, reset, disable, query, DECRQSS, S7/S8,
pane/domain ownership, and DECSTR/RIS lifecycle as resources 1 and 2, but its
startup mode is 0 rather than 2. A disabled state is reported as `65535`.

Modes -1 and 0 through 3 preserve the conventional physical-keypad path.
DECKPNM emits the numeric character directly; DECKPAM emits SS3 plus the
application-keypad final. Modified DECKPAM keys carry xterm's modifier
parameter directly after SS3, for example Control+Alt+keypad 7 is
`ESC O7w`. The mode values remain independently settable and reportable even
though only mode 4 changes this non-parameterized keypad resource's encoding.

Mode 4 bypasses DECKPAM/DECKPNM and reports every covered keypad identity in
the format selected by resource-3 XTFMTKEYS. Function-key-27 is the default;
CSI-u is value 1. The 18 portable Winit keys are digits 0-9, Add, Comma,
Decimal, Divide, Enter, Equal, Multiply, and Subtract. Their X11 keysym mapping
uses private-use `U+E28D`, `U+E2AA` through `U+E2B9`, and `U+E2BD`. The 11
Num Lock-off Insert/navigation/Delete logical identities use `U+E295` through
`U+E29F`, so current lock state remains distinguishable without assuming an
initial lock state. Shift, Alt, Control, and Super/Meta remain in the modifier
parameter. Winit's extra
calculator and phone keypad codes do not correspond to xterm's numeric-keypad
keysym family and remain outside this claim.

## Xterm modifyModifierKeys

Resource 6 implements xterm's `modifyModifierKeys` and `formatModifierKeys`
contract with the same bounded set, reset, disable, query, DECRQSS, S7/S8,
pane/domain ownership, Kitty precedence, and DECSTR/RIS lifecycle as resources
1-4. Its startup mode is 0. Disabled mode and modes 0 through 3 leave modifier
key presses silent; mode 4 reports every covered modifier or lock press in the
selected function-key-27 or CSI-u format.

The portable identities are left/right Shift, Control, Alt, and Super, plus
Caps Lock, Num Lock, AltGraph (the platform's ISO Level 3 Shift identity), and
Winit's unsided Meta and Hyper. Mode 4 uses xterm's keysym-to-private-use
mapping (`U+E103`, `U+E27F`, and `U+E2E1` through `U+E2ED` as applicable) and
the event's Shift/Alt/Control/Super modifier parameter. Winit does not expose
Shift Lock, the wider ISO lock family, or Mode Switch separately from AltGraph;
it also cannot distinguish left/right Meta or Hyper, so those two use xterm's
left-hand identity. Releases stay silent outside Kitty event reporting.

## Xterm modifySpecialKeys

Resource 7 implements xterm's `modifySpecialKeys` and `formatSpecialKeys`
contract with the same bounded negotiation and lifecycle as resources 1-4 and
6. Its startup mode is 0. Disabled mode and modes 0 through 3 retain the normal
legacy path; mode 4 emits the key's xterm `keysym2ucs` value in function-key-27
or CSI-u format. Resource 7 takes precedence over `modifyOtherKeys` for keys
such as Backspace, Tab, Enter, and Escape, matching xterm's category ordering.

Canonical named coverage includes 72 Winit identities: core control keys,
Compose and Japanese/IME keys, and unambiguous XF86 brightness, volume, media,
browser, launch, document, and system keys. Shift+Tab retains its distinct
`ISO_Left_Tab` identity. On Linux, otherwise-unmapped `NativeKey::Xkb` values
are accepted only when they pass xterm's special-category test and one of its
bounded Unicode/private-use conversion ranges. Cursor, function, keypad,
modifier, Delete, ordinary, and unconvertible keysyms cannot leak into this
resource.

Winit collapses some different X keysyms to one named value. Termeric does not
guess among Print/SysReq, Control+Pause/Break, PowerDown/PowerOff,
Standby/Sleep/Suspend, Calculator/Calculater, Refresh/Reload,
Favorites/MySites, Kana Lock/Shift, or Eisu Shift/Toggle. If Winit leaves an
exact Linux XKB keysym unidentified, the raw path still preserves it.

## Mode-aware keys

- DECCKM changes unmodified arrows from CSI to SS3 and likewise changes Home
  and End. Negotiated modifyCursorKeys modes determine the modified form.
- DECKPAM changes physical numeric-keypad digits and operators to VT100/VT220
  SS3 application sequences. DECKPNM restores digits, operators, and carriage
  return. Physical `KeyCode` identity is authoritative even when Num Lock or a
  platform layout changes the logical key. Resource-3 mode 4 replaces both
  paths with an explicit recoverable keypad identity.
- Insert, Delete, Page Up, and Page Down use the xterm-256color sequences
  advertised by the current terminal identity. F1-F35 use xterm's numbered
  function-key mapping.

The ordinary legacy special-key encoder supports modifier parameters 2-8;
the default xterm cursor/function paths and application keypad additionally
preserve Super/Meta through parameters 9-16.

## Text, Control, Alt, and AltGr

Produced event text is sent as UTF-8, including non-ASCII and multi-scalar
layout output. Control uses the traditional C0 mappings for letters,
Space/@/2, `[`, `\\`, `]`, `^`, `_`, and Delete. Alt prefixes ordinary text and
basic control keys with Escape; special keys carry Alt in their xterm modifier
parameter.

AltGr is tracked explicitly from the platform's AltGraph key event. While it
is active, produced Unicode text is sent once, without an Escape prefix or
Control conversion. This preserves a distinct Ctrl+Alt path for intentional
Meta+Control input. Losing window focus clears the tracked AltGraph state.

## Dispatch precedence

Key presses are dispatched in this order:

1. Active modal or overlay input.
2. Active search input, then copy-mode/quick-select modal input.
3. Configurable application shortcuts.
4. Plugin and custom keybindings, including send-text actions.
5. Focused-terminal protocol encoding and PTY write.

Handled shortcuts therefore never also reach the shell. Unhandled repeat
presses follow the same path as initial presses. Releases are silent in legacy
mode and reach the PTY only when the focused application negotiated Kitty event
types for a key that the protocol makes releasable.

Copy-mode navigation is therefore never emitted to the child PTY. Its global
entry shortcuts use the same conflict-aware registry as every other application
action. See the [copy-mode contract](copy-mode.md) for modal bindings and
keyboard-only acceptance evidence.

## Evidence and boundaries

Table-driven tests cover every legacy named key and modifier combination,
DECCKM/DECKPAM, Control/Alt/AltGr and Unicode; exact modifyCursorKeys modes and
codes; exact F1-F35 modifyFunctionKeys modes, disabled remapping, and extended
codes; exact conventional and mode-4 modifyKeypadKeys behavior across all 18
portable keys and 11 Num Lock-off identities; exact modifyOtherKeys level,
mask, CSI-27/CSI-u format, Unicode,
Super/Meta, direct/DECRQSS query and reset,
exact resource-zero negotiation, exact modifyModifierKeys and modifySpecialKeys negotiation, category ordering,
canonical keysym tables, raw-XKB boundaries,
pane-focus, and domain-replication paths; plus Kitty stack lifecycle,
screen and pane isolation, exact CSI-u alternate/text fields, functional-key
event types, extended/keypad identities, Super modifiers, and text-only IME
commits. A real Bash/readline PTY accepts Home in normal and application cursor
modes. Existing Vim, less, and tmux traces exercise legacy application modes.

Native Kitty/xterm keyboard client interoperability and `vttest` remain manual
release checks. Winit's portable `ModifiersState` does not expose initial
Caps/Num-lock state or held Hyper/Meta state, so those modifier bits are not
claimed; their key identities are still reportable in all-key mode. Collapsed
XKB named-key aliases remain outside the claim. Platform
composition and keyboard-only chrome are documented in
[modern text input and accessibility](accessibility-and-input.md).
