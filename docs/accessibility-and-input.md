# Modern text input and accessibility

Termeric enables Winit's platform input-method path on every native window.
`Ime::Preedit` is local UI state: it is drawn beside the focused input cursor and
is never sent to a PTY. Only `Ime::Commit` is routed, once, to the current text
owner (terminal, search, settings filter, command palette, Unicode picker, workspace rename,
command form, or typed plugin-setting dialog). Changing pane focus, losing
window focus, or disabling the IME
cancels stale preedit. Candidate geometry is reported in physical pixels from
the same vertical-tab rail, padding, chrome/search offsets, pane rectangle,
cursor row/column, and
scaled cell dimensions used to render the terminal. Long preedit runs are
clipped only at grapheme boundaries to the space remaining in the focused pane
and never paint across a pane divider.

Command and Unicode palettes anchor native candidates and preedit to their own
visible query caret, not the terminal behind the modal. Their shared
`PaletteLayout` computes a horizontally scrolled, whole-grapheme query viewport;
rendering uses that same geometry and clips glyph overhangs to the field.
The staged domain picker uses the query viewport too, but still rejects IME
composition. All three palette inputs admit at most 256 non-control Unicode
scalars through typing, IME commits where accepted, or accessibility replacement.
Typing/commits stop at the limit; oversized semantic replacement is rejected
atomically. The complete value and byte-accurate cursor remain in the model and
semantic tree even when only the tail is visible. Narrow preedit space may show
no complete grapheme instead of painting outside its owner. Automated geometry,
limit, and routing tests cover these paths. The opt-in software-GPU smoke also
shapes mixed-script long queries and verifies every generated glyph vertex
stays inside the shared input clip; native IME and screen-reader
acceptance remain deferred.

Active terminal search now uses the same whole-grapheme viewport algorithm for
query painting, its caret, and native IME candidate/preedit placement. Its shared
search layout includes the vertical tab rail, chrome, and scaled cell metrics;
glyphs and preedit stay within the query field rather than overlapping controls.
Higher-priority text surfaces retain ownership, and a collapsed query field does
not fall back to the terminal cursor. The full bounded query and UTF-8 caret stay
in the model and semantic tree. Geometry tests cover narrow, shifted, scaled, and mixed-script fields;
a raw-PTY test verifies search composition/commit does not leak into the child.
The software-GPU smoke checks shaped search glyphs against their field clip too.
Native IME and visual acceptance remain deferred. Other text surfaces retain
their existing candidate placement behavior.

The configured Paste shortcut now inserts clipboard text at the caret of the
focused command/Unicode/domain palette, shortcut or Settings filter, or terminal-search query. Query handlers
recognize that binding before consuming its character key; rebinding and
disabling it use the normal shortcut registry. Pasted text is accepted atomically
only if it is single-line, contains no controls or Unicode line/paragraph
separators, and fits the existing limit (256 scalars for palettes and Settings, 128 scalars
for the shortcut filter, 4 KiB for search). Invalid or oversized pastes leave the query unchanged. Palette results
refresh once per paste; accepted pastes clear stale IME composition.
Higher-priority dialogs block background query pastes, and busy domain stages
remain read-only. Unsupported modal paste destinations do not fall back to the
terminal. Ordinary terminal pastes retain bracketed-paste framing, read-only
checks, broadcast policy, and guardrails. Local routing and raw-PTY tests cover
query insertion/refusal and child isolation; real clipboard/compositor
acceptance for these query destinations remains deferred.

Shortcut chord recording blocks clipboard reads and leaves the configured Paste
chord for the recording handler. Outside recording, the filter owns paste ahead
of retained Settings, command-form, or search fields; higher dialogs, palettes,
and a retained tab-rename owner keep their existing priority. A rejected filter
paste preserves the caret and IME preedit; an accepted paste resets filtered
results once and clears preedit. This does not modify or save shortcut bindings.

Settings paste is likewise confined to its filter, never to the selected setting
value. An open dropdown blocks clipboard reads and retains its normal key
handling. Settings owns paste ahead of retained workspace/command/search panels,
while higher-priority dialogs, palettes, shortcut editing, and tab rename retain
their existing ownership. Rejected text cannot fall through to a lower field or
PTY, and pasting does not apply or save configuration. Hermetic shortcut,
Unicode-limit, modal-routing, and raw-PTY checks cover this boundary; real desktop
clipboard acceptance remains deferred.

