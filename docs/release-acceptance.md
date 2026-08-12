# Linux beta acceptance

This file records release evidence separately from unit-test evidence. A beta
tag is permitted only when every required row is either passed on the release
candidate or accepted as an explicit beta limitation.

## Candidate and environment

- Candidate branch/commit at start: `improvements` / `8276753`
- Date: 2026-08-12
- Host session: Wayland with an Xwayland display available
- GPU: AMD Radeon RX 7900 XTX (RADV NAVI31), Vulkan
- Shell available and exercised by the app: Bash
- Fonts discovered by the app: 5 monospace families; system includes Hack,
  Liberation Mono, FreeMono, Adwaita Mono, and Noto emoji coverage
- External CLIs detected: Claude Code 2.1.228; Codex CLI 0.137.0

## Automated and agent-driven evidence

| Area | Result | Evidence |
|---|---|---|
| Wayland launch | Pass | Isolated 8-second release launch created a window, selected the discrete Vulkan GPU, loaded Noto Sans Mono, initialized a 98x32 grid, and spawned Bash without an error log. |
| X11 launch | Pass | Equivalent isolated release launch passed against `DISPLAY=:1`. |
| X11 input/resize smoke | Pass, non-visual | Window-targeted input sent Unicode/combining text, ANSI color and 5,000 output lines; opened a second tab; invoked search/settings/font shortcuts; switched tabs; and resized from 1100x700 to 900x600. No application error was logged. |
| Normal X11 close | Pass | Window-manager Alt+F4 produced `Close requested`, unloaded plugins, and exited with status 0. |
| Abrupt X11 drawable loss | Pass | Acceptance initially found an XGetGeometry/WGPU panic and redraw storm. The candidate now pauses redraws after one surface-loss notice and waits for a real resize; forced drawable destruction no longer panics or spins. |
| Headless GPU render | Pass | Vulkan WGPU test selected an adapter and submitted a render pass with `TERMERIC_GPU_SMOKE=1`. |
| Shell semantics | Partial | Real Bash PTY/OSC 133/Harvest fixture passes. Zsh and Fish binaries are not installed on this host. |
| Protocol/core behavior | Pass (automated) | ANSI, OSC 8, OSC 133, Unicode width/selection, reflow, alt-screen behavior, Kitty parsing, search, panes, sessions, and plugin quick-exit paths are covered by the workspace suite. |
| Plugin executable contracts | Partial | Installed Codex parses open and `resume --last` vectors without authentication; Claude/Codex authenticated interactive GUI workflows were not executed. |
| Packaging | Pass with CI-only dependency check | Clean-index crate archives and Debian archive build; binary, license, desktop file, and icon were inspected. This Arch host lacks `dpkg-shlibdeps`; CI installs `dpkg-dev` and rejects an empty dependency field. |

## Performance baseline and budgets

Initial beta regression budgets are intentionally conservative:

- GUI startup with a short-lived `/bin/true` shell: median under 0.5 seconds.
- 100,000 styled terminal lines / 4 MiB input: under 3 seconds.
- 200 repeated large-history reflows: under 3 seconds.

Measured on the environment above:

| Workload | Measurement |
|---|---:|
| Wayland startup, 5 samples | median 0.110s; 0.099–0.115s |
| X11 startup, 5 samples | median 0.098s; 0.094–0.102s |
| 100,000 styled lines / 4.0 MiB | 0.356s final gate run |
| 200 reflows after 10,000 seeded lines | 2.129s final gate run |

Long-running memory/PTY soak behavior is not yet measured.

## Required manual rows still open

These cannot be truthfully inferred from synthetic input or source inspection:

- Visual inspection on native Wayland and X11 for focus, scale-factor changes,
  surface recovery, resize quality, wide/combining/emoji glyphs, ligatures,
  selection shapes, image placement, and modal hit targets.
- Full-screen `vim`, `less`, and `tmux` interaction (tmux is absent here), plus
  mouse reporting and alternate-screen restoration.
- Clipboard round trips, URL launching, visual/audio bell, and desktop
  notifications under the user desktop's permission/policy environment.
- Zsh and Fish editing and generated shell hooks on hosts where they are
  installed.
- Settings/theme persistence and every tab/pane/search/session workflow under
  human observation, including drag interactions and malformed/cap UX.
- Authenticated Claude and Codex open/resume/restart/context/diff/transcript/
  profile flows; these may consume network/service state and require the
  release operator's accounts.
- A long-running session and memory/large-scrollback soak.
- Install, upgrade, and uninstall on a fresh Debian-family machine with the
  generated package dependency field verified by `dpkg-shlibdeps`.

## Release decision

**Not ready to merge or tag.** Automated gates are strong and both display
backends launch, but the required visual, shell-matrix, authenticated-plugin,
fresh-machine package, and soak rows remain open. Keep `improvements` as the
release-candidate line. Once those rows pass or are explicitly accepted as
beta limitations, fast-forward `main`, wait for green CI on `main`, then create
an annotated `v0.1.0-beta.1` tag from that exact commit.
