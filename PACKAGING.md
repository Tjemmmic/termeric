# Packaging Termeric

Notes for people packaging Termeric for a Linux distribution. Termeric's own packages (the release tarball, the `.deb`, and the three AUR recipes in `packaging/arch/`: `termeric`, built from a release's source, `termeric-bin`, the release's tarball, and `termeric-git`, built from the latest commit) are built exactly as described here; use them as references.

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

The suite needs `git`, but no display, GPU, network, session bus, or installed fonts, so it runs in a clean build chroot. The GPU render smoke and the resource soak are ignored by default. The Bash, Zsh, and Fish integration tests skip when a shell is missing unless `TERMERIC_REQUIRE_SHELL_ACCEPTANCE=1` is set. [Testing](CONTRIBUTING.md#testing) covers the rest.

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

The Debian and Ubuntu names in `termeric/Cargo.toml` exist on Ubuntu 22.04, 24.04, and 26.04 and Debian 12 and 13: the release workflow installs and removes the `.deb` on each with `check_debian_package.py --system-lifecycle`. cargo-deb's `$auto` adds `libc6 (>= 2.35)` and `zlib1g`; `libgcc-s1` comes with `libc6`, which depends on it.

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

To release a version, with its changes under `[Unreleased]` in `CHANGELOG.md`:

1. From an up-to-date `main`, run `python3 scripts/prepare_release.py 0.2.0 --pull-request`. It sets the version everywhere, dates the changelog section and the AppStream release, and opens a pull request. Merge it. If it waits, `python3 scripts/check_release.py --stamp today` moves the date.
2. Once CI passes on `main`, tag that commit: `git tag v0.2.0 && git push origin v0.2.0`. `release.yml` builds and checks the packages and drafts a GitHub release.
3. Read the draft and publish it. `publish.yml` updates the apt repository and the AUR.

Versions are always `X.Y.Z`, with no suffix.

`release.yml` checks that the tag is `v` plus the workspace version, that `CHANGELOG.md` has a dated section for that version, the AppStream metadata a `<release>` with the same version and date, and the `termeric` PKGBUILD the same `pkgver`, that each PKGBUILD's `.SRCINFO` matches it, and that CI passed on the tagged commit. It builds for x86_64 and arm64 inside an Ubuntu 22.04 container, so the binaries need glibc 2.35 or newer, and produces for each architecture a tarball of the install tree and a `.deb`, which it inspects, then installs and removes on every supported Debian and Ubuntu. Finally it writes `SHA256SUMS`, attests the build provenance of each file, and drafts the release with the changelog section as its notes. Running it by hand without `publish` builds and checks everything without drafting a release. Attestations need a public repository.

`publish.yml` takes the newest published release only, checks its files against `SHA256SUMS` and their attestations, and then publishes to each place on its own, so one failing doesn't stop the other. To publish again, after a failure or a fixed recipe, run it by hand from the Actions tab with the release's tag; what is already published stays as it is.

### The apt repository

`scripts/build_apt_repository.py` writes the repository from nothing for each release, signs it, and includes `termeric.sources`, which carries the public key. The workflow installs from it on every supported Debian and Ubuntu, upgrading from the previous release when there is one, and then deploys it to GitHub Pages under `/apt`. It holds only the latest release, to stay small.

People who added the repository trust its signing key and its URL, so keep both. With a new key, everyone has to download `termeric.sources` again.

### The AUR

Each directory in `packaging/arch/` holds what one AUR repository holds: a `PKGBUILD` and its `.SRCINFO`. `termeric` downloads GitHub's archive of the release tag, whose checksum can't be in the tag itself, and `termeric-bin` the release's tarballs, which don't exist until the release is built, so the recipes here always carry placeholders, which makepkg rejects. The workflow pins `termeric` with `updpkgsums` and `termeric-bin` from the release's attested `SHA256SUMS`, builds both on a clean Arch Linux (`termeric` with its tests), installs and removes each, and pushes the copies to the AUR. `termeric-git` always builds the newest commit, so it only changes there when its recipe does, or with a release's version.

The recipes run the tests with `setsid`, in a session of their own. Some tests start interactive shells, which take over the terminal they start from; run from a person's terminal by an AUR helper, they would stop the build.

To fix the recipe of a released version, raise its `pkgrel` here and in its `.SRCINFO`, merge, and run the workflow by hand from `main` with that release's tag, before `main` moves to the next version.

To package a checkout that isn't on GitHub yet, `scripts/build_arch_package.py --package termeric` (or `termeric-git`) builds a recipe from the local checkout (`termeric-bin` needs a release's tarballs, so only the workflow builds it): for `termeric`, an archive of the committed `HEAD`, without checking its checksum; for `termeric-git`, the working tree. [Packages](CONTRIBUTING.md#packages) has the commands.

### Setting up publishing

This is done once, by the repository's owner. Each secret belongs to an environment that only `v*` tags and `main` can use. Pages has to allow tags as well, since publishing a release runs the workflow on its tag:

```bash
repository=repos/Tjemmmic/termeric
gh api -X POST $repository/pages -f build_type=workflow
for environment in apt aur github-pages; do
  gh api -X PUT $repository/environments/$environment \
    -F 'deployment_branch_policy[protected_branches]=false' \
    -F 'deployment_branch_policy[custom_branch_policies]=true'
  gh api -X POST $repository/environments/$environment/deployment-branch-policies -f name='v*' -f type=tag
  gh api -X POST $repository/environments/$environment/deployment-branch-policies -f name=main -f type=branch
done
```

The apt signing key is made for this and nothing else, in a keyring of its own. Keep a private backup of that keyring: GitHub never shows a secret again.

```bash
export GNUPGHOME=~/termeric-signing-key
install -d -m 700 "$GNUPGHOME"
gpg --batch --passphrase '' --quick-generate-key 'Termeric package signing' ed25519 sign never
gpg --armor --export-secret-keys | gh secret set APT_SIGNING_KEY --env apt --repo Tjemmmic/termeric
unset GNUPGHOME
```

The AUR key is also made for this, so it can be removed without touching anyone's own key. Add the public half to the AUR account's SSH keys (one per line):

```bash
ssh-keygen -t ed25519 -N '' -C 'termeric publish workflow' -f ~/termeric-aur-key
gh secret set AUR_SSH_KEY --env aur --repo Tjemmmic/termeric < ~/termeric-aur-key
cat ~/termeric-aur-key.pub
```