Search query editing uses unmodified Left/Right to cross whole graphemes,
Home/End to reach either end, and Backspace/Delete to remove whole graphemes.
This includes combining sequences, ZWJ emoji, and flags. If an insertion leaves
the byte caret inside a newly joined cluster, movement reaches its boundary and
deletion removes that entire cluster. Caret-only and no-op edits retain pending
scan progress, completed results, and the selected match; text changes restart
the search and recompile regex as needed. Accepted editing keys clear stale IME
composition. Ctrl+Left/Right moves to the preceding segment start or following
segment end using Unicode word boundaries, skipping adjacent whitespace;
non-word segments such as standalone punctuation and emoji are navigable too.
Ctrl+Backspace/Delete removes
that span, expanding deletion edges to whole graphemes even when the caret is
inside a newly joined cluster. The implementation scans only the bounded query
and retains no word index. These bindings apply to active terminal search and
the command, Unicode, and domain query palettes. Selection, Ctrl+Home/End, and
other modified editing keys remain consumed without editing; AltGraph never
becomes a Ctrl word-edit command. Local model and raw-PTY tests cover these
rules, but native keyboard/IME acceptance remains deferred.

Primary clicks in the search query now position the caret at the nearest visible
whole-grapheme boundary using the shared painted viewport, including rail/chrome
offsets, scaling, and horizontal scrolling. Clicking a confirmed query resumes
editing without clearing results, selection, filter/scope, or pending scan
progress. Query focus clears stale IME preedit; subsequent typing and commits
insert at the clicked caret. Inert search-bar space consumes primary presses
instead of starting selection in the terminal underneath. Buttons retain their
existing priority and actions. Geometry, model, and raw-PTY tests cover these
paths; native pointer/IME acceptance and drag selection remain deferred.

Unmodified Left/Right and Home/End now move the palette query caret; Delete
removes the following grapheme and Backspace removes the preceding one.
Combining sequences, flags, and ZWJ emoji are edited whole. Typing and accepted
IME commits insert at that caret. Cursor-only and no-op edits preserve result
selection and do not restart Unicode worker searches; actual query changes
retire old results as usual. Busy domain stages consume navigation/deletion
without changing their read-only filter or a palette behind them. The semantic
tree reports the moved UTF-8 caret. The palettes also support the same
Ctrl+Left/Right and Ctrl+Backspace/Delete word edits described above, with local
dispatch and blocked-worker regression coverage. Unsupported modifiers and
AltGraph are consumed without deleting text. A primary click in a palette query
now places the caret at the nearest visible whole-grapheme boundary. Hit testing
uses the same animated input rectangle, cell widths, and horizontally scrolled
text range as painting; blank padding selects the visible edge, not hidden text.
Caret placement preserves results and pending worker searches, and busy domain
stages reject it. Primary palette clicks clear stale IME preedit. Geometry and
model/worker regressions cover placement; the native pointer lane below covers
command/Unicode queries only. Native IME acceptance remains deferred.
Drag/Shift-selection and semantic SetTextSelection remain
unimplemented.

The private-Sway interaction lane now requires seventeen native palette
editing cases and exact Unicode insertion, alternating between two real local
PTY owners. It requires post-insertion fences, no modal-query leakage or
duplicate insertion, no unrelated focus loss, and restoration of the original
pane. Its `palette-input.json` sidecar records exact per-owner expected/actual
bytes. The schema-3 sidecar adds six Ctrl word-edit cases to the seven
unmodified-edit cases and requires command-palette Ctrl+Delete during setup.
Four further cases require pointer caret placement in command and Unicode
queries: middle insertion, wide/combining graphemes, and horizontal scrolling.
It complements the model/geometry tests; it does not test native IME
composition, visual clipping, Orca, or real-desktop policy. See the
[native GUI contract](gui-acceptance.md).

Current strict GUI publication independently reconstructs these seventeen cases
from the transcript and seals both files. The offline palette evidence checker
repeats that reconstruction and verifies both hashes, so missing/changed
artifacts or historical schema-1/schema-2 sidecars cannot stand in for the
current acceptance contract.

Native color fonts use the separate bounded RGBA glyph atlas described in
[Glyph atlas lifetime](glyph-atlas.md). Extended grapheme width remains the
terminal's Unicode width policy; color selection does not change cell width.

## Visual accessibility preferences

Settings > Accessibility and the persisted `[accessibility]` table expose two
independent three-state policies:

```toml
[accessibility]
reduce_motion = "system" # system, on, or off
high_contrast = "system" # system, on, or off
```

