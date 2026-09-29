# Kitty multiple-cursor support

Termeric implements the [Kitty multiple cursors protocol](https://sw.kovidgoyal.net/kitty/multiple-cursors-protocol/) as a bounded, terminal-local presentation feature. The implementation is tracked by [#91](https://github.com/Tjemmmic/termeric/issues/91).

## Implemented profile

- `CSI > SP q` advertises shapes `1`, `2`, `3`, and `29`, color operations `30` and `40`, and queries `100` and `101`.
- Coordinate type `0` addresses the main cursor, type `2` accepts point pairs, and type `4` accepts inclusive rectangles or the full viewport when no coordinates follow it.
- Shape `0` clears selected cells. Invalid shapes, malformed colors, incomplete coordinate tails, zero points, and off-screen points are ignored without changing unrelated state. Rectangles are clipped to the viewport.
- Extra cursors can be block, beam, underline, or follow the effective main cursor shape. They share the main cursor's opacity and blink phase but remain visible when the application hides the main cursor.
- Unset, reverse-video special, sRGB, and indexed cursor/text colors are supported. Dynamic OSC palette entries participate in indexed lookup. Special cursor color takes precedence over special text color as required by the protocol.
- Queries are FIFO and use the terminal's current S7C1T/S8C1T reply framing. Cursor reports use deterministic horizontal rectangle runs.

Extra cursor positions are viewport state, not scrollback content. `ED 2`, `ED 3`, `ED 22`, DECSTR, RIS, and either transition between the main and alternate screen remove them. IND and RI leave their screen coordinates unchanged. Resize preserves only the old/new viewport intersection.

Synchronized output freezes extra cursors and their colors at the same boundary as the visible grid. Split panes render each terminal's own state, shape, and blink phase. Local-domain attachment replays colors and a complete horizontal-run map using ordinary protocol sequences when it fits the existing bounded terminal snapshot. If a deliberately fragmented map cannot fit that snapshot, the relay restores the color pair and omits all cursor positions rather than presenting a partial, misleading map.

## Resource model

An unused terminal allocates no cursor map. The first extra cursor lazily allocates one byte per viewport cell, capped at a 1000-by-1000 viewport and released when the map becomes empty. `TerminalMemoryUsage::extra_cursor_bytes` reports both live state and any synchronized-output presentation snapshot. Protocol parsing performs no allocation proportional to untrusted coordinate values.

## Automated evidence

The default provider-free test lane covers:

- arbitrary input fragmentation and preservation of colon parameter groups;
- byte-exact support, cursor, and color queries under S7 and S8 framing;
- points, rectangles, clipping, malformed tails, overwrite, and clearing;
- all color forms and reverse-video precedence;
- reset, clear, alternate-screen, scroll, resize, and synchronized-output lifecycle;
- follow-main shape and renderer color resolution;
- pane-local blink scheduling and local-domain snapshot replay;
- lazy allocation, explicit memory accounting, geometry caps, compact replay, and all-or-nothing replay under the snapshot byte ceiling.

These tests do not claim subjective native presentation quality.

## Deferred native acceptance

On a candidate build, use a shell that supports `printf` escapes and leave enough visible text around the selected cells:

```bash
printf '\e[>29;2:4:5 q'
printf '\e[>1;2:7:1 q\e[>2;2:7:3 q\e[>3;2:7:5;2:7:7 q'
printf '\e[>40;2:255:128:0 q\e[>30;1 q'
```

Confirm that the cursors appear immediately, blink together, retain distinct shapes, render correctly over narrow and wide text, survive main-cursor hiding, and stay clipped inside their pane. Then clear the state:

```bash
printf '\e[>0;4 q\e[>30;0 q\e[>40;0 q'
```

Record the candidate commit, compositor, GPU/backend, scale factor, font, commands, and observations. Native visual acceptance can be completed later; it is not required to continue provider-free implementation work.

