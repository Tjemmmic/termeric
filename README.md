# Termeric

A GPU-accelerated terminal emulator, written in Rust and themed around roots and rhizomes. Are you Root?

Termeric keeps your shells in tabs, panes, and windows, draws them with your GPU, and understands what your shell and tools are doing: jump between prompts, open links and paths, show images, and keep an eye on coding agents without leaving the terminal.

## Highlights

- **Tabs, panes, and windows**, with layout presets, zoom, broadcast input, and tabs you can drag between windows
- **Workspaces** that save your windows, tabs, panes, folders, and scrollback, and bring them back at startup
- **Themes** named after roots (Turmeric, Ginger, Daikon, and more), community favorites, and your own
- **Good text**: ligatures, color emoji, wide characters, a fallback chain you control, and a Unicode character picker
- **Images** through the Kitty graphics protocol, Sixel, and iTerm2 inline images
- **Shell integration** for Bash, Zsh, Fish, Nushell, and Elvish: prompt jumping and failed-command navigation
- **Search, keyboard copy mode, and quick-select hints** for links, paths, and hashes
- **A command palette, a Settings dialog, and rebindable shortcuts**
- **Coding-agent integration** (experimental, off until you turn it on) for Claude Code, Codex, Gemini CLI, OpenCode, and your own agents
- **Automation**: launch from the command line, and (experimental) add [plugins](examples/plugins/README.md) and script a running Termeric

## Status

