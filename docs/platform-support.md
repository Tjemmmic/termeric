# Platform support policy

Termeric's release target is Linux. Platform claims are evidence tiers, not an
assumption that a successful cross-platform compile establishes a working
terminal.

## Current tiers

| Platform | Tier | Automated evidence | Required before promotion |
|---|---|---|---|
| Linux Wayland | Developer-preview release target | Workspace/core gates, headless Vulkan, resource soaks, private Weston native launch/first-render/PTY/close acceptance, and clean local headless-Sway keyboard/copy/paste/pane-routing, two-window fullscreen/PTY-resize isolation, renderer recovery, plus a displayed-process runner with optional process-attributed DRM memory evidence | Real-compositor input, scale, visual quality, desktop policy, an accepted vendor-GPU memory artifact, clean scheduled history, and closure or acceptance of remaining correctness blockers |
| Linux X11 | Developer-preview release target | Workspace/core gates plus recorded Xvfb/twm native input, resize, tabs/panes, clipboard/link, Vim/less/tmux, surface recovery, visual-signature, and close acceptance; the revised Openbox lane and its two-window fullscreen/resize scenario await their first green artifact | Real-compositor visual quality, desktop policy, vendor GPU behavior, and closure or acceptance of remaining correctness blockers |
| macOS | Build/unit plus bounded PTY CI | Stable Rust workspace build/tests and a shell-independent native PTY output/input/resize/exit/cleanup smoke are configured on `macos-latest` | Green CI history for the configured PTY lane plus native keyboard/IME, clipboard, window, GPU, scale, lifecycle, install, and soak acceptance |
| Windows | Build/unit plus bounded ConPTY CI | Stable Rust workspace build/tests and a shell-independent native ConPTY output/input/resize/exit/cleanup smoke are configured on `windows-latest` | Green CI history for the configured ConPTY lane plus native keyboard/IME, clipboard, window, GPU, scale, lifecycle, install, and soak acceptance |

macOS and Windows remain **Unverified** in the feature matrix until their CI
jobs actually pass and the broader runtime rows above are exercised. A compile
failure or native-PTY smoke failure on either runner is a portability defect,
but a compile plus bounded PTY success is not a full runtime-support claim.

## Release checklist classification

Every candidate checklist item must have one of these dispositions:

| Disposition | Meaning | Current examples |
|---|---|---|
| Automated | Repeatable and required by CI or a documented candidate command | Workspace format/test/build/Clippy, conformance/fuzz, Linux GPU and native GUI lanes, benchmark budgets, resource soaks, Debian archive and fresh-runner lifecycle checks |
| Manual | Requires a person, real desktop policy, visual judgment, hardware, or authenticated account | Real-compositor font/image quality, clipboard/URL/notifications policy, scale factors, real-prior-release upgrade checks, authenticated Claude/Codex workflows |
| Unavailable | Required environment or executable is absent; record the exact gap rather than reporting a pass | Zsh/Fish/tmux on hosts where not installed, vendor GPU tools on unsupported hardware, a missing native platform runner |
| Accepted limitation | The behavior is explicitly documented and approved for this release; it is not silently treated as working | Disabled experimental features or named partial/unsupported behavior that does not violate a default-path release gate |

An **Unavailable** result is never equivalent to success. A release owner must
either obtain the environment, defer the claim, or record it as an accepted
limitation when the release policy permits that choice.

## CI and artifact boundaries

Pushes and pull requests run Linux quality/package, GPU, native X11/Wayland,
performance, parser-fuzz, and 30-second resource-soak lanes plus macOS/Windows
workspace build/unit tests and platform-native PTY lifecycle smoke. The Linux
package lane also installs a synthetic predecessor, upgrades it to the
candidate, and removes it on the disposable runner without launching the GUI.
Weekly and manual runs add longer fuzzing and a ten-minute soak. Native GUI
logs, PTY transcripts, diagnostic captures, fixture
files, and the focused renderer PNG are uploaded; see
[gui-acceptance.md](gui-acceptance.md). CI still cannot establish subjective
quality or real desktop policy, and macOS/Windows remain below runtime status.
Real Linux candidate observations are normalized by the
[native evidence record](native-linux-acceptance.md), which binds every named
manual disposition to an exact candidate, environment, and hashed artifact.

## Promotion rules

Promoting a platform requires all of the following on the candidate line:

1. Green build and unit-test history on the native platform.
2. Native PTY launch, I/O, resize, exit, and child cleanup.
3. Native keyboard, modifiers, composition/IME, F6 chrome focus, mouse, focus, clipboard, and
   paste behavior appropriate to the supported feature subset.
4. Window creation, resize, scale changes, minimize/restore, close, GPU adapter
   failure, and device/surface recovery.
5. Install, upgrade, uninstall, configuration-path, shell-discovery, and font
   fallback checks.
6. A platform-native resource soak with recorded ceilings and artifacts.
7. Updated [feature-support.md](feature-support.md) wording that names the exact
   boundary; “Supported” still requires end-to-end evidence for the capability.

Until then, the project should say “source portability” or “build-and-unit CI,”
not “cross-platform terminal support.”