`reduce_motion = "on"` makes application-owned panels, dialogs, tab
open/close, hover, cursor-position easing, focus dimming, copy flash, toast,
and Harvest chrome snap directly to their semantic state. It also removes the
continuous pulse/fade from Harvest growth indicators. It deliberately does
not suppress terminal-application animation, text blink, the configured
terminal cursor blink, or the visual bell: those are terminal content or
explicit status signals rather than decorative Termeric transitions.

`high_contrast = "on"` derives opaque application-chrome surfaces and chooses
their text colors to meet a 4.5:1 contrast floor across every built-in theme.
It covers tabs, Settings, search, palettes/dialogs, keycaps, borders, and modal
scrims. It does not rewrite the terminal foreground, background, ANSI palette,
truecolor output, inline media, or colors changed by an application through
terminal protocols.

`"off"` is an explicit override. `"system"`, the default, follows a supported
desktop signal and otherwise resolves to off. Legacy `true` and `false` config
values remain valid and are interpreted as explicit `"on"` and `"off"`
overrides, respectively. Saving Settings writes the canonical string form.

Both preferences apply immediately through Settings, config reload, startup,
later theme changes, and live native preference updates where supported:

| Platform | Native preference source | Current behavior |
| --- | --- | --- |
| Linux | XDG Desktop Portal `org.freedesktop.appearance` `reduced-motion` and `contrast` | Read and monitored on one background D-Bus thread. Unsupported portals or individual keys fall back independently without blocking startup. |
| macOS | NSWorkspace / accessibility display preferences | Not implemented; `"system"` resolves to off. |
| Windows | UI Settings / high-contrast notifications | Not implemented; `"system"` resolves to off. |

The Linux mapping follows the official
[XDG Desktop Portal Settings specification](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Settings.html).
Native visual and assistive-technology acceptance remains separate from these
hermetic policy and routing tests.

## Keyboard-only chrome

Press `F6` to enter the tab bar. `Shift+F6` enters from the opposite end. While
the focus ring is active:

- `Tab` / `Shift+Tab` or arrow keys move through each tab, each close button,
  overflow navigation button, New Tab, Commands, Workspaces, and Settings;
- `Home` and `End` move to the first and last control;
- `Enter` or `Space` activates the focused control;
- `Escape` returns focus to the terminal.

A normal left click also exits F6 chrome navigation after its target has been
dispatched, so typing follows the clicked tab or pane without an extra Escape.
The keyboard-anchored tab viewport is preserved until hit testing completes;
clearing it earlier could select a different tab than the one displayed.
Modal interception still takes priority. Releases and non-primary buttons do
not perform this handoff, and an already-terminal focus leaves text-field
accessibility focus and IME preedit state alone. The native overflow fixture
checks terminal, distant-tab, and split-pane input ownership after F6 clicks;
this does not establish screen-reader or platform IME acceptance.

The focus outline uses the exact rendered bounds shared by tab rendering and
hit testing, including the animated bounds when reduced motion is off. It is
theme-derived, two physical pixels wide, and does not rely
on color alone: its persistent outline shape is distinct from hover and active
tab fills.

When 80-pixel minimum tab widths no longer fit, the bar renders only a bounded
contiguous range between previous/next controls. The active tab is always in
that range; keyboard focus temporarily anchors the same range so its outline
never lands offscreen. Hidden tabs remain represented to assistive technology,
and activating one reveals it. The overflow controls have named AccessKit
buttons and share their exact bounds with pointer hit-testing.

## Screen-reader model

AccessKit is initialized before a window becomes visible and bridges to AT-SPI
on Linux, NSAccessibility on macOS, and UI Automation on Windows. The tree
contains the application window, selected tab, named tab-close and global
controls, every active-tab pane, and only the currently visible terminal rows.
The focused pane publishes cursor or selection positions against text-run
nodes. Updates are incremental after the initial full tree, and terminal
content is a polite live region.

When the command palette is open, it becomes the only child of the application
window in the semantic tree. AccessKit exposes a modal dialog containing the
bounded UTF-8 search input, its cursor, the visible result list, the selected result as
the input's active descendant, category/destructive descriptions, and keyboard
shortcuts. Result node identities are allocated from stable command IDs rather
than transient filtered rows, and mappings are retired when commands leave the
bounded registry. A screen-reader click executes only the command that was
announced and only while it remains visibly exposed; stale actions
cannot fall through to terminal or tab controls behind the modal. Semantic row
bounds come from the same `PaletteLayout` used by rendering and mouse hit tests.

