# Termeric feature support

This is the authoritative statement of what Termeric currently supports. The
README and release notes summarize this file; when they disagree, this file
wins. Its initial publication is tracked by
[#24](https://github.com/Tjemmmic/termeric/issues/24).

The last immutable baseline was audited on 2026-08-25 at commit `8c127fc`.
Working-tree capability claims were reconciled on 2026-09-02 and must be pinned
to a new candidate commit before release. A parser branch, unit test, or
configuration field does not by itself make a feature supported: the complete
PTY-to-application-to-render/input path must work.

## Status vocabulary

| Status | Meaning |
|---|---|
| **Supported** | Implemented end to end on the named platform and covered by repeatable evidence. |
| **Partial** | A documented subset works, but important cases or integration paths remain incomplete. |
| **Experimental** | Usable only as an opt-in preview; behavior and persistence may change. |
| **Broken** | Exposed, parsed, configured, or advertised, but a normal supported use case is known to behave incorrectly. |
| **Unverified** | The implementation may exist, but current release evidence is insufficient. |
| **Unsupported** | Not implemented. |

## Platform and runtime

| Capability | Status | Current boundary | Tracking |
|---|---|---|---|
| Linux Wayland | **Partial** | A private Weston lane requires native window creation, first WGPU render, PTY output, and orderly last-PTY close. Clean local private-Sway evidence at `f9ccc2d` covers keyboard-only copy, compositor clipboard/bracketed paste, exact split-pane routing/closure, native close, two-window fullscreen PTY resize/isolation, sequential lost/outdated-surface/device-loss recovery, and a separate displayed-process soak. Clean `bd70faa` additionally passes a validated ten-minute AMD/radeonsi OpenGL soak with required process-attributed DRM memory in all 877 samples. A separate clean `946eb65` release-binary run passes the same ten-minute lane with 954/954 required DRM samples and complete child cleanup; its explicit Noto font profile is not a visual-baseline comparison. Clean `6ab38bf` additionally refreshes release-binary private-Sway interaction and two-window fullscreen with all three recovery injections, independently reconstructed 17-case palette and 11-case Settings evidence, and explicit skipped visual comparisons. Real-compositor input, scale, visual quality, desktop policy, broader adapter/backend and longer-duration coverage, and accepted CI history remain open and have exact candidate-bound evidence schemas. | [GUI acceptance](gui-acceptance.md), [native evidence](native-linux-acceptance.md), [platform tiers](platform-support.md) |
| Linux X11 | **Partial** | Recorded Xvfb/twm automation covers primary input, resize, tabs, panes, search, selection, clipboard, links, modal hit targets, Vim/less/tmux, surface-loss recovery, and normal close. The required Openbox lane adds outdated-surface/device recovery and two-window fullscreen/PTY-resize isolation, but that revised lane awaits its first green artifact. Real-compositor visual and desktop-policy checks remain open and now have an exact candidate-bound evidence schema. | [GUI acceptance](gui-acceptance.md), [native evidence](native-linux-acceptance.md), [platform tiers](platform-support.md) |
| macOS | **Unverified** | Build/unit CI plus a shell-independent native PTY output/input/resize/exit/cleanup smoke are configured, but have not yet supplied accepted green evidence; native window, keyboard/IME, clipboard, GPU, install, and soak acceptance remain absent. | [platform tiers](platform-support.md), [#23](https://github.com/Tjemmmic/termeric/issues/23) |
| Windows | **Unverified** | Build/unit CI plus a shell-independent native ConPTY output/input/resize/exit/cleanup smoke are configured, but have not yet supplied accepted green evidence; native window, keyboard/IME, clipboard, GPU, install, and soak acceptance remain absent. | [platform tiers](platform-support.md), [#23](https://github.com/Tjemmmic/termeric/issues/23) |
| GPU/software rendering | **Supported** | Hardware selection automatically falls back through WGPU software; CLI/config can force either path, diagnostics identify the adapter/backend, and bounded generation-safe surface/device recovery preserves PTYs. Lost surfaces and outdated swap chains share a resize-or-single-recreation path rather than relying on another incidental redraw. Clean local `f9ccc2d` evidence passes the llvmpipe visual/performance subset and private-Sway recovery on an AMD Radeon RX 7900 XTX with both surface states plus device loss followed by continued PTY interaction. Clean `946eb65` now supplies a separately validated ten-minute release-binary AMD/radeonsi OpenGL memory artifact with 954/954 required DRM samples. A separate clean `6ab38bf` release-binary private-Sway run passes lost/outdated-surface and device-loss recovery followed by the interaction and two-window fullscreen scenarios, with Noto and skipped visual comparisons. Equivalent current X11 recovery, accepted CI history, and broader vendor/backend qualification remain release gates. A separate CPU framebuffer is intentionally not implemented. | [recovery contract](renderer-recovery.md), [#33](https://github.com/Tjemmmic/termeric/issues/33) |
| Linux packaging | **Partial** | Debian and Arch artifact construction plus fail-closed archive validation are automated. Exact candidate `cbdc12c` passes guarded install, synthetic-predecessor upgrade, non-GUI version probing, uninstall cleanup, and user-state retention in the pinned official Arch container. The hosted Arch artifact lane and disposable Ubuntu lane still await accepted CI runs; none of this substitutes for a real prior-release upgrade. Arch users have a maintained `termeric-git` PKGBUILD, exact-checkout helper, and checked `.SRCINFO`, but no published AUR package. | [Debian lifecycle](testing.md#debian-package-lifecycle-lane), [Arch lifecycle](testing.md#arch-package-lifecycle-lane), [release acceptance](release-acceptance.md) |

Termeric is therefore a **Linux developer preview**, not a general beta or a
cross-platform daily-driver terminal.

Local source `7df2d38` now also passes private-Sway interaction with all three
renderer-fault injections, two-window fullscreen isolation, and the software-GPU
pixel/readback and unchanged visual-signature checks. These refresh the local
technical evidence only: X11, hosted CI, vendor-GPU, and operator acceptance
are not promoted. Exact environment, hashes, artifacts, and provenance limits
are recorded in [the local GUI evidence](gui-acceptance.md#local-software-renderer-evidence-at-7df2d38).

Native GUI CI now requests clean-candidate provenance: direct harness launches
use one pinned executable, and observed source/binary drift or incomplete
selected scenarios prevents a successful schema-4 result. Debug runs remain
available without that guarantee. This strengthens evidence collection, not
platform promotion; see [the provenance limits](gui-acceptance.md#clean-candidate-mode).

An automated tab-placement scenario now checks top/left/top geometry across
active/inactive tabs and split panes, original PTY retention, and F6
selection/new/close routing. Both native CI paths select it; X11 execution,
hosted results, overflow, drag, IME, and visual acceptance are not inferred from
that wiring. See [tab-placement acceptance](gui-acceptance.md#tab-placement-geometry-and-keyboard-lane).

Overflow keyboard focus now preserves its tab viewport while crossing chrome
controls. A dedicated native regression checks last-close/Previous/New Tab
traversal and prevents navigation bytes from leaking into existing PTYs.
Both layout directions have pure coverage. Short left rails now share one
control row to avoid controls covering tabs; the native regression also checks
compact tab/Previous/Next/New Tab pointer routing. Native overflow visuals,
normal-height arrow activation, drag, IME, and screen-reader acceptance remain
separate checks; CI wiring alone does not establish X11 or hosted acceptance.
See [overflow lane boundaries](gui-acceptance.md#tab-overflow-keyboard-lane).

Held tab drags can traverse hidden tabs through the existing Previous/Next
controls, with deadline-driven 400-ms initial/200-ms repeat dwell. Each move
revalidates the source, pinned/closing/domain boundaries, visibility, and pending
domain authority. Leaving or reversing controls resets dwell; interruptions
clear it, and blocked boundaries retain no timer. Native coverage checks
stationary forward/reverse traversal on normal/compact left rails and the top
strip, with exact owner checks before reselection, layout-specific stopping
indices, restored sibling order, and no unrelated focus loss. Native
pinned/live-domain traversal, X11 execution, and visual acceptance remain
separate checks.

Direct and timed tab dragging now use the same admission check, refusing
closed/closing targets and newly pending same-domain requests. Source routing
must still match the exact identity captured at button-down; finalization cannot
emit a move for a detached/replaced/closed remote owner. Local PTY regressions
cover these cases plus allowed pinned-group movement and unrelated-domain
requests; real native live-domain dragging remains unclaimed.

Divider dragging now rechecks pending same-domain requests at focus refresh and
each resize, not only button-down. An intervening request finishes the gesture
through existing rollback while preserving that earlier request; clearing it
does not restart the drag. Synthetic topology/local PTY tests verify cached
ratios, pane routes, and unrelated-domain isolation without promoting native
live-domain acceptance.

Divider gestures also capture their local/remote ownership at button-down.
Motion and finalization require the original open tab and exact domain/remote-tab
identity; detachment, rebinding, or closure cannot redirect a captured split
resize to another owner. Synthetic regressions cover motion and release-only
rebinding, closed sources, and a local drag acquiring remote metadata. These
checks do not establish native live-domain drag acceptance.

Normal left clicks now exit F6 chrome focus after hit testing, avoiding swallowed
typing and preserving the displayed keyboard-scrolled tab target. The native
overflow lane checks terminal, distant-tab, and split-pane input ownership;
unit checks protect release/non-primary behavior and existing text-field focus.
This does not promote screen-reader, IME, or unrun platform acceptance.

Tab drag motion now verifies the original selected/open tab identity before
reordering, so a held drag cannot move a newly selected tab. The overflow lane
checks forward/reverse local reorder and held-button Ctrl+Tab using exact PTY
owners. Window focus loss now finishes native tab/divider/scrollbar/selection
gestures at their last preview, with one-time live-domain finalization and
rejected-preview rollback. Private Sway tests observe loss/regain for release
while hidden and return while still held; the latter reproduces the old bug.
Modal takeover also finishes native gestures before input dispatch or animation
cleanup, preserving separate panel hover and terminal-protocol release paths.
Private Sway verifies palette interruption without focus loss, PTY query leakage,
or background tab reorder; other modal types have unit rather than native checks.
New presses also respect Settings/Commands/session-panel/dropdown ownership
before background scrollbar/divider hit tests. The native overflow lane checks
outside-divider dismissal for Settings, Commands, and the Workspaces menu;
workspace-panel and scrollbar-specific native acceptance are not inferred.
Geometry synchronization now finalizes native gestures across window resize,
scale, and font/configuration paths, preserving live-domain acknowledgment and
rollback. The private Sway lane verifies a held-tab width change without focus
loss or unintended reorder; unit checks cover scale/direct-sync cleanup and
rejected-preview restoration. Native monitor-scale and live-domain resize
acceptance remain separate.
Scrollbar dragging retains a stable tab/pane owner and cancels on changed focus
or a stale/closing owner; motion cannot retarget a sibling terminal. Raw-PTY
tests verify actual scroll offsets, pane/tab isolation, and cancellation
without relying on focus-refresh callbacks. Native pane-switch and live-domain
scrollbar acceptance remain separate.
Pane-divider motion likewise requires its original open tab to remain selected
and its exact divider to remain visible. Tab switches finish the gesture without
further resizing the hidden tab; pane focus within the same tab remains valid.
Unit coverage checks stale/zoomed targets and live-domain preview rollback with
an earlier request pending; private native coverage checks local PTY geometry.
No-focus/non-modal lost-release, device unplug, edge auto-scrolling,
and native pinned/live-domain drag acceptance remain separate work.

Application mouse reporting also retains the stable terminal that received its
press. Motion/release and focus refresh cancel stale ownership instead of
forwarding held-button state to a newly selected tab or pane. Raw-PTY tests
verify exact destination bytes, closure and no-revival behavior, modifier-safe
release, and preserved unheld hover. The private-Sway interaction lane additionally
checks six ordinary/interrupted native drags across local tabs and panes, with
exact PID-scoped SGR reports, post-release input fences, and no unrelated focus
loss. Its pre-fix negative control detects replacement-tab release leakage.
X11, live-domain, and real-desktop held-button acceptance remain deferred.

Window occlusion and explicit quick-access hiding now finalize native gestures
and clear application mouse capture without waiting for a separate focus-loss
or button-release event. Showing the surface cannot resume selection, overflow
dwell, or divider dragging. Local/synthetic tests preserve ordinary divider
previews and the authoritative rollback/pending-request path for live domains;
they do not establish compositor-specific occlusion delivery or native hidden-
window/live-domain acceptance.

## Terminal compatibility

| Capability | Status | Current boundary | Tracking |
|---|---|---|---|
| PTY and interactive shell launch | **Supported** | Real PTY creation, byte I/O, resize, exit detection, and configured Bash/Zsh/Fish launch paths exist on Linux. Bash composes scalar and indexed-array prompt commands without changing their native hook status/order and retains user PS0 text/expansions; isolated real-PTY comparisons cover both forms and sparse arrays. Unset-safe prompt setup also preserves user-enabled nounset with absent or empty prompt variables instead of raising startup errors. Unambiguous Nushell REPL launches enable native OSC producers without replacing user configuration. Interactive Elvish launches expose a generated module through the standard search path and require one explicit rc import. An isolated Elvish 0.21.0 real-PTY probe passes with the explicit rc import. An isolated Nushell 0.115.1 ordinary-directory probe passes; initially disabled OSC 7 now receives a bounded supplemental report before the first prompt, while the native literal-percent cwd bug remains. Platform evidence outside Linux is unverified. | [shell evidence](integration-acceptance.md#shell-and-harvest-contract), [#23](https://github.com/Tjemmmic/termeric/issues/23) |
| Basic cursor, erase, insert/delete, scroll-region, save/restore, reset, and alternate-screen sequences | **Supported** | Common CSI/ESC operations include Unicode-safe ECH/REP, selective erase, relative cursor movement, full tab-stop control, DSR replies, and cursor overrides. The VT400 rectangle family implements overlap-safe copy/fill/erase and bounded rendition changes with DECOM, wide-cell, BCE, protection, and static-image semantics. Opt-in DECRQCRA returns base VT420/xterm checksums and remains disabled by default because one-cell requests can scrape visible content. DECLRMM/DECSLRM combines horizontal and vertical margins across cursor motion, wrapping, tabs, scrolling, edits, and enclosed images. DECSC/DECRC, CSI s/u, and modes 1048/1049 repeatably preserve screen-local cursor, rendition, G0-G3/GL/GR/pending-shift state, DECOM, delayed wrap, and protection; DECSTR resets the documented live and saved state without clearing content or custom tabs. Custom stops survive retained resize columns, while RIS restores defaults. | [core support manifest](../termeric-core/tests/fixtures/vt_support.toml), [terminal-report security](osc-integration.md), [terminfo contract](terminfo.md), [#16](https://github.com/Tjemmmic/termeric/issues/16) |
| Kitty multiple cursors | **Partial** | Shapes 1/2/3/29, point/main-cursor/rectangle coordinates, clearing, shared unset/special/sRGB/indexed colors, FIFO S7/S8 queries, pane-local rendering, main-cursor-independent visibility, synchronized-output isolation, reset/erase/alternate-screen lifecycle, bounded resize/memory accounting, and compact all-or-nothing local-domain replay have hermetic coverage. Native compositor/GPU visual quality over narrow, wide, scaled, and decorated cells remains deferred manual acceptance. | [profile and evidence](multiple-cursors.md), [#91](https://github.com/Tjemmmic/termeric/issues/91) |
| Kitty terminal file transfer | **Partial** | Linux OSC 5113 `send` sessions can copy regular files, directories, symbolic links, and hard links from a child or remote shell onto the terminal host after one request-scoped approval. Regular-file publication pins its destination parent with no-symlink `openat2` resolution and publishes through retained procfs references; both facilities are required, with no original-path fallback. Publication links from the retained source descriptor, never its mutable staging name, and requires same-mount hard-link support. Unlinked originals fail closed. Staging cleanup checks identity; cleanup/rollback check-and-unlink races and other creation races remain. Symbolic-link and directory creation/identity capture use pinned parents too, including automatically created directories; staging also pins its nearest existing parent and retains that descriptor through cleanup. Leaf substitution before identity capture and non-atomic cleanup remain separate gaps. Hard-link creation also uses the recorded regular-file source handle and pinned destination parent; invalid or unlinked sources fail without a path fallback. Link timestamps use retained no-follow handles and require Linux 5.8+; older kernels fail timestamped-link transfers without a path fallback. Recovering newly created directories under restrictive umasks requires accessible procfs and retained handles, without a destination-path chmod fallback. Relative and absolute symbolic links can target directories in the same transfer, including later-declared entries; directory hard links remain rejected. The `receive` direction can return up to 16 exact regular-file paths shown in a separate one-shot approval, with no pre-consent opens, `openat2` metadata-only no-symlink pinning, single-link regular-file admission before data open, trusted-procfs reopening and opened/path inode revalidation, a 64 MiB aggregate ceiling, fixed 4 KiB payload chunks, eight-chunk acknowledged event batches, acknowledged empty-file completion with exact file/byte counts, and no recursive or link export. The 1,024 destination-entry ceiling includes implicit parents, with unique-path preflight and per-creation enforcement. Download commit rechecks cancellation after filesystem work before accepting completion and rolls back if observed; later cancellation cannot revoke an accepted transfer. Both directions retain exact S7/S8 tab/pane routing, attached-live-domain authority, progress, cancellation, timeout, and shutdown cleanup; detached domains and non-shell plugin/serial terminals fail closed. Quiet levels 1/2, bypass credentials, rsync/deltas, receive-side directory/link export, overwrite/resume, and non-Linux hosts are unsupported. Current real `kitten transfer` acceptance is deferred, so the feature is not yet promoted to Supported. | [profile and deferred acceptance](kitty-file-transfer.md), [#92](https://github.com/Tjemmmic/termeric/issues/92), [#93](https://github.com/Tjemmmic/termeric/issues/93) |
| 16-color, 256-color, and truecolor SGR | **Supported** | Theme-backed indexed/default and truecolor cells render; terminal-local OSC 4/10/11/12 overrides and resets resolve at render time. | [OSC policy](osc-integration.md) |
| SGR text attributes | **Supported** | Bold, dim, italic, slow/rapid blink, inverse, hidden, strikethrough, five underline styles/colors, fallback face preservation, cursor/highlight composition, and visible-only blink scheduling have automated evidence. | [rendering policy](sgr-rendering.md) |
| Unicode grapheme width | **Supported** | Left-to-right extended grapheme clusters, narrow ambiguous width, wide pairs, editing/reflow, search/copy/export, real PTY ingestion, and full-cluster shaping have automated evidence. Bidi and font coverage remain separate boundaries. | [Unicode policy](unicode-support.md) |
| Unicode character picker | **Supported** | A dedicated bounded modal keeps a curated common-character default catalog and searches all 153,626 non-control Unicode 17.0 canonical names with deterministic whole/token-prefix/token-substring ranking, UAX #44 exact name/alias matching, algorithmic Hangul/CJK names, an 8 MiB compact-index ceiling, and at most 256 executable results. A pinned zero-copy 4,342-record CLDR 48.0.0 English short-name/keyword index uses Unicode Emoji 17.0 to preserve exact fully-qualified single-scalar, variation-selector, modifier, flag, and ZWJ grapheme bytes under separate 768 KiB static and 1 MiB result-state ceilings. Exact `U+`, `0x`, and bare-hex queries still reach every valid non-control scalar. Shared render/pointer/AccessKit geometry names each result, accepts IME commits, honors reduced motion, and inserts once only into the exact writable non-broadcast origin. Other locales and glyph-coverage guarantees remain outside the claim. | [Unicode policy](unicode-support.md), [CLDR annotation contract](cldr-annotations.md), [modern input/accessibility](accessibility-and-input.md), [#63](https://github.com/Tjemmmic/termeric/issues/63), [#68](https://github.com/Tjemmmic/termeric/issues/68), [#89](https://github.com/Tjemmmic/termeric/issues/89) |
| Application keyboard modes and modifiers | **Partial** | The focused terminal's DECCKM/DECKPAM state drives physical-key-aware legacy/xterm input. Resource 0 exactly stores, queries, disables, resets, and replicates xterm's 16-bit modifyKeyboard/formatKeyboard state. Runtime DECSET/DECRST 1060/1061 selects mutually exclusive legacy/VT220 keyboard types with historical F-key, editing-key, keypad, and four-bit modifier-permission behavior; both modes share xterm-compatible private save/restore state. `[keyboard] xterm_type` configures the startup/RIS profile for tabs, panes, and local domains. Bounded DECUDK definitions support Shift-invoked VT220 editing keys and F1-F20, focused/broadcast ownership, RIS/DECSTR lifecycle, and domain replication. Normal remains encoder-neutral. Xterm modifyCursorKeys/formatCursorKeys resource 1 supports disabled and modes 0-4 across the portable cursor/edit-key set; modifyFunctionKeys/formatFunctionKeys resource 2 covers F1-F35, including disabled remapping; modifyKeypadKeys/formatKeypadKeys resource 3 covers 18 portable numeric-keypad keys and 11 Num Lock-off identities; modifyOtherKeys/formatOtherKeys resource 4 supports bounded levels 0-3 and modifier masks; reserved modifyStringKeys/formatStringKeys resource 5 stores, queries, disables, resets, and replicates encoder-neutral compatibility state; modifyModifierKeys/formatModifierKeys resource 6 covers 13 portable modifier/lock identities; modifySpecialKeys/formatSpecialKeys resource 7 covers 72 canonical control, Compose/IME, and XF86 identities plus exact convertible raw XKB special keysyms. Resources 1-7 provide bounded query/reset/disable, pane/broadcast ownership, Kitty precedence, and local-domain replication, with exact CSI-27/CSI-u mode-4 output where applicable. Kitty's five progressive flags negotiate per screen and pane, with bounded stacks, disambiguated/all-key events, physical base-layout alternates, common functional/keypad keys, Super, press/repeat/release, and associated keyboard/IME text. The unavailable Begin identity, initial Caps/Num-lock, held Hyper/Meta state, Winit-unavailable ISO/Shift-lock identities, collapsed named-key aliases, and native-client interoperability remain outside the claim. | [keyboard policy](keyboard-input.md), [#16](https://github.com/Tjemmmic/termeric/issues/16) |
| Synchronized output | **Supported** | DECSET/DECRST 2026 freezes a bounded visible-grid, dynamic-color, and image snapshot for only the requesting terminal while live PTY parsing continues. Other panes and native UI keep presenting. DECRST exposes the completed state; a terminal-local one-second safety deadline releases a stalled batch and schedules its final frame. | [event-driven rendering](event-driven-rendering.md), [#6](https://github.com/Tjemmmic/termeric/issues/6) |
| Mouse, focus, and bracketed-paste reporting | **Supported** | Focused-pane mode snapshots drive X10/VT200/button/any-event mouse reports, UTF-8/SGR coordinates, modifiers, DECSET 1007 alternate scroll, exact native focus transitions, and one-write bracketed paste. Shift is the native selection/scrollback bypass; X11 native-window automation exercises mouse and paste end to end. | [input protocol policy](terminal-input-protocols.md), [GUI acceptance](gui-acceptance.md) |
| Terminal-controlled mouse pointer shapes | **Partial** | Kitty/xterm OSC 22 supports all 30 required CSS-derived names, plain/`=` set and empty reset, atomic comma-separated push, pop, current/default/grabbed and capability queries, fixed 16-entry eviction, separate primary/alternate stacks, DECSTR/RIS cleanup, exact pane event/reply routing, and exhaustive Winit icon mapping. Empty stacks use `text`, or `default` while mouse reporting is active; Termeric-owned selection, divider, and native file-drag cursors take temporary precedence. Parser/state/mapping evidence passes, but native X11/Wayland visual acceptance of every platform/theme cursor remains deferred. Legacy X11 cursor-font aliases and configurable pointer defaults are not claimed. | [OSC policy](osc-integration.md), [#69](https://github.com/Tjemmmic/termeric/issues/69) |
| Device Attributes and DEC character sets | **Supported** | Byte-exact DA1/DA2/DECID replies route to their origin; DA1 truthfully adds the implemented Sixel capability. ASCII, UK, and complete DEC Special Graphics sets can be designated into G0-G3, invoked through GL with single or locking shifts, and preserved through reports, restore, RIS, DECSC/DECRC, and 1049. Stored GR invocation is reportable, but UTF-8 non-ASCII scalars are not remapped through it; later and 96-character sets are outside this claim. | [identity and charset contract](device-attributes-and-charsets.md), [#11](https://github.com/Tjemmmic/termeric/issues/11) |
| Terminal capability and status queries | **Partial** | Bounded 7-bit and UTF-8-safe raw C1 framing answers XTVERSION, Kitty keyboard state, and multi-name XTGETTCAP for the child-selected terminal identity, color depth, Fish extensions, and every advertised special-key string. XTGETTCAP stops at the first invalid name and returns one bounded xterm-compatible reply. Read-only XTWINOPS reports exact text-area pixels, rounded cell pixels, rows/columns, and isolated application-owned window/icon labels; its fixed 10-slot xterm title ring supports push/pop, direct slots, partial-entry fallback, and position reporting without enabling native window mutation. DECRQSS adds xterm-compatible DECSLPP/DECSCPP/DECSNLS geometry without enabling their mutation controls and covers the other documented settings, while DECRQPSR/DECRSPS round-trip cursor information and tab stops. Supported ESC/CSI/DCS/OSC/APC protocol bodies accept GR-byte equivalents while human-text OSC paths preserve UTF-8. S7C1T/S8C1T and bounded DECSCL levels 1-5 control terminal-local reply framing; DECSCL applies soft-reset semantics and is status-reportable but does not gate newer parser features. DECRQM returns byte-exact live state for documented ANSI and DEC modes; unknown modes report unrecognized. Host-database/XTSETTCAP operations, DEC status-line and VT525 DECRQSS families, strict historical conformance-level gating, 96-character or multi-page presentation state, alternate-screen alias reports, and other XTWINOPS are not claimed. | [terminal conformance](terminal-conformance.md), [identity and charset contract](device-attributes-and-charsets.md), [integration acceptance](integration-acceptance.md), [terminfo contract](terminfo.md), [PTY pixel contract](pty-pixel-geometry.md), [OSC policy](osc-integration.md), [#16](https://github.com/Tjemmmic/termeric/issues/16), [#70](https://github.com/Tjemmmic/termeric/issues/70) |
| PTY pixel geometry | **Supported (Linux)** | Spawn and resize report matching nonzero rows, columns, and physical grid pixels for single tabs, nested/asymmetric panes, and protocol-v9 daemon-owned live panes; exact repeats are deduplicated and zero-pixel external relays preserve the last valid physical cell extent. Other platforms remain unverified. | [policy and evidence](pty-pixel-geometry.md), [#14](https://github.com/Tjemmmic/termeric/issues/14) |
| Scrollback and reflow | **Supported** | Line/byte-bounded compact history, navigation, resize reflow, styles, hyperlinks, and Unicode wide-pair invariants have automated evidence. Scrolled-back viewports stay anchored to retained rows during full-screen scrolling, top-origin full-width region scrolling, and viewport archival, including capped and shrinking history; actual eviction clamps the view to the oldest remaining row. | [memory model](scrollback-memory.md), [Unicode policy](unicode-support.md) |
| Selection and clipboard copy/paste | **Supported** | Selection/copy/paste route through the focused pane, copy complete graphemes once, paste honors DECSET 2004, and bounded OSC 52 reads/writes follow an explicit deny-read/allow-write policy and stable origin routing. Native mouse selection, word/line expansion, rendering, and copied text use the synchronized presentation, with clipped viewport capture and snapshot-bound drag invalidation. Native drag/release and completed pointer caches follow the terminal's canonical selection during output scrolling and viewport archival; history eviction/clearing and reset cannot resurrect stale anchors. Partial-height and horizontal-margin vertical scrolling, insert/delete lines, line feed, and reverse index preserve stationary or uniformly surviving selections and clear erased/mixed-motion ranges. The constant-time mapping has cellwise-reference and protocol coverage. Ordinary printable writes invalidate overwritten selections, including Unicode grapheme growth, wide halves, and OSC 66 collisions, while preserving unrelated/history/frozen selections. ED/EL/ECH and selective/rectangular erases now invalidate selections from actual glyph damage, preserving protected selections when their cells survive. DECFRA/DECCRA invalidate destination replacement and actual wide/multicell repair damage while preserving untouched copy sources, no-ops, history, and frozen selections. Standalone ICH/DCH follow uniformly surviving selections, preserve stationary ranges, and clear deleted/mixed-motion ranges and actual wide/multicell repair damage. New OSC 66 placement invalidates its destination, whole-block collisions, insert-mode shifts, and repaired ordinary wide halves while preserving skipped blocks and frozen/history selections. Vertical scroll/line-edit glyph repairs now invalidate actual damage outside the moving rectangle while retaining intact survivors. Grapheme width changes share normal text collision/skip handling instead of leaving partial multicell descriptors; print-time erasure/repair reports selection damage. Height-only resizing tracks the exact archived-row shift and retires clipped, evicted, or glyph-damaged selections. Width-changing reflow tracks Simple/Semantic stream endpoints through compact history, wide-character padding, one-column layouts, and retention limits without a per-cell map; lost endpoints retire instead of aliasing other text. Block/physical-line selections retire on width changes because their geometry cannot preserve the same range. Filtered selection maps noncontiguous rows, copies only matching rows across virtualized pages, reuses the existing result index, and invalidates stale projected ranges. Accessible text and selection endpoints follow the same displayed rows rather than hidden live content. These newer paths have hermetic coverage; native painting, clipboard, screen-reader, and desktop permission behavior remain manual acceptance. | [focused routing and mouse selection](focused-pane-routing.md#mouse-selection-presentation), [filtered selection](focused-pane-routing.md#filtered-selection-and-accessible-text), [input protocol policy](terminal-input-protocols.md), [OSC policy](osc-integration.md) |
| Search | **Supported** | Literal, regex, case, filter, navigation, and pane-focus rebinding work. The bounded 4 KiB UTF-8 query editor honors insertion and deletion at its visible cursor, provides an empty-query affordance and explicit close action, and shares one geometry contract across rendering, pointer hit-testing, and AccessKit. Screen readers receive editable query state, polite result progress/counts, assertive regex errors, toggle state, and exact navigation controls. Full-history row scanning no longer runs inside the input callback: scans are cancellable, hidden until complete, and cooperatively limited to roughly 4 ms per rendered frame. Exact occurrence totals and every matching row are retained, while highlighting/navigation use a disclosed circular window of the latest 131,072 occurrences; together with the validated one-million-line history ceiling, this keeps deterministic result indexes below the explicit 8 MiB budget. Filter rendering is virtualized to the focused viewport and wheel-scrollable rather than rebuilding every matching line per frame. Completed literal-query extensions move and reuse the prior matching-row set without copying it, Unicode byte offsets map back to terminal cells, and separate sparse/dense completion, memory, keyboard-dispatch, text-editing, geometry, and semantic-action rails prevent regressions. Total first-scan work remains linear in retained history. Search reads the displayed grid and refreshes across synchronized snapshots, screen switches, resize, and live content revisions, including same-size edits and capped-history rotation. Cursor/viewport navigation, inactive output, and hidden synchronized writes do not invalidate the focused displayed content. Rescans reuse the matcher and existing budget; continuous output can keep results pending, and changed-row incremental indexing remains open. Command-output scopes rescan in-place edits but close on source or row-mapping changes and cannot start while a synchronized viewport is frozen; moving/frozen scope remapping is not implemented. These newer presentation paths have hermetic coverage with native acceptance deferred. | [focused routing and search presentation](focused-pane-routing.md#search-presentation), [benchmark method](performance.md), [event-driven scheduling](event-driven-rendering.md), [modern input/accessibility](accessibility-and-input.md), [#21](https://github.com/Tjemmmic/termeric/issues/21) |
| OSC 8 hyperlinks and detected URLs | **Supported** | Semantic and detected links render and activate from the focused pane with pane-local and scrollback-aware coordinates on Linux. Filtered results now project detected underlines and Ctrl+click targets through displayed source rows, preserve OSC 8 priority, reject blank result rows, and break detected URLs across omitted source rows. Detected targets now preserve Unicode IRI text without rewriting, exclude unmatched closing brackets, and map wide/combined/emoji cells through wrapping, reflow, and history. All eight existing schemes accept ASCII case variants without altering target bytes or allocating normalized lines; exhaustive case permutations, wrapped/nested cache mapping, and an allocation probe cover this change. Conservative separator/control boundaries and retained detection limitations are documented in [Unicode links](focused-pane-routing.md#detected-unicode-links). Hermetic tests cover exact target bytes, Unicode scalar/delimiter boundaries, filter changes, history, Unicode, cache reuse, and frozen single/split-pane targets without opening them; native acceptance of these newer filtered paths remains deferred. | [focused routing](focused-pane-routing.md#filtered-link-presentation) |
| OSC title, clipboard, and dynamic colors | **Supported** | Bounded OSC 0/1/2, OSC 52, and OSC 4/10/11/12 plus reset families use typed events, terminal-local state, exact pane/tab routing, xterm query replies, and documented security/precedence. OSC 0/1/2 retain separate application-owned window/icon labels for bounded XTWINOPS queries and save/restore without exposing UI/native metadata. The active tab title is reflected in its owning native window without allowing inactive origins to retitle it. | [policy and evidence](osc-integration.md), [#10](https://github.com/Tjemmmic/termeric/issues/10), [#70](https://github.com/Tjemmmic/termeric/issues/70) |
| OSC 133 shell semantics | **Supported (Linux)** | Isolated real Bash 5.3, Zsh 5.9.2, and Fish 4.8.1 PTYs preserve custom rc files and produce multiline success/failure command regions. Unambiguous Nushell REPL launches enable Nushell's native OSC 133 setting after user configuration. Interactive Elvish launches expose a generated module that appends hooks after one explicit `use termeric-integration` rc entry. Deterministic routing fixtures pass for both; an isolated Elvish 0.21.0 real-PTY probe also preserves custom rc and yields complete success/failure regions. Nushell 0.115.1 also passes the ordinary-directory multiline success/failure probe; its directory-reporting gaps remain documented below. The bounded state machine validates `A → B → C → D`, ignores stray/duplicate boundaries and prompt-only cycles, recovers a missing `D` at the next `A` only after output began, and accepts exit statuses only from 0 through 255. Previous/Next Prompt use a per-terminal bidirectional cursor over retained signed rows, preserve the selected view during output, follow resize reflow, reset after direct scrolling, and reject evicted or alternate-screen targets. Resize tracks exact logical-prefix eviction, including partial wide-character lines, so evicted command IDs cannot resolve unrelated text at the new history origin. Boundary capture and restoration now share sorted batches with prompt navigation and scan retained history at most once per direction, with scalar-reference correctness tests and a deterministic row-visit rail. ED2, ED3, RIS, and the user Clear Scrollback action retire only metadata whose backing primary rows were erased; stable completion identity remains bounded, while alternate-screen OSC 133 is ignored. Fish 4 and modern Nushell use native marks; Fish 0-3 receive fallback hooks. Unsupported shells, scripted/special-mode launches, Nushell profiles with an existing pre-REPL command, and Elvish `-norc` launches are left untouched and carry an explanation. | [integration acceptance](integration-acceptance.md), [#26](https://github.com/Tjemmmic/termeric/issues/26) |
| Shell-reported working directory | **Partial** | Bounded OSC 7 `file://host/path` reports are terminal- and pane-local, fragmented/C1-safe, strict percent-decoded UTF-8, and preferred over Linux process inspection for titles, Git/plugin context, pane cwd inheritance, sessions, workspaces, and local-domain controls. Remote authorities remain display-only and never become local filesystem paths. The generated Bash, Zsh, Fish, and opt-in Elvish emitters use bounded URI encoding, preventing directory text from injecting terminal commands; oversized reports are omitted whole while prompt hooks continue. Elvish byte tests cover both registered hooks, and its isolated native editor/PTY startup probe passes. Modern Nushell 0.115.1 passes the ordinary-directory native probe, and a bounded conditional supplemental report fixes user-disabled OSC 7 at the first prompt. Native literal `%2F` paths are still misdecoded, tracked by one ignored known-failure regression; real desktop/user-configuration acceptance is separate. Other platforms retain portable parsing/policy code but lack native acceptance. | [OSC policy and remaining emitter work](osc-integration.md#working-directories), [focused routing](focused-pane-routing.md), [#26](https://github.com/Tjemmmic/termeric/issues/26) |
| Terminfo and capability negotiation | **Supported (Linux)** | The versioned `xterm-termeric` entry advertises verified color, cursor/editing, `ech`, `rep`, `cbt`, `tbc`, tmux-compatible `Enmg`/`Dsmg`/`Clmg`/`Cmg` horizontal margins and `Rect` rectangle fill, plus non-destructive DECSTR initialization/reset behavior. It is compiled into Linux packages and selected only when discoverable; the child pins `TERMINFO` to that exact root so inherited databases cannot contradict the selected identity. `xterm-256color` remains an explicit compatibility fallback; remote hosts need the entry installed before SSH forwards the custom name. | [terminfo contract](terminfo.md), [identity boundary](device-attributes-and-charsets.md), [#25](https://github.com/Tjemmmic/termeric/issues/25) |

Elvish's module exposure follows recognized native Go-style flags, final
boolean values, and operand boundaries. Script/command, active utility/no-rc,
unknown, and malformed launches retain argv/environment without generated
files. Two routing tests, 50 native typed-option comparisons, and five Elvish
0.21.0 PTY variants cover this boundary. Integration still requires an explicit
import in the user's effective rc file, including custom `-rc` paths; no
automatic startup injection is claimed. See
[Elvish launch handling](osc-integration.md#elvish-launch-and-opt-in-module-handling).

Elvish's search-path handoff now restores only the exact temporary environment
it installed, preserving profile/rc overrides, empty/unset distinctions, and
native default roots. Three pure cases, ten native module executions, and three
editor/PTY variants cover this behavior and child-marker cleanup after import.
Unconsumed transports and unidentified legacy roots have explicit limits; see
[module-search environment handoff](osc-integration.md#elvish-module-search-environment-handoff).

Fish uses a shell-specific REPL option allowlist and installs integration after
user startup commands as a separate argument, retaining their exact text/order
and preceding scalar exit status. Two routing tests, six startup comparisons
each on Fish 3.7.1/4.9.3, and four Fish 4.9.3 PTY variants cover this boundary.
Command/script, no-config/no-execute, utility, and unknown modes run unchanged
without integration. See [Fish launch handling](osc-integration.md#fish-launch-and-startup-command-handling).

Bash now supports explicit custom `--rcfile`/`--init-file` paths with native
last-option file selection and no PATH-search fallback. Routing, reentry,
seven native startup comparisons, and four PTY variants cover this behavior.
POSIX/no-rc and unrecognized modes run untouched without integration. See
[Bash launch handling](osc-integration.md#bash-launch-and-custom-rc-handling).

Recognized interactive Bash login launches now use a guarded startup bootstrap
that restores ordinary Bash mode and environment before user profiles, preserves
login/logout identity and file-scope declarations, and installs the shared
OSC 7/133 hooks. Eighteen native startup comparisons and three isolated PTY
variants pass on Bash 5.3.15; custom system-profile layouts, arbitrary user code,
and older runtimes remain outside that evidence. See
[Bash login integration](osc-integration.md#bash-login-shell-integration).

Bash hook setup preserves `allexport` without accidentally exporting private
functions or previously unexported prompt variables. Prompt tracking remains
shell-local when `set -a` is enabled later, and inherited stale private state is
discarded. Eight child-environment cases and scalar/array real-PTY comparisons
pass on Bash 5.3.15; explicit user prompt exports remain explicit and are not a
guarantee of clean nested-shell composition. See
[Bash environment boundaries](osc-integration.md#bash-launch-and-custom-rc-handling).

The optional `run_shell_acceptance.py --include-extended-shells` lane now seals
candidate-bound Bash/Zsh/Fish/Nushell/Elvish observations together with exact
selected test results. Nushell's native cwd check remains an accepted limitation
for the known literal-percent bug, and Elvish coverage requires an explicit rc
import. It does not promote provider, desktop, or hosted-CI acceptance; see
[five-shell evidence](integration-acceptance.md#shell-and-harvest-contract).

Zsh forwards user `.zshenv`, login `.zprofile`, and `.zshrc` at file scope,
restoring each effective `ZDOTDIR` before user code and preserving subsequent
directory/export changes. Recovery tests, four directory-handoff comparisons,
nine startup-chain comparisons, and an isolated login-profile PTY probe cover
this behavior. Optional global startup files can still observe or redirect the
temporary directory between stages; full configuration acceptance remains
incomplete. See
[the Zsh startup boundary](osc-integration.md#zsh-configuration-directory-handoff).
Zsh launch routing also preserves command/script, startup-disabled, and unknown
forms without changing their environment. Recognized option order/inversions,
end markers, and stdin positional arguments have pure routing coverage and
four real-PTY acceptance variants; Fish argument routing remains separate.

Nushell integration now recognizes scalar REPL options such as `--config-home`
and grouped short flags while preserving command, service, custom-prelude, and
unknown launches without injecting setup. Three isolated real-PTY option
variants pass; this does not close the native percent-path gap. See
[launch eligibility and its test boundary](osc-integration.md#nushell-launch-eligibility).

The shell versions above are historical observations, not a current candidate
record. The [required-shell evidence runner](integration-acceptance.md#shell-and-harvest-contract)
from [#113](https://github.com/Tjemmmic/termeric/issues/113) binds Bash/Zsh/Fish
observations to exact source and executable identities. Missing required shells
fail; zero observed capability replies remain explicit limitations. A green
current three-shell artifact remains pending, independently of native/provider
acceptance.

## Text and graphics

| Capability | Status | Current boundary | Tracking |
|---|---|---|---|
| Font discovery and fallback | **Partial** | Linux font discovery feeds the settings selector. The configured primary plus as many as 16 unique fallbacks are applied in exact order to complete Unicode graphemes, with platform fallback only as a final safety net. Up to 64 validated non-overlapping Unicode-scalar ranges can prefer an installed family ahead of that chain; lookup is precompiled/binary-searched, preserves complete graphemes and primary cell metrics, falls through safely, applies on live reload, and invalidates row/shaping/atlas caches. Native color fonts and real or bounded-synthetic bold/italic faces work. Validated 1–1000 normal/bold `wght` controls reach scalar, grapheme, ligature, fallback, range, and OSC 66 shaping; variable fonts use their real axis without double synthetic emboldening. Settings exposes those weights, the fallback chain, line height, ligatures, bold-as-bright, a bounded 32-entry custom OpenType feature manager, and a bounded request-scoped range-rule manager with canonical add/edit/confirmed-delete flows, atomic shared validation, stale-identity refusal, and explicit Save persistence. Its on-demand 256-byte/16-grapheme coverage inspector reuses renderer routing, distinguishes missing/incomplete/complete explicit families, and names bounded platform shaping faces without claiming native visual proof. Arbitrary `wdth`/`slnt`/`ital`/`opsz`/custom axes, exhaustive system-font recommendations, broader platforms, and native visual coverage remain open; the separate Unicode picker itself does not compute coverage while filtering. | [SGR policy](sgr-rendering.md), [atlas lifetime](glyph-atlas.md), [Unicode policy](unicode-support.md), [#32](https://github.com/Tjemmmic/termeric/issues/32), [#64](https://github.com/Tjemmmic/termeric/issues/64), [#65](https://github.com/Tjemmmic/termeric/issues/65), [#66](https://github.com/Tjemmmic/termeric/issues/66), [#67](https://github.com/Tjemmmic/termeric/issues/67), [#73](https://github.com/Tjemmmic/termeric/issues/73) |
| Programming ligatures | **Supported** | Compatible one-column row runs use cached `cosmic-text` shaping. The simple Ligatures preset controls four common tags; up to 32 validated explicit OpenType values can override that preset while `kern` remains disabled. Effective feature state is cache-keyed and applies live without changing cell geometry, graphemes, wide cells, or attribute boundaries. | [policy and evidence](ligature-rendering.md), [#9](https://github.com/Tjemmmic/termeric/issues/9), [#67](https://github.com/Tjemmmic/termeric/issues/67) |
| Text sizing and explicit cell widths | **Partial** | Kitty OSC 66 accepts strict bounded `s/w/n/d/v/h` metadata and up to 4096 bytes of safe UTF-8. It supports natural grapheme segmentation or explicit width, atomic multirow grid ownership, wrapping and overwrite rules, destructive/editing controls including ED 22 archival, scroll/resize cleanup, compact history, semantic extraction, ANSI session and local-domain replay, font-size-aware shaping, fractional alignment, pane clipping, and full-block cursor geometry. Invalid operations fail closed and ordinary cells retain their allocation-free path. Hermetic parser/grid/renderer tests pass; native font quality, clipping, cursor appearance, and application compatibility remain manual and unclaimed. | [protocol and deferred acceptance](text-sizing.md), [#72](https://github.com/Tjemmmic/termeric/issues/72) |
| Glyph atlas lifetime | **Supported** | Separate frame-safe LRU atlases cap masks at 8 MiB and native RGBA glyphs at 16 MiB; cached rows pin typed page identities, source color is not theme-tinted, and pressure remains bounded. | [policy and evidence](glyph-atlas.md), [#13](https://github.com/Tjemmmic/termeric/issues/13) |
| Color emoji | **Supported** | Swash RGBA pixels use a separate sRGB-encoded RGBA atlas and shader branch while Unicode grapheme width continues to control cell geometry. Native font availability remains platform-dependent. | [modern input/accessibility](accessibility-and-input.md), [#32](https://github.com/Tjemmmic/termeric/issues/32) |
| Terminal background images | **Partial** | Optional local static PNG/JPEG backgrounds use typed cover/contain/stretch/center geometry beneath the shared terminal content viewport. One generation-safe worker per window enforces encoded, decoded, dimension, path, and concurrency caps; file replacement hot-reloads, stale results are ignored, failures retain the last good texture, and steady state drops the CPU RGBA copy. Completions retain the decoded handle's version, preventing a newer replacement from being mistakenly marked loaded; Unix polling also detects preserved-mtime/length replacements by identity/change time. Handle-based admission, bounded reads, and nonblocking Unix opens reject substituted non-regular sources without waiting for FIFO writers. Configuration, geometry, decoder admission, and replacement/generation routing have hermetic evidence. The opt-in GPU lane adds 104 all-pixel offscreen checks for PNG/JPEG, every fit, opacity, shifted/resized viewports, foreground-rectangle composition, repeated/replaced/cleared images, CPU-copy release, texture cache count, and pipeline recreation. The private-Sway lane adds sampled native PNG/JPEG fit/opacity/top-left-tab pixels, explicit cell-background layering, search/split/resize, file replacement/refusal/disable, and requested simulated renderer recovery. Full plugin chrome, driver memory usage, equivalent X11 presentation, physical GPU recovery, and real-desktop visual/contrast acceptance remain pending. Tiling, animation, remote URLs, per-pane/per-tab images, and a native picker are unsupported. | [background image contract](background-images.md) |
| Kitty graphics | **Partial** | A bounded profile supports direct streamed RGB/RGBA/PNG, padded and raw-unpadded base64, current-Kitty 128-KiB chunks, zlib across direct and local media, chunking, tightly constrained Unix temporary files, private same-user POSIX shared memory through slash-prefixed or current-Kitty bare names with mandatory cleanup, and default-denied opt-in Unix ordinary files in non-domain local tabs with `S`/`O` ranges, final-target identity checks, protected-system-root denial, and no deletion. Early parse and framing failures honor `q=2` without leaking replies into the shell. Natural and single-axis destinations derive exact pixel and cell footprints from live PTY geometry. Non-unique image numbers allocate collision-free IDs, resolve the newest retained match, survive chunking, support numbered placement/deletion, and remain coherent through replacement and eviction; advisory usage hints are accepted. Unicode placeholders support virtual prototypes, exact indexed/true-color image and placement IDs, high ID bytes, diacritic inheritance, aspect-preserving per-cell crops, horizontally coalesced renderer spans, and grid-driven edit/reflow/scrollback/synchronized-output behavior. Synchronized capture rebases scrolled image coordinates with the frozen text viewport; renderer-geometry regressions cover ordinary, Unicode, relative, and pixel-sized images at unchanged dimensions. Height-only resize now follows each live or frozen grid's exact row archival, with relative-parent lifetime, history eviction, and unchanged source geometry; ordinary placements keep fixed coordinates on width changes, retire roots beyond the resized history boundary, and preserve reversible viewport clipping. Frozen Unicode placeholders and their relative descendants now resolve from the resized captured grid using bounded, isolated placement metadata and shared active-frame pixels; width/height reflow and captured-history navigation have renderer-geometry regressions, while native resize acceptance remains deferred. Host font/scale changes update frozen derived geometry without replacing captured content; tab, pane, and domain owners refresh live/frozen pixel footprints before resize eviction, with core, renderer, and real-PTY regressions. Pixel-sized regional scroll and rectangle copy/erase crop at actual cell-pixel boundaries; offset-aware footprints preserve partly covered cells, with core pixel-oracle and parser-to-renderer geometry/UV regressions rather than native visual acceptance. Relative placements support signed offsets, 32-level bounded chains, ordinary or Unicode parents, stable named-parent replacement, exact parent/cycle/depth errors, and cascading lifetime. Frame animation supports direct and local loading, chunk continuation, partial edits, base/color canvases, blend/replace composition, stopped/loading/finite-loop control, exact active-tab deadlines, frame deletion/root promotion, synchronized-output isolation, renderer revisions, and independent frame/memory caps. Protocol-v8 live-domain attachment restores decoded roots/frames, IDs, aliases, normal/alternate/virtual/relative placements, LRU order, revisions, and animation clocks through a bounded one-shot replica channel. Relay begin/failure cleanup preserves host cell metrics, and metadata commit recomputes offset-aware pixel footprints using current receiver geometry, including mid-import font/scale changes; explicit cell-sized destinations and image content remain unchanged. Domain PTYs and relay replicas always deny ordinary-file reads. Queries, geometry, lifecycle, deletion, and truthful replies are covered. Core regressions also cover full-width top-origin image archival and DL from row zero, retained text/image identity, fixed footers, relative descendants, eviction without premature source cropping, and immediate retirement on scrollback clear or budget reduction. Unix file opens and Linux shared-memory load/cleanup opens reject FIFOs without waiting for a writer; substitution, rejected-object preservation, quiet replies, and next-image recovery have Linux regressions, but regular-file I/O remains synchronous. An additional opt-in parser-to-GPU lane checks 128 full frames against an independent CPU pixel reference for local and relay-restored partial-cell Kitty/Sixel/iTerm2 scroll/copy/erase, Kitty offsets, and frozen/released output; portable negative tests prevent missing/stale/overpainted frames from passing. A headless GPU readback proves all 52 classic and Unicode-placeholder rows appear together through initial, output-scrolled, and restored-scrollback frames. The X11 workspace harness launches current `kitten icat` classic/Unicode streams, stdin-backed temporary-file and POSIX shared-memory transfers, and independent `timg -pk` direct-PNG output, binding every fixed or client-generated ID to terminal acceptance diagnostics and both local-media objects to observed cleanup. A candidate artifact plus native visual, ordinary-file policy, multi-pane, animation, relative-placement, and Wayland judgment remain open. | [profile and evidence](kitty-graphics.md), [GUI acceptance](gui-acceptance.md) |
| Sixel and iTerm2 images | **Partial** | Bounded static Sixel and OSC 1337 PNG profiles share the normalized Kitty cache/renderer path, exact pixel sizing, pane clipping, scroll/resize/alternate-screen lifecycle, and resource limits. Pixel-sized regional scroll and rectangle copy/erase retain exact destination coverage at partial-cell boundaries, with outward-rounded source texels, hermetic geometry coverage, and opt-in software-GPU full-frame comparison through real fragmented protocol input. Sixel applies every defined DEC macro aspect and raster `Pan:Pad` override to bounded display geometry without expanding logical RGBA storage. The native Linux acceptance harness requires real `img2sixel` and `timg -pi` clients, but a current accepted native artifact and subjective visual review are still pending; iTerm2 downloads/non-PNG media, stored Sixel macros, private extensions, and animation remain unsupported. | [profile and evidence](inline-images.md), [#31](https://github.com/Tjemmmic/termeric/issues/31) |
| IME/preedit | **Partial** | Winit IME is enabled; pane-positioned preedit remains local, commit is delivered once to the focused text owner, and focus changes cancel stale composition. Command, Unicode, and editable domain-picker filters use atomic bounded commits and their shared painted query viewport for candidate/preedit geometry; domain loading/preparation/error stages block text, and accepted stage changes retire old composition. Deterministic routing, geometry, lifecycle, and raw-PTY isolation tests pass, but real IBus/Fcitx, native CJK composition, and dead-key acceptance have not yet been recorded on a supported desktop. | [modern input/accessibility](accessibility-and-input.md), [#32](https://github.com/Tjemmmic/termeric/issues/32) |
| Accessibility and screen readers | **Partial** | AccessKit exposes selected tabs, named global controls, panes, visible text, cursor/selection, polite incremental updates, and actions. The command palette, staged live-domain picker, settings, custom-command manager, workspace/legacy-snapshot manager, shortcut editor, destructive-command guardrail, bounded information/error/choice dialog, diff review, context assembler, and Harvest command/action surfaces are isolated modal trees with stable identities, active selection/focus, typed controls, exact shared bounds, validated actions, and fail-closed routing. Terminal search exposes its editable UTF-8 query, result/error status, toggle state, navigation, and close action through the same geometry used for painting and pointer input. Bounded transient toasts retain stable identities and exact card bounds, remain present above modal trees, and use polite announcements except for assertive errors. Information notices and diff review provide two-axis navigation, visible-only document content, position status, explicit actions, and stale-action rejection. Context assembly exposes typed checkboxes, a workspace-bounded file-path editor, background status, pointer/wheel controls, and an exact captured terminal destination. Harvest exposes bounded real command text and health, newest-first opening, list navigation/scrolling, an explicit close action, and request-scoped sprout menu items. Guardrail dismissal revokes pending input immediately; typed, pasted, and broadcast confirmations retain exact destinations and never fall back to current focus. F6 provides visible keyboard-only chrome focus; hidden cells and offscreen buffers are excluded. Three-state reduced-motion/high-contrast policies support explicit overrides and live Linux XDG Desktop Portal inheritance without blocking startup; legacy booleans remain compatible. Reduced motion snaps non-essential application transitions and removes Harvest pulsing while retaining terminal/application animation, cursor/text blink, and the visual bell. High-contrast mode enforces a tested 4.5:1 text floor for built-in application chrome without rewriting terminal colors. Native import on macOS/Windows and accepted Orca, VoiceOver, or Narrator evidence remain missing. | [modern input/accessibility](accessibility-and-input.md), [#32](https://github.com/Tjemmmic/termeric/issues/32) |

## Workspace and interaction

Information dialogs have request-scoped Tab/Shift+Tab focus across all declared
choices, input, and footer buttons, with visible focus, Enter activation, and
shared AccessKit identity. New consent dialogs retain a non-consenting default;
local tests do not establish native screen-reader acceptance. See
[dialog accessibility](accessibility-and-input.md).

| Capability | Status | Current boundary | Tracking |
|---|---|---|---|
| Tabs | **Partial** | Create, close, switch, numbered navigation, explicit rename precedence, pinned grouping, shortcut/direct-drag reorder, bounded overflow navigation with active/focused-tab auto-reveal, and complete process-local transfer to the next native window work on Linux. A typed live setting now switches between the established top strip and a DPI-aware bounded vertical-left rail; render, pointer, drag, keyboard-focus, PTY/pane sizing, search/IME, and AccessKit geometry share the same rail offset. Whole-tab transfer preserves split/plugin metadata, reassigns only the destination-local ID, and closes an emptied source window; live-domain placement remains topology-owned. Automated geometry/routing coverage passes, but native X11/Wayland visual, drag, IME, and screen-reader acceptance for the new rail is pending. | [tab-bar placement](tab-bar.md), [advanced controls](advanced-workspaces.md), [modern input/accessibility](accessibility-and-input.md), [#35](https://github.com/Tjemmmic/termeric/issues/35) |
| Terminal launch profiles | **Partial** | Process profiles are supported. Up to 64 validated profiles provide exact executable/argv/cwd/bounded environment or typed Linux serial device, baud, framing, parity, and flow control. CLI and hot-reloaded palette launches preserve workspace identity; process panes inherit their frozen launch while serial tabs reject implicit splits. The serial bridge is descriptor-validated, exclusive, raw, binary-transparent, and bounded, but physical USB/UART permission, line-setting, exclusivity, and disconnect evidence is not yet recorded. Other platforms remain unverified. | [profile contract](terminal-profiles.md), [CLI contract](command-line.md) |
| Split panes | **Supported** | Linux split/create/close/navigation, configurable cwd inheritance, zoom, swap, balance, columns/rows/grid plus focused-master Tall/Fat presets, window-local live-PTY move, focused operations, pane-local images, draggable dividers, lifecycle ownership, bounded event draining, and origin-correct replies have automated evidence. Pane/local-divider allocation history survives collapse/re-split and rollback; stale retired-ID routes fail instead of targeting replacement owners, with checked exhaustion. | [advanced controls](advanced-workspaces.md), [lifecycle](pane-lifecycle.md), [event ownership](pane-event-routing.md), [#94](https://github.com/Tjemmmic/termeric/issues/94), [#116](https://github.com/Tjemmmic/termeric/issues/116) |
| Read-only pane policy | **Partial** | A transient pane-local lock centrally denies keyboard, IME, paste/drop, mouse/focus reports, custom/plugin injection, confirmed guardrail writes, exact running-GUI input, and broadcast without blocking output, parsing, copy/search, or required terminal replies. The lock follows live ownership through split/collapse/process-local moves, locked panes are excluded from broadcast, and tab/split/accessibility indicators expose state. Workspace restore deliberately unlocks. Hermetic route/ownership tests pass; native keyboard/pointer/visual acceptance is pending. | [advanced controls](advanced-workspaces.md), [focused routing](focused-pane-routing.md) |
| Legacy terminal snapshots | **Supported** | The explicitly single-terminal snapshot opens a new shell with title/cwd/last-command metadata and bounded text; split tabs capture one visibly labeled focused pane. V5 atomically publishes metadata plus embedded text, with nonblocking per-session writer locks, one recoverable staging slot, checksum-validated restore, and stale-selection rejection before shell creation. Metadata-only lists avoid decoding the payload. Limits remain 1 MiB metadata and 5 MiB decoded text; Unix opens are nonblocking/no-follow and output is private. V1–v4 remain readable; saving upgrades, without backup history or automatic downgrade. Filesystem I/O remains synchronous; old-writer interoperability and power-loss durability are not claimed. This is not process or topology restoration; complete relaunch persistence is the separate Workspace capability below. | [snapshot contract](session-snapshots.md), [#106](https://github.com/Tjemmmic/termeric/issues/106), [#107](https://github.com/Tjemmmic/termeric/issues/107), [#27](https://github.com/Tjemmmic/termeric/issues/27) |
| Transcript recording/export | **Partial** | Bounded asynchronous JSONL persistence and checkpointed background export have fake-executable PTY coverage. One export per window/four workers per process verifies a flushed byte-length/SHA-256 prefix, streams Markdown from the same staged JSONL, and publishes a completion marker only after both files succeed. Record/output/copy bounds, private Unix staging, cancellation, window-local completion, and explicit failure/no-fallback semantics have hermetic coverage. Scrollback-only export captures at most 32 MiB in memory only when no recorder is attached; its file I/O is background work. Worker-side redaction is UTF-8-safe and removes complete known-prefix spans from content and metadata string values, with local recorder/export regressions. It remains a per-string heuristic, does not sanitize existing files, and is not a secret-free export guarantee. Authenticated-provider acceptance remains consent-gated. | [background I/O](background-io.md), [export contract](transcript-export.md), [#105](https://github.com/Tjemmmic/termeric/issues/105), [integration acceptance](integration-acceptance.md), [#22](https://github.com/Tjemmmic/termeric/issues/22), [#26](https://github.com/Tjemmmic/termeric/issues/26) |
| Complete workspace persistence | **Supported** | Versioned, atomic per-window archives preserve tab order/type, pane-local shell/profile launch identity, nested pane trees and ratios, focus, cwd/title/command metadata, plugin/profile references, config references, and bounded deduplicated scrollback. Schema 0/1 archives migrate to schema 2; restore relaunches processes, partially recovers healthy components, and restores the remembered set as independent windows. | [workspace contract](workspaces.md), [#27](https://github.com/Tjemmmic/termeric/issues/27) |
| Multiple windows and launch CLI | **Supported** | One event loop owns independent window-local renderers, tabs/panes, plugins, clipboard routing, redraw/surface state, and persistence, with deterministic live-owner pane and whole-tab transfer between its native windows. The startup CLI groups windows/tabs and selects cwd, commands/argv, reusable terminal profiles, shell splits, exact plugin profiles, workspaces, titles, geometry, and Linux app identity. | [CLI contract](command-line.md), [advanced controls](advanced-workspaces.md), [#28](https://github.com/Tjemmmic/termeric/issues/28) |
| Window stacking policy | **Partial** | Ordinary windows have a typed, persisted `normal`, `always_on_top`, or `always_on_bottom` policy applied at creation, live settings changes, config reload, and reset. Quick-access remains unconditionally topmost. Configuration/action/headless tests pass, but the desktop environment owns the final stacking result and native X11/Wayland acceptance is pending. | [window policy](window-policies.md), [native evidence](native-linux-acceptance.md) |
| Quick-access terminal | **Supported (X11), partial (Wayland)** | One `--quick-access` command launches or authenticated-control toggles a single persistent frameless always-on-top terminal. Initial geometry is 80% × 45% of the primary monitor, top-centered and clamped; a private nonblocking ownership lock prevents startup races, hidden state keeps PTYs alive, and transient ownership is excluded from automatic workspace restore. X11 honors normal position/focus policy; Wayland compositors may require an app-ID rule for exact placement and activation. | [quick-access contract](quick-access.md), [CLI contract](command-line.md) |
| File/directory drag and drop | **Partial** | Local shell/profile drops insert one bounded absolute UTF-8 path as an exact POSIX-quoted word with no read or Enter. A real file/directory drop on an authenticated local/SSH live-domain pane instead requires request-scoped confirmation, revalidates source identity, streams at most 64 MiB in bounded frames to that exact pane cwd, reports bounded request-scoped progress, supports cancellation during directory preparation and pre-commit transfer with acknowledged daemon cleanup, retries up to three transient failures from a daemon-reported offset, and idempotently replays recent completion. Files atomically publish mode `0600`; deterministic directory archives cap entries/depth/path bytes, reject symlinks/special files/mount crossings/mutation, preserve empty directories and only a normalized executable distinction, and atomically publish on Linux without overwrite. The exact quoted remote path is inserted into the captured pane without Enter. Four private partials reserve at most 256 MiB (directories reserve archive plus extraction). Ordinary partials expire after 60 seconds; CLI `--retain` partials remain for up to 24 hours and can be listed, SHA-256-prefix-verified, resumed, or cancelled by another authenticated process while the daemon remains alive. Plugin/serial/broadcast/modal routes fail closed. Native X11/Wayland delivery, daemon-restart persistence, rich metadata preservation, and real-host evidence remain absent. | [drag/drop and transfer contract](file-drag-and-drop.md), [remote threat model](remote-domains.md) |
| Running-GUI control | **Partial (experimental)** | A process-scoped, same-user authenticated Unix API discovers every running GUI and exposes bounded, semantically validated read-only window/tab/pane/divider inventory with stable runtime IDs, sanitized metadata, human output, and versioned JSON. Exact text queries return at most 64 KiB of presented UTF-8 from a stable pane path, optionally include 256 primary-history rows, prefer newest content, exclude hidden synchronized output, and keep alternate screens isolated from primary history. Disabled-by-default exact input accepts at most 64 KiB of printable/tab/newline UTF-8, honors target-local bracketed paste, makes Enter explicit, permanently replays PTY failures to prevent partial-write duplication, and routes destructive plugin text through visible confirmation with its destination captured. Separate local-tab and fresh-native-window launch actions directly execute at most 128 exact arguments/16 KiB of UTF-8 argv with optional absolute cwd, initial title, and up to 64 combined bounded environment assignments/removals plus explicit inheritance clearing; they never perform implicit shell parsing and preserve replay-safe stable-ID results. Terminal identity and `PWD` remain host-owned, Unix `SHELL` removal is explicitly refused, and existing environments remain private. Clear/unset policies are child-local and one-shot, not workspace persistence. Linux `--env-stdin` accepts one 256-KiB/five-second EOF-terminated JSON object before endpoint contact, with strict merging, redacted diagnostics and pipe/file-only input; it avoids literal values in CLI argv/history but not local socket/replay/child-memory exposure. A 256-record process ledger exposes initial-child portable exit status by launch request ID through cleanup and UI ownership changes; expiry and process exit explicitly limit availability. Client-side `wait-launch` and single-invocation launch `--wait` add deadline-bound polling and portable child exit-code propagation without holding a GUI worker or canceling the child. Tab launch selects the new tab by default; protocol-v18 `--background` suppresses selection at insertion and preserves current pane interaction state, while output and completion continue. Its boolean is replay-bound; an empty manager selects its first tab. Tab launch returns the new tab ID; window launch clones one exact source configuration, initializes one local command tab while hidden, then returns both new stable IDs. Live-domain command ownership remains the separate authoritative `domain spawn` path. Other exact actions create and close ordinary or live-domain tabs/panes, create native windows, request focus/window resize, select targets, close windows, move ordinary tabs/panes across process-owned windows, move live panes between same-domain tabs with exact anchor/direction, reorder live tabs within same-domain visual slots, and resize monotonic ordinary or daemon-owned dividers. Every representable live topology mutation waits for an authoritative daemon snapshot, reconciles it, and returns resulting GUI-local stable IDs/state with pending-retry and terminal-replay safety; the last domain pane/tab remains protected and mismatched results fail. Cross-namespace ordinary moves return fresh destination-local IDs, pinned-tab and topology/capacity rules fail closed; ordinary plugin panes preserve their terminal-local session state, and surviving pane/divider identity persists through applicable topology changes. Cross-window live placement is intentionally rejected because daemon topology has no native-window ownership. Hermetic path/authentication/framing/maximum-shape/text/input/launch/environment/replay/selection/closure/move/resize tests exist; live native-GUI requests still need acceptance evidence. | [GUI control contract](gui-control.md), [CLI contract](command-line.md), [#29](https://github.com/Tjemmmic/termeric/issues/29), [#95](https://github.com/Tjemmmic/termeric/issues/95), [#97](https://github.com/Tjemmmic/termeric/issues/97), [#98](https://github.com/Tjemmmic/termeric/issues/98), [#100](https://github.com/Tjemmmic/termeric/issues/100), [#101](https://github.com/Tjemmmic/termeric/issues/101), [#102](https://github.com/Tjemmmic/termeric/issues/102), [#109](https://github.com/Tjemmmic/termeric/issues/109), [#110](https://github.com/Tjemmmic/termeric/issues/110), [#112](https://github.com/Tjemmmic/termeric/issues/112) |
| Fullscreen window view | **Partial** | F11, a configurable shortcut, and the command palette toggle borderless fullscreen for only the focused native window. The requested state is transient and has deterministic headless routing coverage. A new two-window native scenario requires reversible X11/Wayland compositor geometry, exact PTY character/pixel resizing, and sibling isolation, but it awaits a green CI artifact before promotion. | [window contract](command-line.md), [GUI acceptance](gui-acceptance.md) |
| Persistent/local/remote multiplexing | **Partial (experimental)** | A same-user authenticated daemon owns PTYs beyond GUI lifetime; CLI and GUI detach/reattach cover domains, live workspaces, nonblocking tab/pane creation and closure, authoritative title/pin/order state, focus, input, bounded text, stable nested-divider resizing, pane swap/balance/layout presets and cross-tab movement, monotonic multi-client topology convergence, stable-ID layout repair, scrollback, reconnect, and slow clients. Pane input and terminal replies use bounded nonblocking admission plus pane-local writers, so a stalled child cannot hold global daemon state. CLI controls and authority-aware GUI attachment can use a strict OpenSSH stdio bridge with no daemon-token export or same-ID local fallback. Started relay output workers clear liveness and close their shared transport on normal/error/unwind exits without taking the writer mutex; local regressions verify blocked-writer release and inert-child reaping, not real SSH acceptance. Live output reports truncated/malformed frames and transport errors rather than false success, distinguishing clean frame-boundary EOF and intentional local read cancellation with hermetic regressions. Production live reads preserve unlimited idle waits but bound each started header/payload to ten seconds and check cancellation at most every 500 ms; socket regressions cover stalls, trickles, idle output, and partial-read cancellation. A scope-owned worker set closes before joining all started threads on input EOF/errors, later spawn failure, and unwinding; partial-start/failure fixtures cover teardown without waiting for the writer mutex, while connection deadlines and blocked local stdout remain separate limitations. Pre-worker attachment replay bounds each frame read to ten seconds and the entire snapshot to five minutes; local regressions cover trickled frames, progressing fragments, late flushes, and live-output handoff. Synchronous blocked stdout is only checked once it returns. Input and resize share a guarded send transaction that propagates write/flush/poison failures and closes on error/unwind before releasing the writer lock; local fixtures verify lock ordering, unchanged wire bytes, geometry deduplication, and cleanup. Attached socket/owned-SSH-pipe writes now have ten-second header-to-flush budgets that slow progress cannot renew; local fixtures verify expiry, fresh-frame budgets, and unchanged reader flags without invoking SSH. Mutex waits, serialization, setup, stdout, and bridge forwarding deadlines remain separate limitations. The bridge now scopes/joins input ownership, closes on failure/unwind, half-closes normal input EOF to drain responses, and shares unbuffered negotiation/forwarding stdin; local fixtures cover pipelined requests and teardown without SSH. A bounded real-binary bridge/daemon fixture additionally verifies coalesced requests with closed/open stdin, exact final responses, token non-disclosure, and rejection without daemon startup in isolated directories. Post-authentication topology reads bound each initial/started frame to ten seconds without timing out idle revision waits; cancellation checks between reads and at most 500-ms polls discard partial/stale work and close the channel, with socket-only lifecycle regressions. Topology outages name the exact domain and local/SSH authority once, retry with bounded backoff, and announce recovery only after a route-matched authoritative snapshot reconciles. Bounded saved host profiles resolve existing OpenSSH destinations once and provide human/JSON inventory. A keyboard/mouse command-palette flow asynchronously discovers local or saved-host domains, validates a prepared snapshot, and opens a separate native window without blocking its source. Native remote-host GUI acceptance and native TLS remain unavailable. | [live-domain contract](live-domains.md), [remote threat model](remote-domains.md), [#29](https://github.com/Tjemmmic/termeric/issues/29) |
| Keyboard copy mode and quick-select hints | **Supported (Linux X11 acceptance)** | Pane-local vi/Emacs navigation covers scrollback, word/page/line motions, Unicode-safe character/line/block selection, search, copy, reflow, and exit. Boxed hint labels expose OSC 8 links and bounded configurable regex matches with copy/open actions. Content/viewport changes revalidate captured targets before render/input; surviving targets keep their labels and update positions, lost targets cannot transfer labels, and newly arriving targets require reopening hints. Lost partial prefixes close safely without activation. Unchanged sources use an O(1) check; changed sources rescan the viewport with a bounded target lookup. Portable unit/golden evidence is complemented by keyboard-only X11 clipboard/opener acceptance. Resize uses exact grid-bound cursor/anchor mapping through reflow, archival, eviction, and glyph repair; lost endpoints clear selections instead of clamping onto other text, and width changes retire block/physical-line ranges. An invisible live navigation anchor shares core selection movement/damage rules, including unselected navigation; render/input reconcile canonical coordinates after output, retire lost ranges, and release anchors in the original owner. Hermetic identity-oracle, damage, and single/split-pane regressions cover these newer paths. Synchronized-output navigation, selection, copy-mode search, and hints use the frozen viewport and reset stale modal coordinates at snapshot transitions; these newer paths have hermetic coverage with native acceptance deferred. | [copy-mode contract](copy-mode.md), [#30](https://github.com/Tjemmmic/termeric/issues/30) |
| Advanced tab/pane controls | **Supported (Linux)** | Palette/configurable-shortcut access covers rename/reorder/pin, view-only zoom, swap/balance/presets, launch-faithful live-PTY transfer across unlike ordinary tabs and process-owned native windows, daemon-owned live-domain cross-tab transfer, three cwd policies, and two-step mode-correct broadcast to a selectable active-tab target set with distinct armed/live indicators and immediate cancellation. Tab-owned plugin processes move only with their complete tab; ordinary splits beside them remain movable. Named, persistent, or cross-tab/window broadcast groups remain explicit boundaries. | [contract and safety](advanced-workspaces.md), [#35](https://github.com/Tjemmmic/termeric/issues/35) |

## Product integrations

| Capability | Status | Current boundary | Tracking |
|---|---|---|---|
| Themes | **Supported** | Twenty-six bundled themes, custom theme files, validation, selection, persistence, and active-file reload are implemented. | — |
| Settings, command palette, and configurable shortcuts | **Supported** | Live settings, fuzzy command palette, shortcut recording/conflict handling, and custom commands are implemented. Shared command/Unicode/domain palette query fields bound all insertion routes to 256 non-control scalars and render a caret-following whole-grapheme viewport with explicit glyph clipping. Command and Unicode palette IME candidates use that same visible caret rather than the terminal behind the modal. The separately bounded shortcut filter supports grapheme caret/Backspace, Ctrl-word navigation/deletion, and pointer caret placement through its shared render/IME viewport without disturbing chord recording; plain Delete retains its binding-clear action, while modified Delete is consumed. Filter selection remains absent, and native pointer/IME acceptance remains deferred. | [input geometry and limits](accessibility-and-input.md) |
| Git status/completion/diff workflow | **Partial** | Generation-checked background snapshots drive status and branch completion; complete GUI and external-tool acceptance remains. | [background I/O](background-io.md), [#22](https://github.com/Tjemmmic/termeric/issues/22), [#26](https://github.com/Tjemmmic/termeric/issues/26) |
| Claude Code and Codex integrations | **Experimental** | All twelve built-in commands pass a fake-executable real-PTY workflow, including argv/environment/cwd, restart routing, diff/context/transcript/export/error/profile paths. Clean `ccb234a` binds all 30 fake-provider checks to a sealed partial integration record and exact test-binary digest. Direct Context/Clear/Copy/Jump to Error additionally have raw-shell coverage for exact plugin launch owners in mixed tabs, read-only refusal, stable split/collapse identities, and sibling isolation. Profile choices capture one stable owner, revalidate configuration and writable local ownership, and change only its next-restart metadata; stale choices never retarget siblings or replacement tabs. Restart resolves the source cwd/profile, validates one matching launch and cwd, and opens a replacement before retiring only that plugin owner; configuration/spawn failures preserve existing processes and read-only/live-domain owners refuse a local restart. AI metadata, transcripts, recorder notices, and diff detectors are terminal-owned and move on split/collapse; recording controls, export sources, and delayed diff review retain exact owners instead of mixing sibling activity. Session chrome uses the focused owner. Raw transcripts preserve split UTF-8 with bounded per-recorder input/output decoding, explicit malformed-text replacement, and isolated recording epochs through stop/replacement/EOF. Passive diff capture is byte-stream framed and validates hunk counts under explicit line/patch/history memory limits; real boundary/EOF completion records each accepted diff once, and eviction cannot retarget delayed review actions. Schema-3 workspaces restore mixed plugin/shell/process-profile owners with exact saved references, per-pane cwd, targeted launch validation, and failed-tab rollback; fake-executable archive round-trips cover non-primary plugins and distinct profiles. Explicit palette commands create fresh plugin panes with direction/profile selection, exact cwd and child geometry, independent state, and failure rollback; plugin-pane movement preserves live PTY/profile/recorder/diff ownership across ordinary tabs and process-owned windows, including mid-stream state and stale-action isolation. PTY creation does not prove provider readiness. Authenticated-provider runs remain not run pending explicit operator consent; the partial record does not imply otherwise. | [integration acceptance](integration-acceptance.md), [diff capture](diff-capture.md), [#26](https://github.com/Tjemmmic/termeric/issues/26), [#125](https://github.com/Tjemmmic/termeric/issues/125), [#126](https://github.com/Tjemmmic/termeric/issues/126) |
| Harvest | **Experimental** | Disabled by default; accepted Bash/Zsh/Fish semantic regions, deterministic Nushell launch routing, opt-in Elvish module generation/routing, per-tab/pane ownership, indicators, and search/copy/rerun routing have automated evidence. Elvish 0.21.0's isolated real-PTY probe now yields the expected Harvest success/failure regions after one rc import. Nushell 0.115.1 also yields the expected isolated success/failure regions; its cwd gaps are separate limitations. Monotonic region identities and signed rows survive scrollback movement and resize reflow; evicted live text loses coordinate authority without replaying the completion. The bounded Root Cellar stores normalized command text plus at most 64 KiB of output per entry and 8 MiB across all owned snapshots, opens on the newest entry, visibly counts count/memory-cap evictions, marks clipped entries, preserves stable selection through scrolling and eviction, and shares exact render/pointer/AccessKit geometry. Copy and rerun can use retained snapshots; search and scroll require primary-screen live text and become visibly/semantically disabled with an exact eviction or alternate-screen reason when it is unavailable. Its keyboard, pointer, wheel, close, and exact-entry activation paths open a clamped, request-scoped sprout menu whose disabled and stale actions fail closed. Explain remains consent-gated: local fixtures verify routing and semantics without invoking an authenticated AI provider. Other shells are unsupported with a visible explanation. | [integration acceptance](integration-acceptance.md), [#26](https://github.com/Tjemmmic/termeric/issues/26), [#32](https://github.com/Tjemmmic/termeric/issues/32) |
| Clipboard, URL opener, bells, and notifications | **Partial** | Relative filesystem opening now requires a trusted absolute local terminal directory, including bare filenames; quick-select captures its presented context, Ctrl+click uses the clicked pane, and routed hyperlink events use their origin. Absent/remote reports do not fall back to application cwd; absolute paths and URI arguments remain unchanged. All GUI launch paths now reject missing, closed/closing, or obsolete tab/pane routes before target validation or helper launch; live inactive/read-only owners remain eligible without focus substitution. OSC 8 metadata does not generate activation events. Opener launches additionally share eight process-wide reservations: dedicated waiters retain direct children through exit, full capacity is refused before spawning, and fixture tests verify actual Unix reaping, spawn-error release, and slot reuse without desktop launches. Later helper failures are logged, and long-running handlers keep their slots; this is not a descendant-process limit or a spawn-latency deadline. See the [opener lifecycle contract](copy-mode.md). Pre-launch rejection and owner/frozen routing have hermetic evidence, with native acceptance of these changes deferred. Native X11 automation covers selection copy/paste, bounded OSC 52 write/read, link opener, visual/audio bell, and legacy OSC 9/777 notification delivery with exact PTY/spied-command evidence. Kitty OSC 99 adds strict 128-byte identifiers, interleaved plain/base64 title/body/button assembly, bounded application/type/icon-name metadata, typed urgency, expiry, and standard sound policy, explicit identified close, truthful platform-specific S7/S8 capability discovery, invalid-continuation cleanup, and explicit memory ceilings. Exact-origin `always`/`unfocused`/`invisible` filtering follows active/inactive tabs, focused and visible sibling panes, zoom-hidden panes, and native window focus/occlusion; stale origins and suppressed requests fail closed before delivery side effects. Linux additionally advertises encoded `p=icon`: PNG/JPEG/GIF input is capped at 192 KiB, decoded to one static RGBA frame no larger than 512x512 or 1 MiB, delivered as the exact freedesktop `(iiibiiay)` image-data tuple, and cached process-wide by strict `g` ID under 64-entry/16-MiB-pixel LRU rails. Ordered local theme names win before transmitted fallback, invalid replacement data preserves an older valid cache entry, and no temporary files are created. Linux uses origin-scoped freedesktop D-Bus handles for stable replacement, exact close, terminal-enforced positive expiry, one bounded ordered lifecycle-signal listener, requested `c=1` close reports, independent `a=report` activation replies, and default-on `a=focus` exact-origin tab/pane selection plus native X11/Wayland activation requests. Show/Close/Alive keys and delayed replies retain stable terminal identities across split/collapse, including nonzero survivors; removed/replaced/closed/closing owners cannot receive replies or activation, and the requesting terminal's framing is preserved; unconfirmed closes time out visibly as `untracked`. Buttons are capped at eight labels and use separate private D-Bus action keys; daemon activation tokens are opaque, 4-KiB bounded, generation-scoped, and never logged, exposed to the PTY, or reused. Standard named/silent sound hints and owner-local `p=alive` queries are supported. Ordinary notifications retain `notify-send`, while macOS retains title/body helper behavior. Audio and ordinary notification subprocesses use one bounded background launcher/reaper with a 16-request queue and eight-child ceiling; it blocks without a timer when empty, reaps between requests under queue pressure, buffers spawn acknowledgement, and retains children through sender-disconnect cleanup. Receive-boundary and local-fixture tests verify idle behavior and actual Unix reaping, not whole-app CPU savings; the tracked worker has 64-command/completion queues plus separate 64-live-handle and 64-pending-close ceilings. Immediate launch/capacity/lifecycle failures become capped, deduplicated warning toasts instead of disappearing. Real-compositor portals, permissions, sound devices, and notification presentation/icon/action/signal/sound/occasion/focus behavior remain deferred manual acceptance; the compositor has final authority over focus grants. | [integration acceptance](integration-acceptance.md), [focused routing](focused-pane-routing.md), [OSC policy](osc-integration.md), [#26](https://github.com/Tjemmmic/termeric/issues/26), [#71](https://github.com/Tjemmmic/termeric/issues/71), [#74](https://github.com/Tjemmmic/termeric/issues/74), [#75](https://github.com/Tjemmmic/termeric/issues/75), [#76](https://github.com/Tjemmmic/termeric/issues/76), [#77](https://github.com/Tjemmmic/termeric/issues/77), [#78](https://github.com/Tjemmmic/termeric/issues/78), [#79](https://github.com/Tjemmmic/termeric/issues/79), [#80](https://github.com/Tjemmmic/termeric/issues/80), [#84](https://github.com/Tjemmmic/termeric/issues/84) |
| Runtime third-party plugins | **Supported** | Declarative APIs v1-v32 provide reviewed local profiles, bounded lifecycle and command events, correlated persistent handlers, current-window inventory, stable-ID navigation/topology, typed non-secret settings, presentation-safe terminal reads, ordinary pane/tab creation and closure, cross-tab pane movement, guarded exact-pane input, same-tab layout operations, command-scoped exact-window closure, separately granted process-wide read-only inventory, exact non-activating cross-window tab/pane navigation, bounded cross-window presentation-safe terminal reads, guarded cross-window UTF-8 terminal input, exact cross-window ordinary local-tab/pane creation, exact cross-window ordinary-pane and ordinary-tab movement/same-window reorder, exact cross-window ordinary pane/tab closure, exact cross-window ordinary pane swap/balance/layout presets, result-gated closure of one exact non-final process-owned window, and truthful native focus requests for one exact live process-owned window. Every capability is versioned and independently granted; host operations are single-action, bounded, revalidated at execution, and non-chainable from mutation results. API v20 identifies the exact originating stable window, preserves process creation order, excludes closing windows, and caps aggregate topology/metadata. APIs v21-v32 may retain only their separately granted process-navigation, process-terminal-text, process-terminal-input, process-tab-create, process-pane-create, process-pane-move, process-tab-move, process-pane-close, process-tab-close, process-pane-layout, process-window-close, and process-window-focus authorities after that query; every follow-up result removes all host authority. Cross-window input additionally requires the destination window's disabled-by-default external-input policy and preserves exact-target destructive confirmation. Native focus requests share authenticated GUI control policy: already-focused targets are no-ops, hidden/minimized targets and Wayland fail explicitly, and success does not claim compositor confirmation. The in-app manager supports reviewed local installation with explicit grants and manifest-drift rejection, inspection, reversible default-profile selection, enable/disable, typed settings, confirmed reset/removal, and permission-aware update review. Updates display exact current/candidate identity, fingerprints, capability deltas, and affected default-profile state; grants intersect candidate requests, enabling requires all requested permissions, and commit rejects source or registry drift. Replacement installs remain CLI-only. Registered launch-profile plugins also expose explicit user-invoked pane commands with direction/profile selection, host cwd policy, and the existing reviewed launch configuration; this does not grant structured handlers new mutation authority. Plugin-requested final process-window termination, Wayland application-driven focus, focus-time window restoration, live-domain pane layouts, raw output/raw-byte input, arbitrary layout trees, broader destructive actions, OS-sandboxed runtimes, remote installation, marketplace/signing, and render hooks remain unsupported. | [plugin contract](plugins.md), [#34](https://github.com/Tjemmmic/termeric/issues/34), [#36](https://github.com/Tjemmmic/termeric/issues/36), [#37](https://github.com/Tjemmmic/termeric/issues/37), [#38](https://github.com/Tjemmmic/termeric/issues/38), [#39](https://github.com/Tjemmmic/termeric/issues/39), [#40](https://github.com/Tjemmmic/termeric/issues/40), [#41](https://github.com/Tjemmmic/termeric/issues/41), [#42](https://github.com/Tjemmmic/termeric/issues/42), [#43](https://github.com/Tjemmmic/termeric/issues/43), [#44](https://github.com/Tjemmmic/termeric/issues/44), [#47](https://github.com/Tjemmmic/termeric/issues/47), [#48](https://github.com/Tjemmmic/termeric/issues/48), [#49](https://github.com/Tjemmmic/termeric/issues/49), [#50](https://github.com/Tjemmmic/termeric/issues/50), [#51](https://github.com/Tjemmmic/termeric/issues/51), [#52](https://github.com/Tjemmmic/termeric/issues/52), [#53](https://github.com/Tjemmmic/termeric/issues/53), [#54](https://github.com/Tjemmmic/termeric/issues/54), [#55](https://github.com/Tjemmmic/termeric/issues/55), [#56](https://github.com/Tjemmmic/termeric/issues/56), [#57](https://github.com/Tjemmmic/termeric/issues/57), [#58](https://github.com/Tjemmmic/termeric/issues/58), [#59](https://github.com/Tjemmmic/termeric/issues/59), [#60](https://github.com/Tjemmmic/termeric/issues/60), [#61](https://github.com/Tjemmmic/termeric/issues/61), [#62](https://github.com/Tjemmmic/termeric/issues/62) |

Harvest remains Experimental. Its [exact-source rerun contract](integration-acceptance.md#exact-source-harvest-rerun)
now has raw-PTY regression evidence for broadcast isolation, literal delivery,
read-only/closed/closing refusal and source-bound destructive confirmation
([#114](https://github.com/Tjemmmic/termeric/issues/114)). This fixes a known
fan-out bug without claiming native UI or authenticated-provider acceptance.
Its separate 4-KiB presented-command snapshot now preserves whitespace and
longer complete commands independently of the 512-character display caption.
Both strings count toward the existing 8-MiB owned snapshot budget. Live and
retained truncation disable rerun with execution-time revalidation; partial
copy/Explain disclose incompleteness
([#115](https://github.com/Tjemmmic/termeric/issues/115)). This does not reconstruct
original shell input from terminal presentation.

Harvest source keys now always include the stable tab-local pane ID, preserving
recording, live actions and exact rerun/confirmation routing across split,
collapse and re-split, including nonzero survivors. Removed or read-only sources
cannot be replaced by the current sibling. Cross-tab/window history migration
is not included, and native acceptance remains deferred
([#117](https://github.com/Tjemmmic/termeric/issues/117)).

Guarded-input confirmations now require stable tab/pane identity for every
producer, not just Harvest. Local raw-capture tests cover typed commits, paste,
GUI/plugin/process input, and broadcast refusal/framing through split/collapse,
source removal, read-only changes, cancellation, and replay. Native/provider
acceptance remains separate ([#118](https://github.com/Tjemmmic/termeric/issues/118)).

Unicode/context picker targets now retain stable tab-and-pane identity across
split/collapse; replacement owners cannot inherit pending selections or context.
Context routing additionally verifies the actual pane's plugin launch identity
and prefers its working directory, not a focused shell sibling's. Local fixtures
cover exact bytes, refusal, cancellation and replay; native/provider acceptance
remains deferred ([#119](https://github.com/Tjemmmic/termeric/issues/119)).

Named Unicode-picker queries, including cold index construction, now run off the
UI thread with one in-flight scan and one replaceable latest query. Edits retire
old actions immediately, and generation checks prevent stale completion across
close/reopen. Exact scalar and curated entries remain synchronous. Automated
blocked-worker, idle-scheduling, and accessible-status tests cover this lifecycle;
native responsiveness acceptance remains deferred (see [Unicode policy](unicode-support.md)).

Custom-command Name and Command fields now support whole-grapheme caret/deletion
and Ctrl word editing, with bounded clipped/shaped caret-following text and
matching IME candidate geometry. Automated model, geometry, modifier, and raw-PTY
isolation tests pass; native visual/IME acceptance remains deferred. Configured
clipboard paste inserts atomically at the active form caret under the existing
256-byte Name and 16 KiB Command limits, rejects multiline/control/oversized
input without terminal fallback, and never submits the form. Keyboard text and
IME commits share that atomic insertion path; semantic replacement shares its
single-line/control/size validation and clears stale composition only on
acceptance. Tests cover both field limits, caret/focus preservation on rejection,
and raw-PTY isolation without saving or executing commands. Hermetic clipboard
routing tests pass; real-desktop clipboard acceptance is separate. Primary field
clicks now position the caret at visible whole-grapheme boundaries using the same
scrolled/animated geometry as painting and IME. Geometry, model, and raw-PTY
tests pass; native pointer acceptance and text selection remain outstanding
(see [input contract](accessibility-and-input.md)).

Workspace rename now accepts the configured Paste shortcut at its active
caret under the existing atomic 128-scalar/512-byte single-line limit. Invalid
or oversized input preserves the draft, caret, and composition; paste never
submits or saves a rename. Higher modal owners, the Workspaces menu, and browse
mode retain clipboard guards. Hermetic binding, ownership, Unicode-limit, and
raw-PTY tests pass without a real clipboard or persisted rename. Primary clicks
place the rename caret at visible whole-grapheme boundaries using the shared
scrolled/animated viewport; collapsed fields and invalid model positions cannot
move a hidden or interior-cluster caret. Geometry/model and raw-PTY insertion
tests pass; native pointer/clipboard/IME acceptance and text selection remain
deferred (see [input contract](accessibility-and-input.md)).

Inline tab rename keyboard/IME input now rejects invalid or oversized events
atomically at the existing local/live-domain title limits, deletes whole
graphemes with Backspace, and consumes unsupported modified edits without
saving/cancelling. Captured-tab preview and Escape restoration retain exact
ownership. Local model and raw-PTY tests cover both limits and ordinary-tab
commit/cancel without a live-domain connection. Configured clipboard paste now
appends atomically under those limits, preserves draft/preview/composition on
rejection, and never commits or queues a live-domain request. Hermetic shortcut,
modal-ownership, limit, preview-cancellation, and raw-PTY checks pass. The inline
editor still lacks caret navigation and selection; native clipboard/IME
acceptance remains deferred (see [input contract](accessibility-and-input.md)).

Shortcut, Settings, workspace, and command panels consistently precede retained
tab rename for keyboard, IME/candidate geometry, and paste. Their read-only
states consume input, and command-manager browse keys no longer fall through to
the shell or retained editors. Headless and raw-PTY tests cover frontmost input,
blocked transitions, draft preservation, and rename resumption after dismissal;
native desktop acceptance remains separate (see [input contract](accessibility-and-input.md)).

Custom-command lists now scroll by wheel/trackpad through a shared whole-row
viewport, keeping painting, hover, and Run/Delete targets aligned at absolute
command indices. Resize/form/config changes clamp the effective range, reopening
resets it, and scrolling preserves drafts and composition. Accessible list
scrolling and exact-identity focus reveal are covered alongside geometry/model
and raw-PTY isolation tests, without executing or persisting commands. Native
trackpad/screen-reader acceptance remains deferred. Browse controls now support
Tab/Shift+Tab and arrow cycling, Home/End, Enter/Space activation, and Escape
close, with shared exact-identity keyboard/assistive focus and visible-row
reveal. Stale or hidden targets cannot activate a replacement/invisible command;
Deletion now synchronizes pointer/keyboard/assistive focus on New in browse mode
or preserves the active form field, and retires published row IDs before a
shifted duplicate can receive stale Run/Delete/Focus actions. An isolated Linux
subprocess regression exercises deletion and persistence only under a temporary
configuration root, including the final row and retained drafts.
Validated configuration reload also retires changed or duplicate-count-ambiguous
row identities before queued actions, while keeping unchanged tuples, focus,
and form drafts stable across reordering. Headless/model tests cover replacement,
stale actions, no-op reload, and invalid-config preservation without executing
or saving commands. Model,
modifier, geometry, semantic-transition,
and raw-PTY tests cover navigation isolation and exact Run bytes delivered to an
inert reader, without executing a shell command or provider. Native keyboard,
focus-outline, and screen-reader acceptance remain deferred (see
[input contract](accessibility-and-input.md)).

Explicit custom-command Run and RunCommand keybindings no longer expand the
selected payload into another saved command or rewrite it with a cached Git
branch suggestion. Resolved command chunks skip interactive suggestion creation
and acceptance; ordinary typed aliases and Git suggestions retain their normal
behavior. Temporary libgit2 repositories and inert raw-PTY tests verify literal
Run/keybinding bytes, typed completion, and exact delivered bytes.
The shared terminal-input policy retains read-only and armed/active broadcast
behavior. Raw-PTY collision and safeguard tests use
inert byte readers, not real command execution or providers; native operator
acceptance remains deferred (see [input contract](accessibility-and-input.md)).

Typed, pasted, completed, broadcast, and Harvest re-run input is sent exactly
as the person entered it. Only automated input, from the GUI control protocol
and runtime plugins, waits for the destructive-command confirmation.

Tab and Enter share Git-ref validation and literal Bash/Zsh/Fish quoting
chosen from the focused pane's enabled integration metadata. Unknown dialects
refuse names needing quotes; complex input stays with native shell completion.
Partial erasure counts Unicode scalars, not bytes. Inert PTY routing/refusal and
isolated POSIX argument round trips pass; nested-shell detection, native
Fish/Zsh/operator acceptance, and grapheme-aware editing remain unverified or
outside this helper (see [input contract](accessibility-and-input.md)).

Git completion navigation now yields modified/AltGraph keys to the normal
shortcut and terminal routes after dismissing the popup. Ctrl+Tab retains tab
navigation, read-only panes do not swallow application shortcuts, and modified
Enter cannot accept a branch through legacy CR encoding. Broadcast and stale-list
ownership have model coverage; inert PTYs verify shortcut routing and exact
input bytes. Native keyboard acceptance remains deferred (see
[input contract](accessibility-and-input.md)).

Git completion collection now shares a 16,384-name / 4 MiB UTF-8 text budget
across local and remote branches, charged before copying and deduplication.
Overflow discards the partial suggestion set, retires the open popup, preserves
Git status and typed input, and warns once to use shell completion. In-budget
refreshes restore suggestions on subsequent input. These bounds do not cap
libgit2 internals or whole-process RSS (see [background I/O](background-io.md)).

Completion redraws now share immutable suggestion storage and cached name-length
metadata instead of deep-copying the list twice and rescanning it for popup
width each frame. Storage/lifetime regressions cover full-budget lists,
replacement, and Unicode scalar sizing; native performance gains are not yet
measured (see [background I/O](background-io.md)).

The Git popup now includes header/footer rows in its height, keeps the selection
visible, and fits complete rows within the available content viewport. Labels
are bounded and clipped; views too small for one item retire completion instead
of consuming keys for an invisible popup. Geometry/text regressions pass; native
visual acceptance remains deferred (see [input contract](accessibility-and-input.md)).

Split-pane completion placement now projects the uniquely focused pane's
rendered cursor through its actual grid origin, including fractional coordinates
and updated frame geometry. Missing or ambiguous focus does not anchor to a
sibling. Render-data tests cover focus/resize/zoom/content-origin changes,
single-grid collapse, and edge fitting; native split-pane visual acceptance
remains deferred (see [input contract](accessibility-and-input.md)).

Guarded submission cancellation preserves the already-sent editable prefix on
the unchanged focused route, so retrying Enter rechecks it. Withheld paste and
replacement bytes are excluded; safe editing and exactly-once broadcast retry
have inert raw-PTY coverage. This does not preserve shell state across focus,
history/cursor edits, or read-only resets, and is not a complete shell parser
(see [input contract](accessibility-and-input.md)).

Shortcut-editor filter typing, native character events, IME commits, and
semantic replacement now enforce the same 128-scalar/512-byte single-line limit.
Complete Unicode input events are retained, oversized commits are rejected
atomically, and Backspace removes whole graphemes. Modifier/recording guards and
raw-PTY tests cover ownership without persisting bindings; native IME/visual
acceptance remains deferred (see [input contract](accessibility-and-input.md)).
Shortcut filter text and its placeholder now use shaped, clipped,
whole-grapheme scrolling; the native IME candidate follows the same visible
caret without rebuilding results. Recording suppresses the filter caret and
preedit, including over retained Settings. Geometry/priority and opt-in
GPU-backed glyph-clipping checks cover the path, not real-desktop IME or visual
acceptance.

Shared palette query editing supports unmodified Left/Right/Home/End and
whole-grapheme Backspace/Delete with insertion at the moved caret. Ctrl+Left/Right
and Ctrl+Backspace/Delete use the same Unicode word-segment editing as search;
unsupported modified edits and AltGraph are consumed without changing text.
Local dispatch and blocked-worker regressions cover these bindings, but
real-desktop Ctrl word-edit acceptance is deferred. Primary query clicks place the caret at
the nearest visible whole-grapheme boundary using shared animated/scrolled
geometry; busy domain filters stay read-only. Geometry and model/worker tests
cover pointer placement; native domain/search pointer and IME acceptance is deferred.
Cursor-only
edits preserve selection and pending Unicode searches, and busy domain stages
remain read-only. Geometry, model, app routing, IME insertion, and semantic caret
tests cover this behavior. The private-Sway interaction lane additionally
requires seventeen native query-editing cases (seven unmodified, six
Ctrl word-edit, and four command/Unicode pointer cases) with exact, fenced Unicode
insertion into alternating local PTY owners. Schema-5 clean-candidate GUI records
independently reconstruct these cases from the PTY transcript and seal it plus
the schema-3 sidecar; an offline checker rejects changed artifacts, historical
seven/thirteen-case sidecars, and older unsealed records. This is a palette-specific
evidence check, not a validator for every GUI scenario. Native IME, visual clipping,
screen-reader, and real-desktop acceptance remain deferred
(see [input contract](accessibility-and-input.md)).
Historical clean local `2f7399d` passed the private-Sway lane and independent
thirteen-case reconstruction, providing native command/Unicode word-edit
delivery evidence without promoting domain/search, pointer, or native IME
acceptance (see [native GUI evidence](gui-acceptance.md)).
Clean local `e19272f` subsequently passed all seventeen cases under private
Sway with AMD/radeonsi OpenGL, including the four pointer cases; independent
transcript reconstruction and artifact seals passed. Native command/Unicode
query pointer delivery is therefore verified within that fixture, while
domain/search pointer, native IME, and visual acceptance remain deferred.

Active terminal search shares whole-grapheme horizontal scrolling between its
painted query/caret and IME candidate/preedit geometry, including rail/chrome
offsets and scaled cells. Geometry and raw-PTY routing tests pass, and the
software-GPU smoke checks shaped query clipping. Search also supports
whole-grapheme Left/Right and Backspace/Delete, plus Home/End; caret-only edits
preserve scan progress and selected results. Ctrl+Left/Right and
Ctrl+Backspace/Delete add Unicode word-segment movement/deletion, skip adjacent
whitespace, and preserve whole graphemes. Selection and other modified editing
keys remain consumed without editing. Model and raw-PTY tests
cover query edits and IME insertion without child-input leakage. This does not
claim native keyboard, IME, or visual acceptance
(see [input contract](accessibility-and-input.md)).

Search-query primary clicks now use the same whole-grapheme pointer mapping as
palettes. Clicking a confirmed query resumes input without resetting matches,
selection, scope/filter, or pending scan progress; blank bar space consumes the
press instead of reaching terminal selection. Shared-geometry, model, and
raw-PTY tests cover caret placement and resumed IME insertion. Native pointer/IME
acceptance and drag selection are not claimed.

The configurable Paste shortcut now reaches command, Unicode, domain,
shortcut-filter, Settings-filter, and terminal-search query fields at their byte caret. The
shortcut filter retains its 128-scalar bound and refuses paste while recording
a chord, without reading the clipboard or changing bindings. Query pastes reject controls,
multiline text, and over-limit input atomically; higher-priority or read-only
surfaces cannot redirect them to a background query or shell. Local routing and
raw-PTY isolation tests pass. Native clipboard acceptance for these fields is
still deferred (see [input contract](accessibility-and-input.md)).

Shortcut-filter accessibility now advertises the same recording ownership as
keyboard input: the field is read-only while capturing a chord, stale filter
edit/focus requests are rejected, and focus cannot fall back to that field if
the selected binding scrolls away. Cancel, timeout, and capture restore editing.
Accepted semantic replacement clears preedit without changing bindings; inactive
owners refuse replacement. Model/action/tree regressions cover this boundary;
native screen-reader acceptance remains deferred.

Settings-filter keyboard text and IME commits now use atomic insertion under a
256-scalar/1,024-byte ceiling, reject controls and line separators, and preserve
complete combining/ZWJ events. Backspace/Delete remove whole graphemes;
Home/End and Ctrl+Left/Right navigate the filter, and Ctrl+Backspace/Delete
remove adjacent Unicode words. Plain Left/Right still adjust setting values;
unsupported modified editing keys cannot fall through to those actions. Caret
moves preserve result selection and scroll. Dropdowns
refuse filter mutation, and AltGraph text cannot trigger the Ctrl+S/Ctrl+R
actions. Model/dispatch/raw-PTY checks cover this boundary without changing
configuration. Settings-filter paste uses the same atomic limits, refuses
clipboard reads while a dropdown is open, and never applies a setting or falls
through to a retained field or PTY. The private-Wayland interaction lane covers
eleven clipboard, Unicode, pointer, and keyboard-editing cases with exact PTY
ownership and reversible font-size effects. Clean candidate publication now
requires independent reconstruction of those byte fences and native geometry,
agreement with bounded per-case font/focus log ranges, and seals over all three
artifacts; old unsealed Settings reports are not equivalent evidence (see
[native acceptance contract](gui-acceptance.md#wayland-settings-filter-interaction)).
Selection and real-desktop
clipboard/IME/visual acceptance for this filter remain separate work.

Settings-filter accessibility replacement is now wired through the same bounded
single-line policy, including empty-query clearing and UTF-8 caret reporting.
Successful semantic edits clear preedit and refresh results without applying
configuration. Dropdowns expose a read-only filter and reject retained edit/focus
requests; closed or covered Settings cannot receive background edits. Model and
in-process action/tree regressions cover this boundary, not native screen-reader
acceptance.

The Settings filter now shapes and clips a caret-following whole-grapheme slice;
its renderer, accessibility bounds, and IME candidate/preedit position use the
same scaled/animated input geometry. Dropdowns and collapsed fields expose no
editable candidate area. Pure geometry/ownership and software-GPU glyph-clipping
checks cover this boundary, not whole-panel or native IME acceptance.

Settings-filter pointer placement uses that shared visible-grapheme viewport
and revalidates active/dropdown state before changing the caret. It preserves
result selection/scroll and never applies configuration; dropdown dismissal
does not also place a caret. Geometry, action, paste/IME, and raw-PTY regressions
cover the path, including private-Wayland pointer input; real-desktop visual
acceptance and drag selection remain open.

Built-in plugin profile selection distinguishes **Automatic profile** from every
explicit name, including `default`. Selection/restart prefer a matching owner in
the active tab; delayed choices retain the original identity. Exact references
survive OpenTab, pane movement, restart, and workspace round-trips. These paths
have local fake-executable coverage only, not authenticated-provider acceptance;
see [profile behavior and limits](plugin-panes.md).

## Performance and resource evidence

Search/palette query viewport calculation and pointer mapping now use
allocation-free grapheme iteration instead of collecting a full-query index.
Differential geometry tests and an isolated allocator regression pass, including
zero allocation requests across 100 evaluations of each long-query fixture.
This measures temporary allocation traffic, not whole-app RSS or native latency
(see [performance evidence](performance.md)).
The paired release-only CPU diagnostic also exposed and drove a fix for repeated
segmentation of long combining clusters. Boundary/prefix reuse retains exact
geometry and zero-allocation coverage; three local processes show lower median
batch times than the indexed reference across all five query fixtures. Those
microbenchmarks are diagnostic, not native-latency or portable-budget evidence.

Shaped-run retention now has independent 8 MiB persistent and 1 MiB frame-local
retry budgets, plus a 4,096-entry ceiling on each. Accounting covers owned
key/string/feature/glyph capacities; bucket overhead and transient/font-system
memory are separate. Over-budget runs still render without caching. Independent
capacity recounts and software-GPU bypass/retry checks cover these boundaries;
this is not a total RSS or throughput claim. See [shaped-run budgets](glyph-atlas.md).

Glyph rasterization no longer keeps an unbounded duplicate CPU bitmap map
behind the bounded GPU atlas. A deterministic 282-key fixture verifies identical
pixels and placement with zero retained CPU image payload (343,899 bytes in
the old cached path); software-GPU stress covers normal/deferred hits, pressure,
resize, and color glyph retention. Scaler/font/shaping memory is separate, and
evicted glyphs may need rerasterizing. This does not claim a measured process
RSS or throughput improvement; see [raster lifetime](glyph-atlas.md).

Clean `5a5e79d` now has an independently validated schema-5 600-second local core
soak plus offscreen Vulkan/llvmpipe smoke: all 28 checks passed, with 50.5 MiB
peak RSS, 712 KiB quartile RSS growth, zero thread/descriptor growth, and complete
child/transcript cleanup. This is one exact-candidate local record, not hosted
CI history or displayed/vendor-GPU/multi-day acceptance. See
[the measured workload and limitations](resource-soak.md#evidence-and-limitations).

Clean `e446dfd` separately passes the schema-6 600-second displayed Sway/GL
software-renderer soak and independent validation: 950 samples, 236.9 MiB peak
RSS, 3.6 MiB quartile RSS growth, zero thread/descriptor growth, 237 resize and
237 pane-churn actions, and complete child cleanup. All 24 checks passed;
DRM memory was explicitly unavailable in all samples, not measured as zero.
This is local software-renderer duration evidence, not a low-memory ranking,
vendor-GPU measurement, hosted history, or real-desktop acceptance.

The schema-5 core resource soak includes sustained diff-history count and
owned-capacity pressure, exact completion, eviction reconciliation, and
independently recounted retained allocations. Older core records do not prove
this workload; core results alone do not establish displayed stability, and
neither current local lane proves vendor-GPU stability. See
[resource-soak boundaries](resource-soak.md).

| Area | Status | Current boundary | Tracking |
|---|---|---|---|
| Performance measurement | **Partial** | A versioned, fail-closed 73-metric contract covers core, renderer, PTY, terminal and Unicode-picker search, shaped-run byte-pressure cost/retention, passive streamed-diff processing/fragmentation/retained history, tab/pane scaling, 10k/100k history, allocation traffic, displayed RSS/PSS/private memory, idle CPU/frame submissions, and native key-to-present latency; missing/duplicate/unit-drifted/malformed/context-drifted rows fail, and unavailable optional probes remain explicit skips. The five-name-query Unicode workload retains a required 250 ms maximum per batch in the serial release lane; parallel correctness tests retain result/storage checks without wall-clock assertions. Cross-report comparisons suppress incompatible machine, unit, workload, and adapter rows rather than reporting false percentages. A separate opt-in X11 runner gives Termeric, Alacritty, kitty, WezTerm, Ghostty, and Konsole identical child bytes and isolated requested settings, accounts terminal process trees, and permits ranking startup, PTY response, RSS, or idle CPU only when that metric's grid/retention/pixel contract matches. Clean source `5e7d4b9` has two calibrated Termeric/kitty software-Xwayland runs with matched 100×30 grids and 901×601 windows, permitting all four metric comparisons. Lower local Termeric RSS/startup and faster kitty PTY response are recorded alongside Termeric's nonzero post-workload CPU tail; earlier unmatched or dirty-tree results remain historical. Separate core and displayed resource-soak runners record process growth. Hardware-backed and broader-terminal comparisons, post-workload CPU attribution, compositor scanout latency, portable vendor utilization, and release-duration history remain open. | [benchmark method](performance.md), [soak method](resource-soak.md), [#18](https://github.com/Tjemmmic/termeric/issues/18), [#23](https://github.com/Tjemmmic/termeric/issues/23) |
| Scrollback memory | **Partial** | Compact primary-screen style-run history is bounded by both lines and one configurable per-terminal byte budget; alternate screens are viewport-only and cannot retain a second budget. The 100k styled fixture owns 108.0 MiB and measured 107.6 MiB p95 process growth. Release-duration allocator/RSS evidence across supported Linux environments remains open. | [memory design and evidence](scrollback-memory.md), [#19](https://github.com/Tjemmmic/termeric/issues/19) |
| Idle rendering | **Partial** | PTY threads wake a reason-traced dirty-state scheduler; unchanged visible windows sleep between exact presentation/maintenance deadlines, while occluded or surface-lost windows suppress blink/UI/toast/graphics cadence without starving PTY drains or safety timeouts. Real Wayland and X11 probes measured 0.0% CPU in all five idle samples on the acceptance host. A current private-Xvfb artifact additionally records zero rendered frames in every idle window, proving unchanged state submits no application frames; portable vendor-utilization observation remains unmeasured. | [design and evidence](event-driven-rendering.md), [#20](https://github.com/Tjemmmic/termeric/issues/20) |
| Incremental rendering/GPU buffer reuse | **Partial** | Unchanged terminal rows reuse persistent geometry and capacity-managed GPU buffers; cell/search/URL changes invalidate affected rows while rasterization and global layout changes clear the cache. Pane clip bounds are signed independently so sub-cell divider movement cannot reuse stale clipping. Search intervals and position-sorted URL intervals avoid per-cell linear match scans, including mapping-only wide-cell URL changes. Steady offscreen benchmarks allocate no new buffers and a one-row change rebuilds exactly one row; native visible-frame and vendor-driver profiling remain open. | [design and evidence](incremental-rendering.md), [#21](https://github.com/Tjemmmic/termeric/issues/21) |
| Background Git/transcript I/O | **Partial** | A generation-checked Git worker publishes immutable status/branch snapshots; cwd changes cancel stale work while slow same-cwd scans survive poll deadlines and coalesce one overdue follow-up. Transcript persistence uses a 512-entry/2 MiB bounded queue, 250 ms batching, in-memory cap accounting, one-shot degradation notices, and a bounded durable shutdown wait. Entry-owned byte reservations release on dequeue, rejected sends, and unread-queue cleanup after failure or panic without erasing live producer charges. Clean writers block without timer polling; only a pending batch has a deadline, and further accepted or cap-rejected traffic cannot postpone its flush. Shutdown drains accepted commands in order and retains one completion result across retries; timeouts, sink cleanup, final-text loss, writer failures, and panics cannot become false success merely because the sender or notices were consumed. Long-duration real-repository and real-filesystem soak evidence remains open. | [design and evidence](background-io.md), [#22](https://github.com/Tjemmmic/termeric/issues/22) |
| Long-running resource stability | **Partial** | Schema-5 core and schema-6 displayed records seal logs/artifacts and independently reconstruct resource checks, including privacy-safe descriptor/thread/child inventories and optional process-attributed DRM memory. A pinned Linux executable descriptor binds each launch to measured bytes; source state before build/setup, at each launch, and after execution plus final pathname/pinned-object checks prevent observed checkout/rebuild drift from being promoted as candidate evidence. Older schemas are historical, not equivalent evidence. The runners retain failed diagnostics, and pure/fake runner-path tests cover drift, cleanup, schema tampering, and false promotion without a GUI. Historical `5eeded9` core and `a53d097` displayed measurements retain their original pass/failure boundaries. Current-schema local ten-minute core and software-displayed results pass. The separate clean `bd70faa` AMD/radeonsi displayed run passes all 24 checks with 877/877 required DRM samples, 110.2 MiB peak resident GPU memory, 5.0 MiB quartile growth, and complete child cleanup. The separate `946eb65` release-binary refresh passes all 24 checks with 954/954 required DRM samples, 147.3 MiB peak RSS / 1,008 KiB growth, 110.2 MiB peak GPU resident memory / 4 KiB growth, +1/+3 thread/fd quartile growth aligned with the two observed pane-count states, and complete cleanup of 120 children; explicit font/backend differences preclude ranking it against older runs. Clean scheduled-CI history, real-desktop observation, broader adapter/backend coverage, and longer durations remain open; this is not signed compiler provenance or a multi-day stability claim. | [soak method and evidence](resource-soak.md), [#23](https://github.com/Tjemmmic/termeric/issues/23), [#81](https://github.com/Tjemmmic/termeric/issues/81), [#82](https://github.com/Tjemmmic/termeric/issues/82), [#83](https://github.com/Tjemmmic/termeric/issues/83), [#90](https://github.com/Tjemmmic/termeric/issues/90), [#108](https://github.com/Tjemmmic/termeric/issues/108) |

Cross-terminal comparison schema 2 now refuses partial or zero-defaulted resource
totals. Every discovered process must be readable with a stable identity; idle
CPU additionally requires unchanged membership/identity and monotonic counters
across both observations. Missing resource samples remain explicit and suppress
only the affected metric's statistics/ranking, while valid timing rows remain
eligible under their existing contracts. Known identities remain tracked for
cleanup after metric-read failures. Synthetic failure/launch-path tests and a
live self-process read cover the new contract. The clean `49b3fab` private
Sway/Xwayland Termeric/kitty run additionally retained complete observations in
all ten measured samples, while correctly refusing resource ranking for unequal
native window sizes. It does not promote hardware performance or platform
acceptance (see [local comparison evidence](performance.md#current-local-comparison-evidence)).

Resource ranking additionally requires valid positive integer window dimensions
in every sample. Matching absent/malformed geometry is no longer treated as a
match; query failures and duplicate dimension fields remain explicit, blocking
RSS/idle ranking without discarding valid timing rows or process observations.
Re-evaluation preserves all five recorded native reports' existing contracts;
calibration must be validated separately rather than promoting mismatched runs.

Two clean `d6d5261` software-Xwayland follow-ups retain all twenty measured
Termeric/kitty samples after the multicell-repair optimization. Termeric's PTY
medians decreased relative to the earlier record, but overlapping samples,
non-interleaved old/new builds, and shifted kitty timing prevent a causal
end-to-end percentage claim. The paired core diagnostic measures the narrower
optimization; resource ranking remains disallowed by unequal window sizes.
See [native follow-up evidence](performance.md#native-follow-up-at-d6d5261).

The subsequent row-local presence cache at `4081caa` has a paired core-replay
improvement plus two clean native Termeric/kitty follow-ups. Termeric's native
PTY medians were 48.10 and 50.51 ms; all ten samples were below the preceding
build's recorded range. This supports lower response times in that local
software-rendered workload, not a portable causal percentage, a memory ranking,
or an overall terminal winner. kitty remained faster on the PTY workload;
these settings do not permit resource ranking, and broader hardware/platform
evidence remains open.
See [row-cache native evidence](performance.md#native-follow-up-at-4081caa).

Explicit Termeric font-size/line-height benchmark controls now support a locally
verified matched-grid/window recipe. Two clean `5e7d4b9` runs measured median
process-tree RSS of 185.42/185.60 MiB for Termeric and 256.31/258.29 MiB for kitty.
This is a limited software-rendered profile, not a general memory-efficiency
claim or a memory optimization against earlier differently sized runs. Termeric
also had one 11–12%-of-one-core post-workload CPU observation in each run; zero
medians do not erase those bursts, whose attribution remains open. Failed and
successful calibration records and both full runs are preserved in
[the calibrated evidence](performance.md#calibrated-resource-comparison-at-5e7d4b9).

A clean `c9dadca` thread/frame diagnostic did not reproduce those large CPU
bursts across 25 measured Termeric processes; this is not a fix or permission
to discard the earlier observations. It also found the requested comparison
font absent: Termeric resolved to Noto Sans Mono, while kitty's actual face
was not captured. Existing geometry/resource contracts pass but do not verify
same-font equivalence, so the local figures above must not be promoted as
fully controlled font-equivalent rankings. Font-resolution controls and CPU
attribution remain open; see
[the diagnostic evidence](performance.md#post-workload-cpu-diagnostic-at-c9dadca).

The schema-3 comparison runner now records each isolated sample's installed
font-family inventory check and Fontconfig-resolved face file/index/fingerprint.
Missing, substituted, unreadable, malformed, or inconsistent font observations
block every ranking; raw measurements remain available. Schema-1/2 history
does not satisfy this new gate. A passing Fontconfig control still explicitly
leaves actual terminal-renderer face equivalence unverified, rather than
claiming the probes observe renderer selection. The probes run before timing
and may warm font caches, so new startup results are not directly comparable
with the older methodology.

Clean `cd801ad` native evidence verifies both sides of that gate: the absent
DejaVu request completed its workloads but blocked every ranking, while two
installed-Noto runs passed the recorded controls with matched grids/windows
and a common face fingerprint. Their median RSS observations were
183.46/183.57 MiB for Termeric and 253.20/257.99 MiB for kitty. The larger
Termeric post-workload CPU activity recurred (up to roughly 10% of one core),
and timing outliers remain retained. This is not an application optimization
or verified renderer-font-equivalent ranking; see
[the schema-3 native evidence](performance.md#native-font-control-verification-at-cd801ad).

Clean `f9ad5d2` diagnostics reproduced matching CPU bursts and attributed most
of their scheduled CPU time to llvmpipe software-rendering workers. Frame
counters captured 17–22 extra frames over a short post-response tail. Separate
4 KiB output runs consistently logged 25–27 animation frames; an otherwise
matching reduced-motion control removed that tail. Source inspection found
legacy frame-delta cursor interpolation, so replacing it with a fixed-duration
`Instant` transition is the concrete next fix. This is attribution evidence,
not an implemented optimization, proof about every historical burst, or a
steady-idle/hardware-GPU claim; see
[the CPU-tail evidence](performance.md#cpu-tail-attribution-at-f9ad5d2).

Cursor interpolation now uses a fixed 150 ms `Instant`-based ease-out
transition, with deterministic retargeting and lifecycle tests. Hidden cursor
movement snaps, and the scheduler no longer chases live-grid cursor positions
behind synchronized-output snapshots. This corrects animation timing; it does
not establish a universal CPU reduction, especially where the old
frame-dependent implementation accidentally snapped immediately. See
[the transition contract](event-driven-rendering.md) and
[the implementation boundary](performance.md#bounded-cursor-transition-implementation).

Clean `8f3154f` native follow-up with animations enabled reduced the observed
4 KiB post-response tail from 25–27 extra frames over 395–429 ms to nine frames
over 135–142 ms. The 1 MiB case now consistently animates too, so it consumes
more post-response CPU than the old traced cases that accidentally snapped.
Both outcomes are retained as a predictable-duration correction and its cost,
not an across-the-board optimization; see
[the native cursor evidence](performance.md#native-cursor-follow-up-at-8f3154f).

Clean `2edd584` now has a preserved provider-free release benchmark refresh:
67 measured metrics pass their producer contracts and all 40 budgets with
measurements pass. Six displayed-window metric rows remain explicitly skipped;
three associated budget decisions are skip-allowed, not measured successes.
The offscreen adapter was software llvmpipe/Vulkan. This refresh does not prove
native acceptance, a hardware performance ranking, or a before/after speedup;
see the [raw report and measurement limits](performance.md#headless-budget-refresh-at-2edd584).

## Release gates

Termeric may be tagged as a Linux beta only when all of these are true on the
exact release candidate:

1. Every **Broken** default-path capability above is fixed, disabled, or
   explicitly removed from the UI/configuration.
2. The VT support manifest and deterministic conformance/fuzz smoke from
   [#16](https://github.com/Tjemmmic/termeric/issues/16) pass.
3. The candidate-bound [hermetic release preflight](release-preflight.md),
   headless GPU smoke, and the end-to-end Linux acceptance suite from
   [#17](https://github.com/Tjemmmic/termeric/issues/17) pass.
4. Performance, memory, idle-resource, and soak budgets from
   [#18](https://github.com/Tjemmmic/termeric/issues/18) through
   [#23](https://github.com/Tjemmmic/termeric/issues/23) have current results
   and no unresolved release-blocking regression.
5. Terminfo/capability claims are truthful and verified as defined by
   [#25](https://github.com/Tjemmmic/termeric/issues/25).
6. Bash/Zsh/Fish/Nushell/Elvish, desktop integrations, and consent-based authenticated
   plugin checks in [#26](https://github.com/Tjemmmic/termeric/issues/26)
   either pass or are accepted as named beta limitations.
7. Fresh Debian-family and Arch install/upgrade/uninstall checks pass with
   package dependencies populated.
8. Every remaining **Partial**, **Experimental**, **Unverified**, or
   **Unsupported** item is described accurately in user-facing documentation.
9. Required manual evidence records the candidate SHA, date, environment, tool
   versions, disposition, and artifact path; an unavailable prerequisite is
   not treated as a pass.

Experimental features may remain in a beta only when disabled by default and
when enabling them exposes their limitations. Unsupported future features do
not block a beta unless the product currently advertises or exposes them as
working.

## Maintaining this matrix

Every pull request that changes a user-visible capability must:

1. Update the relevant row and its exact working boundary.
2. Add or link repeatable evidence; parser-only coverage must be identified as
   parser-only.
3. Update README or release notes if their summary changed.
4. Record candidate SHA, date, platform, and tool versions for manual evidence
   in [release-acceptance.md](release-acceptance.md).
5. Avoid changing a status to **Supported** until the full user path is tested.
