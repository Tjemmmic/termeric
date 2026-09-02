# Tab-bar placement

Termeric supports a horizontal top tab strip and a vertical left tab rail.
Placement is a window preference rather than workspace state, so every open
window follows the live configuration while saved workspace topology remains
portable.

```toml
[window]
tab_bar_position = "top" # top or left
```

The same setting is available under **Settings → Window → Tab Bar Position**.
Changing it, reloading the configuration, or resetting settings immediately
recalculates every local PTY and split-pane pixel geometry; running processes
are resized rather than restarted.

## Left-rail contract

Left placement keeps Commands, Workspaces, and Settings in the existing top
toolbar. The tab strip begins below that toolbar and provides the same tab body,
close button, activity/diff/plugin/read-only/broadcast state, drag reorder,
keyboard focus, and new-tab action as top placement.

The rail prefers 220 logical pixels and scales with the native display scale.
It uses a 160-logical-pixel minimum when the window can preserve it, retains at
least 160 logical pixels for terminal content on ordinary window sizes, and
splits exceptionally narrow viewports evenly instead of allowing the rail to
consume the entire window.

When all tabs do not fit, bounded previous/next controls reserve fixed rows and
the viewport automatically follows the active tab or F6 keyboard focus. Hidden
tabs remain reachable without shrinking labels into unusable slivers.

## Shared geometry

Rail width is part of the authoritative terminal geometry. The same physical
offset is applied to:

- PTY columns and pixel width;
- single terminals, nested panes, dividers, scrollbars, images, and Harvest
  indicators/menus;
- selection, mouse protocols, pane focus, divider dragging, and tab dragging;
- search, Git completion, plugin chrome, IME candidate/preedit placement, and
  AccessKit bounds.

Render, pointer hit-test, keyboard-focus, and accessibility tab bounds all use
the same visible-slot layout. Native X11/Wayland visual, drag, IME, and screen-
reader acceptance for the left rail remains an operator release check.
