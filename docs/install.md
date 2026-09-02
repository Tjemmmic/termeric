# Installing Termeric on Linux

Termeric currently targets modern Linux desktops with Vulkan or another WGPU
backend. Wayland and X11 are both built through Winit; release acceptance is
tracked separately for each display stack.

## Build and install from source

Install a stable Rust toolchain (Rust 1.85 or newer) and the platform packages
needed by Winit, Wayland/X11, fontconfig, Vulkan, OpenSSL, and libgit2. On
Debian or Ubuntu:

```bash
sudo apt-get install build-essential pkg-config libfontconfig1-dev \
  libwayland-dev libxkbcommon-dev libxcb-shape0-dev libxcb-xfixes0-dev \
  libssl-dev libvulkan-dev mesa-vulkan-drivers
cargo build --release --locked
sudo install -Dm755 target/release/termeric /usr/local/bin/termeric
sudo install -Dm644 assets/termeric.desktop \
  /usr/local/share/applications/termeric.desktop
sudo install -Dm644 assets/icons/termeric.png \
  /usr/local/share/icons/hicolor/1024x1024/apps/termeric.png
sudo tic -x -o /usr/local/share/terminfo assets/terminfo/xterm-termeric.terminfo
```

Alternatively, build the Debian artifact with `cargo-deb`:

```bash
cargo install cargo-deb --locked
cargo deb -p termeric --locked
sudo apt install ./target/debian/termeric_*.deb
```

Maintainers can inspect the generated artifact without installing it:

```bash
python3 scripts/check_debian_package.py target/debian/termeric_*.deb
```

The destructive install/upgrade/remove mode is intended only for a fresh,
disposable Debian-family host. It requires root and the explicit
`TERMERIC_PACKAGE_LIFECYCLE_ACCEPT=1` opt-in; it refuses to replace a Termeric
package that was already installed. CI runs that mode on its fresh Ubuntu
runner. See [testing.md](testing.md#debian-package-lifecycle-lane) for the exact
contract.

## Upgrade or uninstall

For a source installation, fetch the desired audited revision and repeat the
locked release build and three `install` commands. Remove it with:

```bash
sudo rm /usr/local/bin/termeric
sudo rm /usr/local/share/applications/termeric.desktop
sudo rm /usr/local/share/icons/hicolor/1024x1024/apps/termeric.png
sudo rm /usr/local/share/terminfo/x/xterm-termeric
```

For a Debian installation, use `sudo apt install ./new-package.deb` to upgrade
and `sudo apt remove termeric` to uninstall. User configuration and session
data are retained by both uninstall paths.

## User files and shell integration

Configuration and custom themes live under `~/.config/termeric/` on a normal
Linux setup. Declarative plugin state uses `plugins.toml` and one bounded
installed manifest under `plugins/<id>/`; uninstall retains both until the user
explicitly runs `termeric plugin remove ID` or removes their configuration.
Saved workspaces, legacy sessions, and transcripts use the platform data directory,
normally `~/.local/share/termeric/`. When experimental Harvest mode is enabled,
generated Bash and Zsh integration files are placed under
`~/.config/termeric/shell-integration/`; Fish uses a launch-time init command.
Nushell uses no generated hook file: Termeric enables modern Nushell's native
OSC 7/133 producers with a pre-REPL command after its normal configuration
loads. Existing user pre-REPL commands are left untouched rather than combined.
For an interactive Elvish launch, Termeric writes
`shell-integration/elvish/lib/termeric-integration.elv` and exposes its parent
through Elvish's standard module search path. Add
`use termeric-integration` once to `~/.config/elvish/rc.elv`; the module appends
its hooks and restores the original module-search environment before child
commands inherit it. Elvish command/script, daemon, language-server, version,
and `-norc` launches remain untouched.
Termeric loads the user's normal Bash/Zsh configuration before its generated
hook.
The installed desktop entry declares `StartupWMClass=termeric`, enables startup
notification, and exposes a **New Window** action using `termeric --new-window`.
Custom launchers may set matching X11/Wayland identity with `--app-id`; see
[`command-line.md`](command-line.md).

## Dependency updates

`Cargo.lock` is committed because Termeric ships an application. Dependency
updates must be intentional, reviewed as their own change when practical, and
verified with formatting, strict Clippy, workspace tests, a workspace build,
package creation, and the Linux GPU smoke lane. Release and CI builds use
`--locked` so an unreviewed resolver change cannot alter an artifact.
The core crate is package-verified first; the application tarball is assembled
with the whole workspace and `--no-verify` until that matching core version has
been published to the target registry.

## Troubleshooting

- If no GPU adapter is found, Termeric automatically tries WGPU's software
  fallback. Install current Mesa/Vulkan drivers and try `termeric --renderer
  software`; `WGPU_BACKEND=vulkan` can restrict the API and `vulkaninfo` can
  confirm llvmpipe or hardware visibility. The Debian package recommends
  `mesa-vulkan-drivers`; source and non-Debian installs must provide an
  equivalent WGPU software implementation. On a Mesa/OpenGL installation,
  `WGPU_BACKEND=gl LIBGL_ALWAYS_SOFTWARE=true termeric --renderer software`
  makes the driver choice explicit before WGPU starts. Startup reports every
  failed path.
- If launch fails under Wayland, confirm `libwayland` and `libxkbcommon` are
  installed; try `WINIT_UNIX_BACKEND=x11 termeric` to isolate display issues.
- If glyphs are missing, install a monospace font with the required Unicode
  coverage and choose it in Settings. Font discovery uses fontconfig on Linux.
- If clipboard access fails, verify that the desktop session exposes the
  Wayland data-control or X11 clipboard protocol expected by `arboard`.
- If a remote or elevated application reports `unknown terminal type:
  xterm-termeric`, install the entry on that host or use the compatibility
  commands in [`terminfo.md`](terminfo.md).
- If an experimental live-domain daemon is stale, inspect it with `termeric
  domain list`; terminate each intended domain with `domain kill`, then run
  `domain stop-server`. Its private socket disappears on clean shutdown. The
  0600 token may remain in `$XDG_RUNTIME_DIR/termeric` (or
  `/tmp/termeric-UID`) and is reused for that login/runtime directory.
  Never delete a live socket merely to bypass the ownership checks; see
  [`live-domains.md`](live-domains.md).
- Start with `RUST_LOG=termeric=debug termeric` to collect diagnostic logs.
  Renderer logs include policy, adapter, WGPU backend, device type, software
  state, and recovery results. See [renderer-recovery.md](renderer-recovery.md).
  Transcripts may contain sensitive terminal data and should not be attached
  without review.
