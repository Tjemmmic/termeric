# Terminal background images

Termeric can place one optional static local image behind the terminal content
of each native window. The feature is disabled by default and cannot be
controlled by terminal escape sequences, plugins, live domains, or remote
hosts.

## Configuration

Add the nested window policy to `~/.config/termeric/config.toml`:

```toml
[window.background_image]
path = "/absolute/path/to/background.png"
opacity = 0.20
fit = "cover"
```

Relative paths are resolved beside `config.toml`. PNG and JPEG are the only
accepted formats. Animated PNG is rejected rather than silently displaying one
frame. The path is configured in TOML; Settings shows the active path read-only
and applies **Background Image Opacity** and **Background Image Fit** live.

`fit` is typed and accepts:

- `cover`: preserve aspect ratio, fill the viewport, and crop centrally;
- `contain`: preserve aspect ratio and show the complete image with the theme
  background visible around it;
- `stretch`: fill the viewport without preserving aspect ratio;
- `center`: retain decoded pixel size, center the image, and clip overflow.

Opacity is independent of `[window].opacity` and must be between `0.0` and
`1.0`. A value of zero keeps the decoded GPU texture available but paints
nothing. Removing `path` disables the feature and releases that texture.

## Viewport and layering

The image occupies the terminal content viewport below the top toolbar,
plugin chrome, and search bar, and to the right of a configured vertical tab
rail. It spans the whole content area behind all splits rather than being
duplicated per pane. Explicit terminal cell backgrounds, selection/search
highlights, inline terminal graphics, text, cursors, and application chrome are
painted above it. An OSC 11 application background therefore remains
authoritative for that terminal viewport.

Opening search or plugin chrome changes the content viewport and recomputes the
selected fit deterministically. Resizing and switching between top and left
tabs use the same physical-pixel geometry path.

## Resource and failure policy

Backgrounds are deliberately bounded per native window:

| Resource | Limit |
|---|---:|
| Encoded local file | 16 MiB |
| Decoded RGBA pixels | 64 MiB |
| Width or height | 8,192 pixels |
| Configured path | 4,096 bytes |
| Concurrent decoder workers | 1 |

Decoding runs off the event-loop thread. Changes observed during an active load
coalesce to the newest generation, and stale results cannot replace a newer
configuration. After upload, the steady-state CPU RGBA copy is released; the
dedicated GPU texture remains until the feature is disabled, replaced, or the
renderer is recreated. A renderer recreation performs a fresh bounded decode.

The configured file is watched by the existing 750 ms configuration poll.
Replacement reloads asynchronously. A missing, malformed, animated,
unsupported, oversized, or unreadable replacement produces a bounded warning
and retains the last known good GPU image. No path is read merely because it
appears in terminal output.

## Evidence and remaining acceptance

Hermetic tests cover typed configuration and Settings mapping, all four fit
geometries, shifted content origins, static PNG decode, format rejection,
animation-control detection, and encoded-size rejection. The existing opt-in
headless GPU lane compiles and submits the shared image shader/pipeline without
opening a native window; native presentation remains a separate gate.

Before release promotion, manually verify PNG and JPEG backgrounds on X11 and
Wayland with top and left tabs, search, plugin chrome, split panes, resize,
opacity extremes, each fit, file replacement, malformed replacement, and
renderer recovery. Review text contrast with representative themes. Tiling,
per-pane/per-tab images, animated backgrounds, remote URLs, automatic image
selection, and a native file picker are intentionally unsupported.