Termeric is a **developer preview for Linux**, on Wayland and X11. Expect rough edges. Configuration and saved workspaces may change between previews without migration, and only the latest preview receives fixes. Please report problems in [GitHub issues](https://github.com/Tjemmmic/termeric/issues).

Some features are experimental and may change between previews: coding agents and Harvest, which stay off until you turn them on in Settings, and scripting a running Termeric, live domains, and third-party plugins. [FEATURES.md](FEATURES.md) lists what works, what partly works, and what is planned.

## Install

Termeric has one install method for each platform it supports, and you can always [build from source](#build-from-source).

| Platform | Architectures | Install with | Status |
| --- | --- | --- | --- |
| Debian 12 and later, Ubuntu 22.04 and later | x86_64, arm64 | [`.deb` package](#debian-and-ubuntu) | Developer preview |
| Arch Linux | x86_64, arm64 | [AUR package](#arch-linux) | Developer preview |
| Other Linux with glibc 2.35 or later | x86_64, arm64 | [Tarball](#other-linux) | Developer preview |
| Windows Subsystem for Linux | x86_64, arm64 | The Linux packages, under WSLg | Planned |
| macOS | — | Chosen with the port | Planned |
| Windows | — | Chosen with the port | Planned |
| FreeBSD and other BSDs | — | Chosen with the port | Planned |

Wayland and X11 both work; [FEATURES.md](FEATURES.md) notes where they differ. The arm64 packages are built by the release workflow but haven't been tested on arm64 hardware yet.

Every package installs the program, its desktop entry and icons, the `xterm-termeric` terminfo entry, the manual page, and Bash, Zsh, and Fish completions. Termeric also needs a Vulkan driver for your GPU, or Mesa, whose software renderer it uses when the GPU can't be used.

### Debian and Ubuntu

For Ubuntu 22.04, Debian 12, and later. Download the `.deb` for your machine from the [releases page](https://github.com/Tjemmmic/termeric/releases) and install it with apt, which also installs the libraries it needs:

```bash
sudo apt install ./termeric_*_amd64.deb
```

On arm64, the file name ends in `_arm64.deb`. To upgrade, install a newer `.deb` the same way. To uninstall, run `sudo apt remove termeric`.

### Arch Linux

Install [`termeric`](https://aur.archlinux.org/packages/termeric) (the latest release) or [`termeric-git`](https://aur.archlinux.org/packages/termeric-git) (the latest commit) from the AUR, with an AUR helper or by hand:

```bash
git clone https://aur.archlinux.org/termeric.git
cd termeric
makepkg -si
```

Also install the Vulkan driver for your GPU (`vulkan-radeon`, `vulkan-intel`, or `nvidia-utils`) if you don't have it. To uninstall, run `sudo pacman -R termeric` (or `termeric-git`).

### Other Linux

The tarball needs glibc 2.35 or newer. Download it and `SHA256SUMS` from the [releases page](https://github.com/Tjemmmic/termeric/releases), check them (`gh attestation verify`, from the [GitHub CLI](https://cli.github.com), confirms the release workflow built the file from the tagged source), and run its `install.sh`, which installs for your user under `~/.local`:

```bash
sha256sum --check --ignore-missing SHA256SUMS
gh attestation verify termeric-*-x86_64-linux.tar.gz --repo Tjemmmic/termeric
tar -xzf termeric-*-x86_64-linux.tar.gz
termeric-*-x86_64-linux/install.sh
```

On arm64, the file names say `aarch64` instead of `x86_64`. To install for everyone instead, run `sudo termeric-*-x86_64-linux/install.sh --prefix /usr/local`. To upgrade, run a newer tarball's `install.sh` with the same `--prefix`; it removes the previous version's files first. To uninstall, run `install.sh --uninstall` with the same `--prefix`.

### Build from source

You need Rust 1.95 or newer, a C compiler, Python 3 (to stage the install tree), and the development files for fontconfig, Wayland, xkbcommon, XCB, and Vulkan. On Debian or Ubuntu, install Rust with [rustup](https://rustup.rs) if the distribution's is older, then:

```bash
sudo apt install build-essential git pkg-config libfontconfig1-dev libwayland-dev \
  libxkbcommon-dev libxcb-shape0-dev libxcb-xfixes0-dev libvulkan-dev \
  mesa-vulkan-drivers ncurses-bin python3
```

On Arch Linux:

```bash
sudo pacman -S --needed base-devel git rust pkgconf fontconfig wayland \
  libxkbcommon libx11 ncurses mesa vulkan-icd-loader python
```

Then build, stage the install tree (the program, desktop entries, icons, AppStream metadata, terminfo entry, manual page, shell completions, and documentation), and install it for your user under `~/.local`:

```bash
git clone https://github.com/Tjemmmic/termeric.git
cd termeric
cargo build --release --locked
python3 scripts/stage_install_tree.py --binary target/release/termeric --out target/install-tree
target/install-tree/install.sh
```

To install for everyone instead, run `sudo target/install-tree/install.sh --prefix /usr/local`. `install.sh` records what it installed in `PREFIX/lib/termeric/installed-files`, and tells you if the program's folder isn't on your `PATH` or Zsh needs to be told where the completions are.

To upgrade, pull, rebuild, remove `target/install-tree`, and stage and install again with the same `--prefix`; `install.sh` removes the previous files first. To uninstall, run `target/install-tree/install.sh --uninstall` with the same `--prefix`. Windows that are already open keep running the old version, so close them when you're ready.

You can run `target/release/termeric` without installing, but two installed files matter:

- **The desktop entry.** On Wayland, desktops take a window's icon and taskbar grouping from the installed desktop entry that matches its app ID, `org.termeric.Termeric`; without it you get a generic icon.
- **The terminfo entry.** With `xterm-termeric` installed, programs see `TERM=xterm-termeric` and can use everything Termeric supports. Without it, Termeric sets `TERM=xterm-256color`, which works but describes less. To install only this entry, run `tic -x -o ~/.terminfo assets/terminfo/xterm-termeric.terminfo`.

### Where Termeric keeps your files

Uninstalling never removes your settings or saved data. They live in two folders, or under `$XDG_CONFIG_HOME` and `$XDG_DATA_HOME` if you set those:

| Folder | Contents |
| --- | --- |
| `~/.config/termeric/` | `config.toml` (your settings), `themes/` (custom themes), `plugins.toml` and `plugins/` (installed plugins), `supervisor.md` (agent supervisor rules), `shell-integration/` (generated shell hooks) |
| `~/.local/share/termeric/` | Saved workspaces and tab snapshots, and agent transcripts and exports |

## Getting started

- **Ctrl+Shift+P** opens the command palette, which lists every command with its shortcut. If you remember one key, make it this one.
- **Ctrl+,** opens Settings. Changes apply as you make them; **Ctrl+S** saves them to `~/.config/termeric/config.toml`.
- **Change Theme…** in the palette previews each theme as you move through the list.
- **Keyboard Shortcuts** in the palette lists and rebinds every shortcut.
- **F6** moves keyboard focus to the tab bar and toolbar, and **Escape** returns it to the terminal.

You can also edit `config.toml` by hand; Termeric reloads it when you save it. [`assets/config.example.toml`](assets/config.example.toml), installed as `share/doc/termeric/config.example.toml`, lists every setting with its default and range. To write a theme, start from [`assets/themes/custom-example.toml`](assets/themes/custom-example.toml) and save it in `~/.config/termeric/themes/`.

Bash, Zsh, Fish, and Nushell get shell integration on their own. For Elvish, add `use termeric-integration` to `~/.config/elvish/rc.elv`.

`termeric --help` and `man termeric` describe the command line, including when a launch opens its window in an already running Termeric. Each subcommand (`control`, `domain`, `plugin`, `workspace`, `mcp`) has its own `--help`.

## Troubleshooting

To see what went wrong, start Termeric from another terminal with a detailed log; read it through before sharing it, since it can contain text from your terminal:

```bash
RUST_LOG=warn,termeric=debug termeric --separate-process 2> termeric.log
```

**It won't start, or shows nothing.** Termeric renders with Vulkan or OpenGL and falls back to software rendering (Mesa's llvmpipe) when the GPU can't be used. Make sure your graphics drivers are installed (`mesa-vulkan-drivers` on Debian and Ubuntu; `mesa` and `vulkan-icd-loader` on Arch; `vulkaninfo --summary` shows what Vulkan sees), then narrow it down:

```bash
termeric --renderer software
WGPU_BACKEND=gl termeric --renderer software
WGPU_BACKEND=gl LIBGL_ALWAYS_SOFTWARE=true termeric --renderer software
```

Set `backend` under `[renderer]` in `config.toml` to keep a choice. A launch that sets one of these variables to a value the running Termeric doesn't have starts a process of its own, so these work while Termeric is running.

**Wayland or X11.** Termeric uses Wayland when `WAYLAND_DISPLAY` is set. To try X11 through XWayland, run `env -u WAYLAND_DISPLAY termeric`.

**"unknown terminal type: xterm-termeric".** A remote host, `sudo`, or a container doesn't have Termeric's terminfo entry. Copy it to a host once, or use the common type for that connection:

```bash
infocmp -x xterm-termeric | ssh host 'mkdir -p ~/.terminfo && tic -x -o ~/.terminfo /dev/stdin'
TERM=xterm-256color ssh host
```

For `sudo`, install the entry system-wide or run `sudo env TERM=xterm-256color command`. To have every program in Termeric see `xterm-256color`, start it with `TERMERIC_TERM=xterm-256color termeric`.

**Boxes instead of characters.** None of your fonts has them. Install fonts that cover them (such as Noto Sans Mono, Noto Color Emoji, and Noto Sans CJK), and add them as fallbacks on the **Font** page of Settings or with `fallback` under `[font]`.

**Copy and paste.** Use **Ctrl+Shift+C** and **Ctrl+Shift+V**; plain Ctrl+C interrupts the program. When a program uses the mouse, hold **Shift** to select. On a Wayland compositor without the data-control protocol, Termeric reaches the clipboard through XWayland, which must be running. A failed copy shows a notice with the reason.

**A mistake in `config.toml`.** Termeric still starts, with the defaults or the settings it had, and shows a notice naming the key and line until the file loads. It never saves over a file with an error. **Reset Configuration to Defaults** in the palette starts over and keeps the old file beside the new one.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). [AGENTS.md](AGENTS.md) describes the architecture for contributors and coding agents. If you package Termeric for a distribution, see [PACKAGING.md](PACKAGING.md). Report security problems privately as described in [SECURITY.md](SECURITY.md).

## License

Termeric is released under the [MIT License](LICENSE). Third-party licenses are listed in [THIRD_PARTY_NOTICES.md](termeric/THIRD_PARTY_NOTICES.md).
