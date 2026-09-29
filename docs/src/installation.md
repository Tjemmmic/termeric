# Installation

Termeric runs on Linux desktops with Wayland or X11. You can install it from a release package or build it from source. Either way, it needs a working graphics driver; if the GPU can't be used, Termeric draws with Mesa's software renderer instead (see [Troubleshooting](troubleshooting.md)).

> **Note.** Release packages are published starting with Termeric's first release. If there is no release for your system yet, [build from source](#build-from-source).

## Debian and Ubuntu

Each release on the [GitHub releases page](https://github.com/Tjemmmic/termeric/releases) includes a `.deb` package. Download it and install it with apt, which also pulls in the libraries it needs:

```bash
sudo apt install ./termeric_*_amd64.deb
```

The package installs the `termeric` command, the desktop entry and icons (so Termeric appears in your application menu), and the `xterm-termeric` terminfo entry. It recommends `mesa-vulkan-drivers`, which provides the software renderer used when no GPU is available.

To upgrade, download the newer `.deb` and install it with the same command. To uninstall:

```bash
sudo apt remove termeric
```

## Arch Linux

Termeric is in the AUR as two packages:

| Package | Builds |
|---|---|
| `termeric` | The latest release |
| `termeric-git` | The latest development version from the main branch |

Install either one with an AUR helper such as `paru` or `yay`:

```bash
paru -S termeric
```

Or build it yourself with `makepkg`:

```bash
sudo pacman -S --needed base-devel git
git clone https://aur.archlinux.org/termeric.git
cd termeric
makepkg -si
```

The optional dependencies `mesa` and `vulkan-icd-loader` provide the Vulkan and OpenGL drivers Termeric renders with, including the software fallback. Install them if your system doesn't already have them.

Upgrade with your AUR helper (`paru -Syu`), or pull the AUR repository and run `makepkg -si` again. Uninstall with `sudo pacman -R termeric` (or `termeric-git`).

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

### Install system-wide

From the `termeric` checkout, install the program, desktop entry, icons, and terminfo entry under `/usr/local`:

```bash
sudo install -Dm755 target/release/termeric /usr/local/bin/termeric
sudo install -Dm644 assets/termeric.desktop /usr/local/share/applications/termeric.desktop
for icon in assets/icons/hicolor/*/apps/termeric.png; do
  sudo install -Dm644 "$icon" "/usr/local/share/icons/${icon#assets/icons/}"
done
sudo tic -x -o /usr/local/share/terminfo assets/terminfo/xterm-termeric.terminfo
```

### Install for your user only

To install without root, put everything under your home directory. The desktop entry is rewritten to point at the program's full path, because desktop launchers don't always search `~/.local/bin`:

```bash
install -Dm755 target/release/termeric "$HOME/.local/bin/termeric"
for icon in assets/icons/hicolor/*/apps/termeric.png; do
  install -Dm644 "$icon" "$HOME/.local/share/icons/${icon#assets/icons/}"
done
mkdir -p "$HOME/.local/share/applications"
sed "s|^Exec=termeric|Exec=$HOME/.local/bin/termeric|" assets/termeric.desktop \
  > "$HOME/.local/share/applications/termeric.desktop"
tic -x -o "$HOME/.terminfo" assets/terminfo/xterm-termeric.terminfo
```

### Why the desktop entry and terminfo matter

- **Desktop entry and icons.** On Wayland, desktops such as GNOME and KDE Plasma take a window's icon and taskbar grouping from the installed desktop entry that matches its app ID, `termeric`. Without the entry you get a generic icon. The entry also adds a **New Window** action to the launcher's menu.
- **Terminfo.** With the `xterm-termeric` entry installed, programs in Termeric see `TERM=xterm-termeric` and can use everything Termeric supports. Without it, Termeric sets `TERM=xterm-256color` instead, which works but describes fewer capabilities. If you log in to other machines from Termeric, see [Troubleshooting](troubleshooting.md#unknown-terminal-type-xterm-termeric).

### Upgrade a source build

Pull the latest changes, rebuild, and repeat the install commands you used:

```bash
git pull
cargo build --release --locked
```

### Uninstall a source build

For a system-wide install:

```bash
sudo rm /usr/local/bin/termeric /usr/local/share/applications/termeric.desktop
sudo rm /usr/local/share/icons/hicolor/*/apps/termeric.png
sudo rm /usr/local/share/terminfo/x/xterm-termeric
```

For a per-user install:

```bash
rm ~/.local/bin/termeric ~/.local/share/applications/termeric.desktop
rm ~/.local/share/icons/hicolor/*/apps/termeric.png ~/.terminfo/x/xterm-termeric
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