The Unicode character picker reuses the same bounded visual geometry but owns
an isolated command registry, modal semantic tree, and stable node namespace.
Its editable search exposes the exact UTF-8 value and cursor, the selected
visible character is the active descendant, and every option announces its
character/name plus category, aliases, and code point. Dynamic code-point
results and Unicode 17.0 canonical-name searches retire their previous semantic
identities immediately. A query materializes no more than 256 ranked options,
so complete name discovery cannot expand the accessibility tree to the full
Unicode database. Focus and click
actions must resolve to a currently visible stable ID, so filtered or stale
screen-reader actions cannot insert text or fall through to background
controls. Keyboard, pointer, wheel, IME commit, AccessKit replacement, and
reduced-motion behavior all operate on the same picker state. Successful
activation additionally revalidates the exact writable tab/pane captured when
the picker opened.

The staged live-domain picker reuses that exact palette geometry but owns a
separate isolated modal tree and stable action namespace. Saved-host identities
come from configured profile names; live-domain identities come from validated
domain IDs rather than result ordering. Duplicate domain IDs are collapsed, and
every semantic action must still be both visible and actionable in the current
stage before it can start discovery or preparation. Loading, empty, preparing,
and error entries are polite status nodes rather than executable options.
Loading/preparation makes the filter read-only, and both palette workflows size
keyboard selection and scrolling to the rows the current window can render.
Headless routing tests stop before client construction and never open a local
socket or SSH process.

The settings panel follows the same modal isolation rule. Its semantic tree
contains a UTF-8-aware filter input, section headings, numeric spin buttons
with bounded increment/decrement actions, expandable combo boxes and their
currently rendered options, and read-only labels. Stable identities use the
setting's section and label rather than its filtered row number; dropdown
options are resolved by their announced value so a stale request cannot choose
a different item after a dynamic option list changes. Selection, scrolling,
row bounds, and dropdown bounds come from `SettingsModalLayout`, which is also
used by rendering and pointer hit tests. Every mutation returns the same
`SettingsAction` used by keyboard and mouse input and is applied by the normal
configuration-validation path. Requests for background controls are swallowed
while the modal is active.

The filter advertises and handles `SetValue` through the same single-line,
256-scalar/1,024-byte policy used by keyboard, IME, and paste insertion. Accepted
replacement (including empty text to clear) moves the caret to the end, resets
results, clears stale preedit, and returns semantic focus to the filter without
applying or saving configuration. Rejected payloads leave the query state
unchanged. While a dropdown is open, the filter is read-only, advertises neither
replacement nor focus actions, and rejects retained requests; semantic focus
falls back to the selected setting rather than the non-editable filter. Higher
modals and closed Settings also reject stale replacement requests. Model and
in-process AccessKit action/tree tests cover this contract; native screen-reader
acceptance remains deferred.

Font → Range Overrides is exposed as a named button with its active count. It
opens the same accessible information/input overlay used by other bounded
management flows: rules and navigation are named choices, add/edit uses one
editable `U+START-U+END | FAMILY` field with UTF-8 cursor semantics, and delete
requires a named confirmation. Request IDs and full rule identities prevent a
stale semantic action from retargeting a different rule after config reload;
cancel, dismissal, and overlay replacement revoke the pending action context.
The manager's named **Inspect Glyph Coverage** choice opens a bounded `Text`
input. Its read-only report exposes named Retry and Back buttons and announces
visible truncation plus the distinction between configured-family and platform
shaping evidence. The shared overlay describes inputs generically rather than
mislabeling non-plugin forms as plugin settings.

Font → OpenType Features is likewise a named button with its active count. Its
bounded overlay exposes canonical feature entries, paging, Add, Edit, Delete,
confirmation, and Back as named actions. The `Feature` text input accepts one
`tag = value` pair. Request IDs plus the complete prior tag/value identity keep
keyboard, pointer, and accessibility mutation on the same stale-safe path.

The custom-command panel is likewise isolated as a modal dialog. It exposes
named New and Close buttons, each command as separate Run and Delete actions,
an empty-state announcement, and the inline editor's Name and Shell Command
text inputs plus Save and Cancel buttons. Command identities use the complete
name/command/alias tuple and duplicate occurrence rather than a transient row
index. Before an action is applied, that identity must still resolve to the
same current command; removed or changed entries cannot retarget a stale Run or
Delete request. Form value replacement is UTF-8 aware, single-line, and
bounded. Rendering, pointer hit testing, and AccessKit bounds all use the same
`CommandsPanelLayout`, and pointer, keyboard, and accessibility actions share
one application path. Automated accessibility tests use inert form strings and
never submit or execute a custom command.

