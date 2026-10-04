# Troubleshooting

This page covers the problems you're most likely to hit and how to fix them. If yours isn't here, [collect a log](#collect-logs) first: Termeric usually says exactly what went wrong.

> **Tip.** If Termeric is already running, launching it again usually opens a new window in the running copy, and environment variables you set for that launch (such as `RUST_LOG`, `WGPU_BACKEND`, or `TERMERIC_TERM`) have no effect there. When a fix below asks you to start Termeric with one of them, add `--separate-process` so it starts a copy of its own.

## Termeric won't start or shows nothing: renderer problems

Termeric draws with your GPU through Vulkan or OpenGL. At startup it tries the GPU first and, if that fails, falls back to a software renderer (normally Mesa's llvmpipe) on its own, logging a warning because software rendering is slower and uses more power. If both fail, Termeric exits and prints every path it tried.

To fix a renderer problem:

1. Make sure current graphics drivers are installed. On Debian and Ubuntu, install `mesa-vulkan-drivers`; on Arch, `mesa` and `vulkan-icd-loader`. Running `vulkaninfo --summary` (from `vulkan-tools`) shows whether Vulkan can see your GPU or llvmpipe.
2. Force the software renderer to confirm the GPU driver is at fault:

   ```bash
   termeric --renderer software
   ```

3. If Vulkan itself is the problem, have Termeric use OpenGL, optionally with Mesa's software OpenGL driver:

   ```bash
   WGPU_BACKEND=gl termeric --renderer software
   WGPU_BACKEND=gl LIBGL_ALWAYS_SOFTWARE=true termeric --renderer software
   ```

`--renderer` accepts `auto` (the default: GPU, then software), `gpu` (GPU only), or `software` (software only). `gpu` and `software` never switch to the other kind. To make a choice permanent, set it in `config.toml`; the `--renderer` option overrides it for one launch:

```toml
[renderer]
backend = "software"
```

`WGPU_BACKEND` chooses the graphics API independently: `vulkan` or `gl`.

If your graphics driver resets or the GPU is lost while Termeric runs, Termeric rebuilds its renderer and keeps your tabs and shells running. If this happens repeatedly within a short time, it closes the window with an error instead of looping.

## Wayland or X11

Termeric uses Wayland when `WAYLAND_DISPLAY` is set and X11 otherwise. To run it under X11 (through XWayland) on a Wayland desktop, for example to tell whether a problem is Wayland-specific, clear that variable:

```bash
env -u WAYLAND_DISPLAY termeric
```

If Termeric fails to start on Wayland, check that the Wayland and xkbcommon libraries are installed (`libwayland-client0` and `libxkbcommon0` on Debian and Ubuntu, `wayland` and `libxkbcommon` on Arch).

If the window shows a generic icon or isn't grouped under Termeric in your taskbar on Wayland, the desktop entry isn't installed. See [Installation](installation.md#why-the-desktop-entry-and-terminfo-matter).

## Missing or wrong glyphs

Boxes or blanks instead of characters mean none of your fonts has them. Termeric finds fonts through fontconfig.

- Install fonts that cover what you need, for example Noto Sans Mono, Noto Color Emoji for emoji, and Noto Sans CJK for Chinese, Japanese, and Korean.
- Pick your main font on the **Font** page in Settings, and add fallback fonts there or in `config.toml`. Termeric tries them in order for each character before asking the system:

  ```toml
  [font]
  family = "JetBrains Mono"
  fallback = ["Noto Sans Mono", "Noto Color Emoji"]
  ```

- `fc-list :spacing=mono family` lists the monospace fonts fontconfig can see.

See [Fonts and text](fonts-and-text.md) for more.

## Copy and paste don't work

Termeric uses the Wayland or X11 clipboard of your desktop session. On Wayland it uses the compositor's data-control protocol when the compositor offers it, and otherwise the X11 clipboard through XWayland, so on a compositor without data-control, XWayland must be running. If copying fails:

- Check that you're using **Ctrl+Shift+C** and **Ctrl+Shift+V**; plain Ctrl+C sends an interrupt to the program in the terminal, as in any terminal.
- If a program in the terminal uses the mouse, hold **Shift** while selecting.
- Try running Termeric under X11 (see above) to see whether the problem is Wayland-specific.
- Programs in the terminal can write to the clipboard with OSC 52 escape sequences, but can't read it unless you allow it with `allow_read = true` under `[security.osc52]`.

## "unknown terminal type: xterm-termeric"

Programs inside Termeric see `TERM=xterm-termeric`. When you log in to another machine with `ssh`, the remote side receives that name too, and complains if it doesn't have the matching terminfo entry. Install the entry on the remote machine once:

```bash
infocmp -x xterm-termeric | ssh host 'mkdir -p ~/.terminfo && tic -x -o ~/.terminfo /dev/stdin'
```

Until you do, use the widely known `xterm-256color` for that connection:

```bash
TERM=xterm-256color ssh host
```

The same applies to containers (install the entry in the image, or pass `TERM=xterm-256color`). `sudo` sometimes can't see a per-user entry in `~/.terminfo`; install the entry system-wide or run `sudo env TERM=xterm-256color command`.

To make every program in Termeric use `xterm-256color`, start it with `TERMERIC_TERM=xterm-256color termeric --separate-process`. Termeric also does this by itself when the `xterm-termeric` entry isn't installed locally.

## Configuration errors

If `config.toml` has a mistake, including a misspelled or unknown key, Termeric still starts, using default settings, and shows a notification with the error and its line. The notification stays until the file loads again. It won't save over the broken file (for example when you pick a theme) until you fix it, so your edits aren't lost. Fix the file and save it; Termeric reloads it automatically.

If you save an invalid change while Termeric is running, it keeps the previous settings and shows why the reload failed. If the configured theme can't be loaded at startup, Termeric uses the Turmeric theme; if it breaks while Termeric runs, the current theme stays. Either way your other settings apply, a notice stays until the theme loads, and saving doesn't replace the theme you configured.

To start over, run **Reset Configuration to Defaults** from the command palette and confirm. Termeric keeps a copy of your old file beside the new one, named `config.toml.<number>.bak`, and the notice says where it is.

## Collect logs

Termeric writes its log to standard error. To capture a detailed log, start it from another terminal:

```bash
RUST_LOG=warn,termeric=debug termeric --separate-process 2> termeric.log
```

This records Termeric's own detailed messages plus warnings from the libraries it uses, such as the clipboard and graphics drivers.

Reproduce the problem, close Termeric, and look through `termeric.log`. Its renderer messages name the GPU or software adapter, the graphics API, and whether software rendering was used. Before sharing a log or a transcript, read it through: it can contain text from your terminal.
