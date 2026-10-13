# Packaging Termeric

Notes for people packaging Termeric for a Linux distribution. Termeric's own packages (the release tarball, the `.deb`, and the two AUR recipes in `packaging/arch/`: `termeric`, built from a release's source, and `termeric-git`, built from the latest commit) are built exactly as described here; use them as references.

## Identity

| What | Value |
| --- | --- |
| Application ID (Wayland `app_id`, X11 `WM_CLASS`) | `org.termeric.Termeric` |
| Quick-access window ID | `org.termeric.Termeric.QuickAccess` |
| Desktop entries | `org.termeric.Termeric.desktop`, and the hidden (`NoDisplay=true`) `org.termeric.Termeric.QuickAccess.desktop`, which only gives the quick-access window its icon |
| Icons | `org.termeric.Termeric.png` in the hicolor sizes 16 to 512 |
| AppStream component | `org.termeric.Termeric` |
| Binary | `termeric` |
| Terminfo entry | `xterm-termeric` |

Desktops match a window to the desktop entry and icon of the same name as its application ID, so keep these file names.

## Building

Termeric needs Rust 1.95 or newer, a C compiler (for the bundled libgit2), `pkg-config`, Python 3 (for `scripts/stage_install_tree.py`), and the development files for fontconfig, Wayland, xkbcommon, XCB, and Vulkan. [Build from source](README.md#build-from-source) lists the Debian and Arch package names.

```bash
cargo build --release --locked
```

`Cargo.lock` is committed; build with `--locked`. libgit2 is always built from the source bundled in `libgit2-sys`, without network transports, so no system libgit2, OpenSSL, or libssh2 is involved.

Don't let your build system inject cross-language LTO flags (`-flto` in `CFLAGS`). The x86_64 Rust toolchain links with rust-lld, which can't read the GCC LTO objects the bundled libgit2 would then contain. The PKGBUILDs turn off makepkg's LTO with `options=('!lto')` for this reason; Cargo's own release optimization still applies.

## Tests

```bash
cargo test --workspace --locked
```

The suite needs no display, GPU, network, session bus, or installed fonts, so it runs in a clean build chroot. The GPU render smoke and the resource soak are ignored by default. The Bash, Zsh, and Fish integration tests skip when a shell is missing unless `TERMERIC_REQUIRE_SHELL_ACCEPTANCE=1` is set. [Testing](CONTRIBUTING.md#testing) covers the rest.

## The install tree

`scripts/stage_install_tree.py` stages everything an installation puts under its prefix:

```bash
python3 scripts/stage_install_tree.py --binary target/release/termeric --out DIR
```

| Path | Contents |
| --- | --- |
| `bin/termeric` | The program |
| `share/applications/` | The two desktop entries |
| `share/metainfo/org.termeric.Termeric.metainfo.xml` | AppStream metadata |
| `share/icons/hicolor/*/apps/` | The icons |
| `share/terminfo/x/xterm-termeric` | The compiled terminfo entry |
| `share/man/man1/termeric.1` | The manual page |
| `share/bash-completion/completions/termeric`, `share/zsh/site-functions/_termeric`, `share/fish/vendor_completions.d/termeric.fish` | Shell completions |
| `share/doc/termeric/` | `README.md`, `CHANGELOG.md`, `FEATURES.md`, `LICENSE`, `THIRD_PARTY_NOTICES.md`, and `config.example.toml` |

`DIR` also gets `install.sh` and `MANIFEST`, which lists each file to install with its mode. To install the tree into a package root:

```bash
DIR/install.sh --prefix /usr --destdir "$pkgdir"
```

With `--destdir`, `install.sh` copies the files and nothing else: it records nothing and refreshes no caches, since the package manager tracks the files and its hooks refresh the caches. Without it, `install.sh` installs for a user (`--prefix` defaults to `~/.local`), records the files for `--uninstall`, and, outside `/usr`, writes the program's full path into the desktop entries. That is how the release tarball is installed.

The tree is relocatable. Termeric looks for its terminfo entry in `PREFIX/share/terminfo` next to its own binary (as well as the usual places), so the entry needs no system-wide installation and no `tic` at install time.

`THIRD_PARTY_NOTICES.md` lists the licenses of every crate compiled into the binary and of the bundled libgit2; ship it. Arch packages also install `LICENSE` to `/usr/share/licenses/$pkgname/`.

## Runtime dependencies

The binary links only libc, libm, libgcc_s, and libz. Everything else is loaded at runtime with `dlopen`, so tools that read `NEEDED` entries, such as `dpkg-shlibdeps`, can't find it; declare these yourself:

| Library | Loaded by | Arch | Debian and Ubuntu |
| --- | --- | --- | --- |
| `libwayland-client.so.0`, `libwayland-cursor.so.0` | winit, on Wayland | `wayland` | `libwayland-client0`, `libwayland-cursor0` |
| `libwayland-egl.so.1` | wgpu's OpenGL backend, on Wayland | `wayland` | `libwayland-egl1` |
| `libX11.so.6`, `libX11-xcb.so.1` | winit, on X11 | `libx11` | `libx11-6`, `libx11-xcb1` |
| `libXcursor.so.1` | winit, on X11 | `libxcursor` | `libxcursor1` |
| `libXi.so.6` | winit, on X11 | `libxi` | `libxi6` |
| `libxcb.so.1` | x11rb, on X11 | `libxcb` | `libxcb1` |
| `libxkbcommon.so.0`, `libxkbcommon-x11.so.0` | keyboard handling | `libxkbcommon`, `libxkbcommon-x11` | `libxkbcommon0`, `libxkbcommon-x11-0` |
| `libvulkan.so.1` | wgpu's Vulkan backend | `vulkan-icd-loader` | `libvulkan1` |
| `libEGL.so.1` | wgpu's OpenGL backend | `libglvnd` | `libegl1` |

Termeric doesn't load libXrandr. It reads the fontconfig configuration files itself, without libfontconfig, so it needs the configuration (`fontconfig` on Arch, `fontconfig-config` on Debian) and at least one monospace font; it bundles no font. On Arch that is the `ttf-font` dependency; on Debian, `fontconfig-config` pulls in a font.

For rendering it also needs a Vulkan driver for the GPU, or Mesa: Mesa's llvmpipe and lavapipe are the software fallback Termeric uses when no GPU can be used. Termeric's packages make these optional (the PKGBUILDs' `optdepends` on `vulkan-driver` and `mesa`; the `.deb` recommends `mesa-vulkan-drivers`).

The Debian and Ubuntu names in `termeric/Cargo.toml` exist on Ubuntu 22.04 and 24.04 and Debian 12 and 13: the release `.deb` installs and removes cleanly on each (checked 2026-10-10 with `check_debian_package.py --system-lifecycle`). cargo-deb's `$auto` adds `libc6 (>= 2.35)` and `zlib1g`; `libgcc-s1` comes with `libc6`, which depends on it.

`scripts/check_debian_package.py` and `scripts/check_arch_package.py` fail a package that misses any of them.

## Checking a package

```bash
python3 scripts/check_debian_package.py target/debian/termeric_*.deb
python3 scripts/check_arch_package.py target/arch/termeric-*.pkg.tar.zst
```

Both compare the package with the staged tree's file list and modes (with the manual page gzipped, as cargo-deb and makepkg's default `zipman` option ship it), validate the desktop entries and icons, check the terminfo entry with `infocmp`, and check the declared dependencies. `--system-lifecycle` also installs and removes the package; it needs root, so use a disposable container. Docker's Ubuntu and Arch images skip installing documentation (Ubuntu's `/etc/dpkg/dpkg.cfg.d/excludes`, and the `NoExtract` lines in Arch's `/etc/pacman.conf`, which also skip manual pages), so remove those first, or the check reports those files missing. [Packages](CONTRIBUTING.md#packages) has the build commands.

The AppStream metadata validates with `appstreamcli validate --no-net assets/org.termeric.Termeric.metainfo.xml`, and the desktop entries with `desktop-file-validate`; CI runs both. The metadata has no screenshots yet: they need stable URLs, which come once screenshots are committed to the repository and can be linked at a release tag.

## Generated files

The manual page and completions under `assets/man/` and `assets/completions/` are generated from the command description in `termeric/src/cli/spec.rs` and committed, so packaging needs no extra build step. The hicolor icons are rendered by `scripts/generate_icons.py` and committed too.

## Releases

Pushing a `v*` tag runs `.github/workflows/release.yml`. It checks that the tag is `v` plus the workspace version, that `CHANGELOG.md` has a dated section for that version, the AppStream metadata a `<release>` with the same version and date, and the `termeric` PKGBUILD the same `pkgver`, and that CI passed on the tagged commit. It then builds for x86_64 and arm64 inside an Ubuntu 22.04 container, so the binaries need glibc 2.35 or newer, and produces for each architecture a tarball of the install tree and a `.deb` that `check_debian_package.py` has passed. It also checks that each PKGBUILD's `.SRCINFO` matches it. Finally it writes `SHA256SUMS`, attests the build provenance of each file, and drafts a GitHub pre-release with the changelog section as its notes.

To cut a release: set the version in `Cargo.toml` and as `pkgver` in `packaging/arch/termeric/PKGBUILD` and its `.SRCINFO` (with the version's `CHANGELOG.md` section and AppStream `<release>` entry in place), run `python3 scripts/check_release.py --stamp today` to date both with the same day, commit, wait for CI on that commit, then tag it. Running the workflow by hand without `publish` builds and checks everything without drafting a release. Attestations need a public repository.

### The AUR

Each directory in `packaging/arch/` holds what one AUR repository holds: a `PKGBUILD` and its `.SRCINFO`. `termeric-git` always builds the newest commit, so it only needs publishing when its recipe changes. `termeric` downloads GitHub's archive of the release tag, whose checksum can't be in the tag itself, so it is pinned after the release is published. Until then its checksum is a placeholder that makepkg rejects, so an unpinned recipe can't be installed by mistake. From the repository root, with an AUR account and its SSH key:

```bash
git clone ssh://aur@aur.archlinux.org/termeric.git ../aur-termeric
cp packaging/arch/termeric/PKGBUILD ../aur-termeric/
(cd ../aur-termeric && updpkgsums && makepkg --printsrcinfo > .SRCINFO && makepkg --force)
python3 scripts/check_arch_package.py ../aur-termeric/termeric-*.pkg.tar.zst
cp ../aur-termeric/PKGBUILD ../aur-termeric/.SRCINFO packaging/arch/termeric/
```

`makepkg` there downloads the tag's archive, checks it against the pinned checksum, builds, and runs the tests. Commit the pinned recipe here, then commit `PKGBUILD` and `.SRCINFO` in `../aur-termeric` and push. `termeric-git` is published the same way from `ssh://aur@aur.archlinux.org/termeric-git.git`, without `updpkgsums`.

To package a checkout that isn't on GitHub yet, `scripts/build_arch_package.py --package termeric` (or `termeric-git`) builds a recipe from the local checkout: for `termeric`, an archive of the committed `HEAD`, without checking its checksum; for `termeric-git`, the working tree. [Packages](CONTRIBUTING.md#packages) has the commands.