The custom-command Name and Command fields support whole-grapheme Left/Right,
Home/End, Backspace/Delete, and Ctrl+Left/Right/Backspace/Delete word edits.
Typing and IME commits insert at the active caret; unsupported modified editing
keys are consumed, and Ctrl/Alt/Super shortcuts do not insert their literal
letters. AltGraph text remains available. Both fields use clipped, shaped text
and a caret-following whole-grapheme viewport; inactive fields show their start.
IME candidates/preedit share the active field's animated/scaled geometry, and
field changes retire stale composition. Existing byte limits and submission
validation are unchanged. Model, geometry, modifier, and raw-PTY isolation tests
pass without saving or executing commands. Native visual/IME acceptance remains
deferred. Primary clicks place the caret at the nearest visible whole-grapheme
boundary using that same animated/scaled viewport. Padding selects a visible
edge; a scrolled field does not jump to hidden text. Clicking an inactive field
uses its displayed unscrolled text before changing focus. Collapsed fields
consume clicks without moving a hidden caret, and invalid byte/interior-cluster
positions cannot change form focus. Geometry/model and raw-PTY tests cover
placement followed by IME insertion; native pointer acceptance remains deferred.
Selection remains unimplemented.

The configured Paste shortcut (Ctrl+Shift+V by default) inserts clipboard text
at the active form caret without saving or executing it. Name retains its
256-byte ceiling and Command its 16 KiB ceiling; the search query's smaller
4 KiB limit does not restrict command forms. Empty, multiline, control-containing
(including tabs and escape sequences), Unicode line/paragraph separator, and
oversized pastes are rejected atomically without moving the caret or clearing
pending composition. Successful insertion uses one UTF-8 splice and clears
stale preedit. Higher-priority modal owners and the panel's browse mode keep
pastes blocked; rejection never falls through to the terminal or a retained
search. Hermetic shortcut, UTF-8 boundary, field-limit, ownership, and raw-PTY
tests cover this path without accessing a real clipboard or submitting a command.
Real-desktop clipboard/IME acceptance remains separate.

The workspace manager and legacy-snapshot manager share a bounded modal model.
Each visible entry is a named group with exact Load and Delete actions;
workspaces additionally expose Rename. Stable actions resolve the saved entry's
ID and manager mode again before use, so a stale request cannot load, rename,
or delete a different entry. The rename form has a bounded UTF-8 text input and
visible Save/Cancel controls. `SessionPanelLayout` owns the panel, header,
rename, viewport, row, and action-button geometry across rendering, pointer hit
testing, hover, and AccessKit. Long lists scroll, keyboard selection stays
visible, and Up/Down, Enter, Delete, F2, and Escape provide complete keyboard
operation. Accessibility tests inject in-memory entries and never load, rename,
or delete a persisted workspace or snapshot.

The Settings filter accepts complete native character events and IME commits
atomically, capped at 256 Unicode scalars (at most 1,024 UTF-8 bytes). Controls,
line/paragraph separators, and over-limit events are rejected without changing
the filter, caret, result selection, or scrolling. Plain/Shift text, named Space,
and AltGraph text retain every scalar, including combining and ZWJ sequences;
other modified text is consumed, except explicit Ctrl+S/Ctrl+R Save/Reset actions.
Plain Backspace/Delete remove a whole grapheme. Home/End move the filter caret,
Ctrl+Left/Right move across Unicode word boundaries, and Ctrl+Backspace/Delete
remove the adjacent word. Plain Left/Right retain their setting-value and
dropdown behavior; unsupported modified editing keys are consumed without
changing a setting. Caret-only moves preserve result selection and scroll state.
Hidden panels and open dropdowns refuse
filter edits, so typing cannot silently change the list behind a dropdown.
Native typing clears stale IME preedit only on a successful edit. Model,
headless-dispatch, and raw-PTY tests cover limits, modifier ownership, and child
isolation without saving configuration. This does not add Settings-filter
selection or reassign plain Left/Right to character navigation; native IME and visual
acceptance remain deferred.

