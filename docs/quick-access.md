# Quick-access terminal contract

`termeric --quick-access` is a toggle-or-launch command intended for a desktop global shortcut. The first invocation starts one dedicated terminal window; later invocations discover that exact window through Termeric's authenticated same-user GUI control plane and toggle it without creating another shell or terminal process.

```bash
# Bind this exact command to a desktop shortcut such as F12.
termeric --quick-access

# Launch selection applies when the quick terminal does not exist yet.
termeric --quick-access --terminal-profile dev.rust
termeric --quick-access --working-directory ~/src
termeric --quick-access --geometry 1400x620
```

Quick access is deliberately a single-window launch mode. Combining it with a second `--new-window`, or repeating `--quick-access`, fails before connecting to the display. Tab, split, cwd, command, terminal-profile, plugin-profile, and workspace launch selection otherwise use the ordinary validated CLI contract. Those options affect only first creation; once the owner exists, the command is a pure visibility toggle.

## Native window behavior

Without explicit geometry, the initial window uses 80% of the primary monitor's physical width and 45% of its physical height. It is horizontally centered at the monitor's top edge, undecorated, always-on-top, and identified as `termeric-quick-access` on both X11 and Wayland. Explicit physical-pixel geometry is clamped to the selected monitor. If the platform publishes no monitor before window creation, Termeric falls back to normal configured size while retaining the quick-access identity and window level.

If the window is visible and focused, another invocation hides it. If it is hidden or another application is focused, another invocation shows it, clears minimization, requests native focus and urgent attention, and schedules a render. Terminal tabs, panes, PTYs, scrollback, and current working state remain alive while hidden. There is no automatic focus-loss hiding, so ordinary window switching cannot unexpectedly conceal work.

Hiding explicitly ends active tab/divider/scrollbar/selection gestures and clears application mouse capture, even if the compositor sends no focus-loss or button-release event. Native occlusion notifications use the same cleanup. Showing the window requires a fresh press to start another gesture; terminal state and already-completed selection remain intact. Live-domain previews use normal control finalization and authoritative rejection/rollback rather than being silently discarded. Unit/local-PTY fixtures cover this lifecycle; desktop event-ordering acceptance remains separate.

Window managers remain authoritative. X11 accepts the initial position and focus request under normal policy. Wayland may ignore absolute placement and deny activation of an existing surface; use a compositor rule keyed by `termeric-quick-access` when exact top-edge placement or scratchpad activation is required. Termeric consumes a launcher-provided startup/activation token for initial creation and clears it before any PTY child is started.

## Ownership, discovery, and persistence

A user-private `quick-access.lock` uses a nonblocking kernel advisory lock held for the GUI process lifetime. The descriptor is `O_NOFOLLOW`/`O_CLOEXEC`, its file and directory are owner-validated and private, and the kernel releases the lock after crashes. Keeping one stable inode avoids PID-reuse and unlink races. An invocation that arrives during startup fails visibly and can be retried; it never races a second owner into existence.

GUI control protocol v17 and JSON inventory schema 3 mark each window with `visible` and `quick_access`. The launcher validates every discovered process inventory before acting. Multiple owners, truncated inventory, unsafe runtime paths, authentication failures, stale targets, or non-quick target IDs fail closed. A request ID identifies the exact visibility response. Changes are expressed as an idempotent desired state rather than a raw toggle. The client retries that same request ID and state once after an ambiguous transport failure, so a lost response cannot invert the window twice.

The dedicated window does not consume automatic workspace restore, create an automatic archive, or write itself into the automatic multi-window restore set. Explicit **Save Workspace As...** remains available. Closing performs normal PTY teardown. This keeps a transient drop-down terminal from replacing or duplicating daily workspace recovery.

## Verification boundary

Hermetic tests cover CLI exclusivity, monitor-relative geometry and clamping, visibility-state decisions, private single-owner locking and symlink refusal, versioned request/response framing, and an authenticated Unix-socket discovery plus toggle round trip. Actual placement, focus/attention policy, always-on-top behavior, and desktop global-shortcut configuration remain native X11/Wayland acceptance tasks because the compositor/window manager owns those results.
