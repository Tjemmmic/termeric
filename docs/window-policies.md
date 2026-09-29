# Window stacking policy

Ordinary Termeric windows can request one of three desktop stacking levels:

```toml
[window]
level = "normal" # normal, always_on_top, or always_on_bottom
```

The same policy is available as **Window Level** in Settings. A change is applied to the current native window immediately and is included when the configuration is saved. Startup, manual reload, hot reload, and reset all use the same typed application path. New process-owned windows inherit the active configuration.

`termeric --quick-access` is deliberately separate: its transient drop-down window always requests `always_on_top` even if the ordinary-window setting is normal or bottom. Changing the ordinary preference while quick access is open updates the saved policy but does not demote that special window.

## Platform boundary

Termeric passes the selected level to Winit at creation and on live changes. The desktop compositor or window manager remains authoritative and may ignore, constrain, or reinterpret stacking hints. This is especially relevant to Wayland, where clients cannot impose global desktop policy. The feature remains Partial until candidate-bound native X11 and Wayland evidence confirms the observable result on supported desktop environments.

The setting does not imply focus stealing, keyboard grabs, automatic hiding, workspace pinning, or persistence of a compositor-specific desktop assignment. Those are separate policies and are not currently claimed.

## Hermetic evidence

Unit tests cover TOML round trips, exact enum-to-native mapping, Settings action mapping, and headless application state. These tests do not launch a desktop or exercise any AI/provider integration. Native stacking behavior is left to the manual release acceptance workflow.