Settings-filter rendering now uses a caret-following whole-grapheme viewport,
shaped text, explicit glyph clipping, and a clipped placeholder. Rendering,
AccessKit input bounds, and native IME candidate/preedit placement share
`SettingsModalLayout` input geometry across scale and animation. Computing the
candidate position does not rebuild the settings inventory. A collapsed field
has no candidate position, and an open dropdown exposes neither an editable
filter caret nor IME preedit. Geometry and ownership tests plus the opt-in
software-GPU glyph-clipping smoke cover this path; real desktop presentation
and IME-client acceptance remain deferred.

Primary clicks in the Settings filter now place the caret at the nearest visible
whole-grapheme boundary using that same scaled/animated viewport. Padding maps
to the visible slice edges, not hidden text. The resulting filter-only action
revalidates active/dropdown state and the exact grapheme boundary before moving
the caret or clearing IME preedit; it never applies configuration. Result
selection and scroll position are preserved. A click that dismisses an open
dropdown does not also move the filter caret, and collapsed inputs do nothing.
Geometry, model/action, paste/IME, and raw-PTY regressions cover this path;
native pointer acceptance and drag selection remain deferred/separate.

The keyboard-shortcut editor is a bounded modal tree with a UTF-8-aware filter,
category headings, and one stable action button per visible binding. Action
identity comes from `BindableAction`, never a filtered row number. Activating a
binding enters the existing timed recording path; a named custom action clears
it, conflict reassignment is announced through a polite live status, and stale
actions are rejected after filtering or scrolling hides their target. Pointer
row activation, keyboard selection, rendering, and AccessKit bounds all use
`ShortcutsPanelLayout`. Category headings count toward the viewport and
scrollbar, preventing the previous selectable-row/rendered-row drift. Tests
change only an in-memory registry and never persist user bindings.

Shortcut-filter typing, complete native character events, IME commits, and
semantic replacement now share a 128-Unicode-scalar ceiling (at most 512 UTF-8
bytes), rejecting controls and Unicode line/paragraph separators. Keyboard and
IME insertions are atomic: an event that would exceed the remaining allowance
does not insert a partial prefix or change the current result selection.
Complete combining/ZWJ sequences in a native character event are retained rather
than reduced to their first scalar; Backspace removes a whole grapheme. Plain
and Shift text, Space, and AltGraph text are accepted, while other modified text
is consumed without insertion. Chord recording still owns its input and rejects
filter mutations. Model, dispatch, and raw-PTY isolation tests pass without
saving user bindings; native IME/visual acceptance remains deferred.

The filter supports whole-grapheme Left/Right, Home/End, and Backspace, plus
Ctrl+Left/Right word navigation and Ctrl+Backspace word deletion. Caret-only
movement preserves the selected result and scroll position; text deletion resets
them. Changed caret/text clears stale IME preedit and requests a redraw, so
subsequent keyboard or IME text inserts at the displayed position. Unsupported
modified editing keys are consumed without mutation or PTY delivery. Plain
Delete still clears the selected binding, not filter text; modified Delete does
neither. Recording retains exclusive key ownership. Filter selection is not
implemented.

Primary clicks in the shortcut filter place its caret at the nearest visible
whole-grapheme boundary, using the renderer/IME viewport at the current scale
and animation position. Left/right padding maps to the visible slice edges,
not hidden text. Caret placement preserves result selection and scroll; typed
and IME text then inserts at that position, with no PTY fallback. Recording and
collapsed input rectangles consume clicks without moving the caret or activating
a result. Shared-geometry, dispatch, and raw-PTY regressions cover this path;
native pointer/visual acceptance remains deferred.

The shortcut filter now renders a caret-following whole-grapheme viewport with
shaped, explicitly clipped text and a clipped placeholder. Its visible caret
and native IME candidate/preedit position share `ShortcutsPanelLayout`, including
animation and scaled cells. Candidate geometry does not rebuild the result list;
collapsed inputs have no candidate position and recording shows no editable
filter caret. Recording also blocks preedit when a Settings panel is retained
underneath. Geometry/priority regressions and the opt-in GPU-backed query glyph
clipping smoke cover this boundary; native IME and pixel-level presentation
acceptance remain separate.

Application information, errors, and plugin notices share a request-scoped
information dialog. Titles are capped at 256 characters; stored notice content
is capped at 512 lines and 4,096 characters per line with visible omission
markers. The card reserves a window-bounded viewport so Copy and Continue never
move off-screen. Up/Down/Page keys and the wheel scroll lines, Left/Right and
Shift+wheel scroll long lines, and Home/End variants move to horizontal or
vertical boundaries. Rendering, pointer rows/buttons, viewport sizing, and
AccessKit all use `InfoOverlayLayout`.

