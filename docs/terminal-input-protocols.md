# Terminal-controlled input

Termeric routes mouse reports, native-window focus reports, and bracketed paste
from the focused terminal's mode snapshot to that same focused PTY. UI chrome
and input-blocking surfaces are hit-tested first. These guarantees apply in
both single-terminal and split-pane tabs.

## Mouse reporting

The application implements the legacy xterm mouse families below.

| DEC private mode | Behavior |
|---|---|
| `9` | X10 button presses |
| `1000` | Button press and release |
| `1002` | Press, release, and motion while a button is held |
| `1003` | Press, release, and all pointer motion |
| `1005` | UTF-8 coordinates through coordinate 2015 |
| `1006` | SGR decimal coordinates and explicit release reports |
| `1007` | Vertical wheel steps become cursor keys on the alternate screen |

Reports include left, middle, and right buttons; vertical and horizontal wheel
directions; release and drag state; and Shift, Alt, and Control modifier bits.
Grid coordinates are one-based on the wire and are calculated from physical
cursor and renderer geometry relative to the focused pane. Scrollback offsets
never leak into protocol coordinates. Values outside the legacy encoding's
safe range are rejected rather than wrapped or truncated; SGR coordinates are
not artificially capped.

Hold **Shift** while pressing or scrolling to bypass application mouse
reporting and use Termeric's native selection or scrollback behavior. Alt still
selects a rectangular block while using this bypass. A release that belongs to
an already-reported press remains application-owned so changing modifiers
mid-drag cannot leave the application button state stuck.

An application-owned press captures its stable tab/pane identity. Focus-surface
refresh, drag motion, and release revalidate that owner; switching terminals or
closing the source cancels the capture instead of sending held-button motion or
a release to another application. Returning focus does not revive the capture.
Cancellation does not synthesize a release into an unfocused or closed PTY.
Ordinary DECSET 1003 hover reports still follow the focused terminal, and a
valid captured release still works after modifier changes or modal takeover.

Ctrl+click on a detected or semantic hyperlink remains Termeric's native link
activation gesture. Ctrl is included in application reports at other cells.

## Focus reporting

When the focused terminal enables DECSET 1004, a real native-window transition
sends `CSI I` for focus-in or `CSI O` for focus-out. Duplicate platform
notifications are ignored. Tab or pane switches are not fabricated as window
focus changes, and no report is emitted before a focused live PTY exists.

## Bracketed paste

When the focused terminal enables DECSET 2004, one paste operation is written
as one ordered byte vector:

```text
CSI 200 ~ + clipboard UTF-8 + CSI 201 ~
```

Without that mode, only the clipboard UTF-8 bytes are written. The snapshot,
framing decision, selection cleanup, and PTY destination all refer to the same
focused terminal. Termeric does not rewrite or sanitize pasted content.

Native local file and directory drops reuse this exact focused-terminal paste
route after separate path validation and POSIX quoting. Confirmed live-domain
uploads capture the exact remote pane, then ask the daemon to apply that pane's
current paste mode only after atomic publication. Neither path
adds Enter; broadcast, plugin, and serial boundaries remain unavailable. See
the [drag/drop and transfer contract](file-drag-and-drop.md).

## Evidence and limits

Unit tests assert byte-exact reports for every implemented tracking and
encoding family, coordinate boundaries, modifiers, focus, paste, and alternate
scroll. A real raw-mode shell PTY test captures one combined stream in a
focused split pane and proves ordering and duplicate-focus suppression.
Additional raw-PTY captures verify tab/pane-switch isolation at the actual
mouse-report write paths, release-only transitions, closed-source cancellation,
no revival after focus returns, modifier changes, and ordinary hover reports.
The private-Sway interaction lane also exercises six native held-button cases:
ordinary drags, a focus switch before release, and a focus switch before motion,
each across local tabs and panes. `interaction/mouse-ownership.json` records the
exact source/destination PIDs and parsed SGR reports. A persistent virtual
keyboard and a no-focus-loss assertion prevent accidental cancellation from
masking the bug; post-release input fences and per-PTY stream assembly prevent
split reads from hiding leaked reports. The pre-fix `5f64bdb` binary fails the
replacement-tab release check while the fixed build passes. X11 held-button
switches, live domains, and real-desktop event policy remain separate acceptance.

Native X11 automation now drives mouse, paste, Vim, less, and tmux through the
real application window; the private Wayland interaction lane adds keyboard,
clipboard/paste, pane routing, and the ownership checks above. See
[`gui-acceptance.md`](gui-acceptance.md). Real-compositor policy and subjective
interaction quality remain manual.
