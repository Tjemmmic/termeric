# Installation

Termeric runs on Linux desktops with Wayland or X11. You can install it from a release package or build it from source. Either way, it needs a working graphics driver; if the GPU can't be used, Termeric draws with Mesa's software renderer instead (see [Troubleshooting](troubleshooting.md)).

> **Note.** Release packages are published starting with Termeric's first release. If there is no release for your system yet, [build from source](#build-from-source).

## Debian and Ubuntu

Each release on the [GitHub releases page](https://github.com/Tjemmmic/termeric/releases) includes a `.deb` package. Download it and install it with apt, which also pulls in the libraries it needs:

```bash
sudo apt install ./termeric_*_amd64.deb
```

The package installs the `termeric` command, the desktop entry and icons (so Termeric appears in your application menu), the `xterm-termeric` terminfo entry, the `termeric(1)` manual page, and Bash, Zsh, and Fish completions. It recommends `mesa-vulkan-drivers`, which provides the software renderer used when no GPU is available.

To upgrade, download the newer `.deb` and install it with the same command. To uninstall:

```bash
sudo apt remove termeric
```

## Arch Linux

Termeric is planned for the AUR from its first release, as `termeric` (the latest release) and `termeric-git` (the latest development version from the main branch). Neither is published yet. Until they are, build the development package with the `PKGBUILD` in this repository:

```bash
sudo pacman -S --needed base-devel git
git clone https://github.com/Tjemmmic/termeric.git
cd termeric/packaging/arch
makepkg -si
```

`makepkg` fetches the latest source from GitHub, builds and tests it, and installs it as the `termeric-git` package, with the desktop entry, icons, the `xterm-termeric` terminfo entry, the manual page, and shell completions. It also needs a Vulkan driver for your GPU (`vulkan-radeon`, `vulkan-intel`, or `nvidia-utils`) and `mesa` for the OpenGL backend and the software fallback; these are optional dependencies, so install them if your system doesn't already have them.

To upgrade, run `makepkg -si` in `packaging/arch` again; it builds the newest commit on the main branch. Uninstall with `sudo pacman -R termeric-git`.

## Any other distribution: the release tarball

Each release also includes a tarball for x86_64 and one for arm64 (`aarch64`), for distributions without a package. It needs glibc 2.35 or newer (Ubuntu 22.04 and later, or any current distribution), the Wayland, X11, xkbcommon, and Vulkan or EGL libraries your desktop already uses, and a monospace font.

Download the tarball for your machine and `SHA256SUMS` from the [releases page](https://github.com/Tjemmmic/termeric/releases), then check them. `sha256sum` confirms the download is intact, and `gh attestation verify` (from the [GitHub CLI](https://cli.github.com)) confirms it was built by Termeric's release workflow from the tagged source:

```bash
sha256sum --check --ignore-missing SHA256SUMS
gh attestation verify termeric-*-x86_64-linux.tar.gz --repo Tjemmmic/termeric
```

Unpack it and run its `install.sh`. By default it installs for your user only, under `~/.local`, with the program, desktop entries, icons, AppStream metadata, the `xterm-termeric` terminfo entry, the manual page, shell completions, and documentation:

```bash
tar -xzf termeric-*-x86_64-linux.tar.gz
cd termeric-*-x86_64-linux
./install.sh
```

To install for everyone instead, run `sudo ./install.sh --prefix /usr/local`. Outside `/usr`, `install.sh` writes the program's full path into the desktop entries, because desktop launchers don't always search `~/.local/bin`, and it tells you if that folder isn't on your `PATH` or Zsh needs to be told where the completions are. Termeric finds its terminfo entry next to the program wherever you install it.

To upgrade, unpack the newer tarball and run its `install.sh` with the same `--prefix`; it removes the previous version's files first. To uninstall, run `install.sh --uninstall` (with the same `--prefix`, and `sudo` for a system-wide install) from any release's tarball. It removes exactly the files recorded in `PREFIX/lib/termeric/installed-files`.

## Build from source

You need Rust 1.89 or newer, a C compiler, and the development files for fontconfig, Wayland, xkbcommon, XCB, and Vulkan.

On Debian or Ubuntu, install Rust with [rustup](https://rustup.rs) if the distribution's version is older than 1.89, then install the rest:

```bash
sudo apt install build-essential git pkg-config libfontconfig1-dev libwayland-dev \
  libxkbcommon-dev libxcb-shape0-dev libxcb-xfixes0-dev libvulkan-dev \
  mesa-vulkan-drivers ncurses-bin
```

On Arch Linux:

```bash
sudo pacman -S --needed base-devel git rust pkgconf fontconfig wayland \
  libxkbcommon libx11 ncurses mesa vulkan-icd-loader
```

Then clone and build:

```bash
git clone https://github.com/Tjemmmic/termeric.git
cd termeric
cargo build --release --locked
```

The finished program is `target/release/termeric`. You can run it straight from there, but installing it as described below gives you a menu entry, the right icon, and full terminal capabilities.

### Install

From the `termeric` checkout, stage the install tree (the program, desktop entries, icons, AppStream metadata, terminfo entry, manual page, shell completions, and documentation), then install it with the `install.sh` it contains. For your user only, under `~/.local`:

```bash
python3 scripts/stage_install_tree.py --binary target/release/termeric --out target/install-tree
target/install-tree/install.sh
```

Or system-wide, under `/usr/local`:

```bash
sudo target/install-tree/install.sh --prefix /usr/local
```

Outside `/usr`, `install.sh` writes the program's full path into the desktop entries, because desktop launchers don't always search `~/.local/bin`. It records what it installed in `PREFIX/lib/termeric/installed-files`, and tells you if the program's folder isn't on your `PATH` or Zsh needs to be told where the completions are.

### Why the desktop entry and terminfo matter

- **Desktop entry and icons.** On Wayland, desktops such as GNOME and KDE Plasma take a window's icon and taskbar grouping from the installed desktop entry that matches its app ID, `org.termeric.Termeric`. Without the entry you get a generic icon. A second, hidden entry does the same for the quick-access window. The entry also adds a **New Window** action to the launcher's menu.
- **Terminfo.** With the `xterm-termeric` entry installed, programs in Termeric see `TERM=xterm-termeric` and can use everything Termeric supports. Without it, Termeric sets `TERM=xterm-256color` instead, which works but describes fewer capabilities. If you log in to other machines from Termeric, see [Troubleshooting](troubleshooting.md#unknown-terminal-type-xterm-termeric).

### Upgrade a source build

Pull the latest changes, rebuild, and stage and install again with the same `--prefix`. `install.sh` removes the previous installation's files first:

```bash
git pull
cargo build --release --locked
rm -rf target/install-tree
python3 scripts/stage_install_tree.py --binary target/release/termeric --out target/install-tree
target/install-tree/install.sh
```

### Uninstall a source build

Run `install.sh` with `--uninstall` and the prefix you installed to. It removes exactly the files it recorded:

```bash
target/install-tree/install.sh --uninstall
sudo target/install-tree/install.sh --uninstall --prefix /usr/local
```

## After an upgrade

Windows that are already open keep running the old version. Launching Termeric again after an upgrade starts the new version in a separate process, and tabs can't be dragged between the old and new windows. Close the old windows when you're ready.

## Where Termeric keeps your files

Uninstalling never removes your settings or saved data. They live in two folders (or under `$XDG_CONFIG_HOME` and `$XDG_DATA_HOME` if you set those):

| Folder | Contents |
|---|---|
| `~/.config/termeric/` | `config.toml` (your settings), `themes/` (custom themes), `plugins.toml` and `plugins/` (installed plugins), `shell-integration/` (generated shell hooks) |
| `~/.local/share/termeric/` | Saved workspaces, sessions, and agent transcripts |

To remove Termeric completely, delete both folders after uninstalling. See [Configuration](configuration.md) to start editing `config.toml`.