Tab and Shift+Tab cycle through the optional input, every declared choice,
Copy, and Continue/Save; Enter activates only the focused control. Number
shortcuts remain available, but are no longer required to reach later choices.
Focus has an accent outline, scrolls choice rows into view, survives viewport
resize, and uses the same owner in AccessKit. Manual vertical scrolling leaves
choice focus so Enter cannot activate an offscreen choice. IME commits and
preedit are accepted only while the input itself is focused.

New non-input dialogs focus Continue, never the first consent/destructive
choice. Escape retains dismissal/cancellation behavior, repeated Enter key
events are ignored, and replacement/dismissal revokes the old focus identity.
Keyboard traversal does not itself accept consent or change application data.
Native visual and screen-reader acceptance remain deferred; automated tests
exercise the state, request identity, geometry, and action routing locally.

The semantic tree contains only the modal dialog, heading, visible document
slice, polite position status, explicit choices, Copy, and default action
button. An optional single-line text input is used by typed plugin settings; it
publishes its value, UTF-8 cursor, byte limit, exact visible-row bounds, and
SetValue action, while the default action becomes Save. Keyboard editing,
pointer cursor placement, IME candidates/preedit, rendering, and AccessKit use
the same bounded input and geometry. Offscreen text and choices are retired
until scrolled into view. Every
action includes the notice generation; dismissing revokes actions immediately
while the closing animation continues, and a replaced profile picker cannot
apply a hidden stale number, pointer, or screen-reader choice. Profile choices
are named buttons with the same row bounds and number shortcuts shown on
screen. Tests use inert strings and fake choice identities and do not invoke a
plugin executable or provider.

The destructive-command guardrail is an isolated assertive alert dialog for a
critical match and a polite alert dialog for a warning. It exposes the bounded
exact command, reason, target/user context, and separately named Cancel and Run
anyway actions. Cancel receives initial semantic focus; `Enter` and `Escape`
cancel, while keyboard confirmation deliberately requires `Ctrl+Enter`.
Rendering, pointer hit testing, and AccessKit bounds share `GuardrailLayout`.
Every action carries a monotonic request identity, dismissal discards the
buffered bytes immediately even while the exit animation still owns input, and
a stale action cannot confirm a later request.

A guarded request captures the exact tab or pane destination—or the complete
pane-broadcast destination/encoding set—at interception time. Confirmation
validates that every captured destination still exists before delivering any
member and never falls back to current focus. Typed commits, clipboard paste,
and pane broadcast use this path. Hermetic tests use a fake plugin identity and
an inert local process; they neither start an AI provider nor confirm the
guarded payload. This remains a best-effort product warning for plugin tabs,
not a shell parser or security boundary: the finite substring rules can produce
false positives and false negatives and cannot prove what a CLI or downstream
agent will execute.

Scrollback and inactive-tab terminal buffers are intentionally not exposed.
SGR hidden cells are replaced by blank semantic cells, so invisible/password-
style content cannot leak through the accessibility tree. A selection that
intersects the currently visible scrollback viewport is mapped into those
visible semantic rows; offscreen selection endpoints are clipped to the
viewport without exposing additional history. Terminal content is read-only to
assistive technology; PTY input continues through the normal focused input
route.

Diff review is an isolated modal document with visible-row-only semantic text,
polite row/column position status, two-axis keyboard, wheel, and semantic
scrolling, and explicit Copy and Done controls. Rendering, pointer hit testing,
viewport clamping, and AccessKit bounds share `DiffReviewLayout`. Review
actions carry a monotonic request identity; closing revokes the request before
the exit animation finishes, and a replaced dialog rejects retired nodes. Done
marks only the detector-local diff identity on the original stable tab, never
all diffs or whichever tab is currently focused. The review view is bounded to
4,096 rows and 4,096 characters per row; its omission marker is visual only,
and Copy resolves the complete raw diff from its exact source only when used.

The context assembler is also an isolated request-scoped modal. Source rows are
typed checkboxes with stable identities; the Specific Files row opens an
AccessKit text input accepting comma-separated workspace-relative paths.
Rendering, pointer hit testing, wheel viewport, and semantic bounds share
`ContextPickerLayout`. Gathering runs off the UI thread with three-second
command deadlines, bounded command output, 10,000-character source limits, a
64,000-character aggregate limit, and at most 16 workspace-contained regular
files. Escape cancels selection, file editing, or an in-flight request as
appropriate. Late worker results are generation-checked, and successful
insertion writes only to the exact stable tab-and-pane identity captured when
the picker opened. The requested plugin is resolved from live per-pane launch
identity: a focused shell sibling is not a plugin destination. A matching focused
plugin pane is preferred, otherwise the first matching live pane in visual order
within the first matching tab is selected, without changing focus. Gathering
prefers that owner's reported/process working directory; the existing fallback
applies only when it is unavailable. Completion rechecks the same owner's plugin
identity and writability after split/collapse; closed, replaced, changed-plugin
and read-only destinations fail visibly without focus or plugin-name fallback.
Cross-tab/window pending-target migration is not implied. Raw-capture regression
evidence does not replace native/provider acceptance
([#119](https://github.com/Tjemmmic/termeric/issues/119)).

Terminal search is a non-modal AccessKit toolbar above the focused terminal.
Its 4 KiB UTF-8 query is editable only while search owns keyboard input; result
progress/counts are polite live status, regex failures are assertive, and case,
regex, filter, previous, next, and close controls expose their current state.
Rendering, pointer hit-testing, and semantic bounds all use `SearchBarLayout`.
Insertion, IME commit, accessibility replacement, and Backspace share the same
cursor-aware query model, including when the cursor is in the middle of a
multi-byte query.

Transient toasts are non-interactive status nodes with stable per-toast
identities and a 512-character message ceiling. Information, success, and
warning notices are polite; errors are assertive. `toast_layout` supplies both
the painted card and AccessKit bounds, and toast nodes are appended to the root
even when an isolated modal tree owns the rest of the application.

Harvest's Root Cellar is an isolated modal dialog containing a bounded command
history list, actual normalized command text, exit/health metadata, an explicit
close button, and stable entry identities. It opens with the newest command
selected; keyboard, pointer, wheel, and AccessKit scrolling share one viewport
and selection model. Its status announces retained and evicted entry counts,
and entries with 64-KiB-clipped output disclose that limit. Rendering, hit
testing, and semantic bounds all use `CellarLayout`. Activating an exact entry
closes the cellar and opens its sprout action menu.

The sprout menu is an isolated AccessKit menu with stable identities scoped to
the exact visible request. Focus and selection track one another, the shared
`SproutMenuLayout` keeps rendering and hit targets inside the window, and stale
or background actions are consumed without execution. Actions that require
evicted live scrollback or a closed source terminal are disabled, omitted from
keyboard navigation, visibly dimmed, and carry a semantic reason; retained
primary rows temporarily hidden by an alternate-screen application carry a
separate reason. Retained 64-KiB snapshots are labeled as truncated before
Copy or Explain. The Explain item states
that it sends the selected command/output to an unambiguous open AI plugin tab;
automated accessibility tests inspect that contract but deliberately never
invoke a provider-backed explanation.

Automated semantics now cover every current application surface. Issue #32
remains open until native Orca, VoiceOver, or Narrator acceptance demonstrates
that the platform bridge, focus, announcements, editing, and updates are usable
end to end.

## Platform acceptance

Record Linux results with the candidate-bound, artifact-hashed
[native acceptance record](native-linux-acceptance.md). Its exact IBus, Fcitx 5,
and Orca cases turn the following procedure into validated evidence while still
keeping real input and screen-reader judgment manual.

IME tests should use both single-terminal and split-pane layouts and confirm
that preedit never appears in the PTY transcript before commit, then that CJK
text and a dead-key composition arrive exactly once. Also move focus during a
composition and verify it is cancelled rather than committed to another pane.

- Linux: test IBus and Fcitx 5 under both X11 and Wayland; test Orca with the
  desktop's AT-SPI bus enabled.
- macOS: test the built-in Japanese or Pinyin input source and dead keys; test
  VoiceOver navigation and activation.
- Windows: test Microsoft Pinyin/Japanese IME and dead keys; test Narrator's
  scan mode and UI Automation actions.

For each screen reader, verify the window name, selected tab, focused pane,
visible text, cursor, selection, live output, every named tab-bar control,
command-palette search and activation, settings filtering and adjustment, and
activation of tab/pane/control actions. No test should announce scrollback,
inactive terminal buffers, background controls behind a modal, or SGR-hidden
text.

Color acceptance uses an installed native color font (Noto Color Emoji on
Linux, Apple Color Emoji on macOS, Segoe UI Emoji on Windows), representative
single-scalar and ZWJ emoji, screenshot review, and repeated atlas-pressure
output. Bidirectional reordering remains outside this feature and is documented
as unsupported.
