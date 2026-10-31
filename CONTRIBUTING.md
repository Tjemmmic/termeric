# Contributing to Termeric

Thanks for your interest in Termeric. This page covers building from source, the checks a change needs to pass, the slower suites to run by hand, and the conventions the project follows.

## Building

You need a stable Rust toolchain (1.95 or newer, the `rust-version` in `Cargo.toml`) and the development packages for fontconfig, Wayland, xkbcommon, XCB, and Vulkan; [Build from source](README.md#build-from-source) lists them for Debian, Ubuntu, and Arch Linux. Then:

```bash
cargo build --locked
cargo run --locked
```

The workspace has two crates: `termeric/` is the application (windowing, rendering, UI, agents, plugins) and `termeric-core/` is the terminal emulation library (parser, grid, PTY). [AGENTS.md](AGENTS.md) describes the architecture and the rules that keep input, PTY output, and panes routed correctly. Read it before changing anything in `app/`, `renderer/`, or `tabs/`.

## Before you send a change

Run the checks CI runs on every pull request:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
python3 -m unittest $(git ls-files 'scripts/test_*.py' | sed 's|/|.|; s|\.py$||')
python3 scripts/check_markdown_links.py
python3 scripts/check_terminfo.py --require-apps
```

`check_terminfo.py --require-apps` compiles `assets/terminfo/xterm-termeric.terminfo` and runs Vim, less, and tmux against it, so those three need to be installed. The Python tests cover the helper scripts themselves.

The scripts in `scripts/` need Python 3.11 or newer, and `generate_icons.py` also needs Pillow.

Add tests with your change. A bug fix should come with a test that fails without it.

## Testing

Most of Termeric's tests are ordinary Rust unit and integration tests that run anywhere. A few are ignored by default because they need a GPU or several minutes; run them by hand. Behavior that needs a real display is checked by hand before a release.

### CI

`.github/workflows/ci.yml` runs the checks above on pull requests and pushes to `main`. It sets `TERMERIC_REQUIRE_SHELL_ACCEPTANCE=1`, so the Bash, Zsh, and Fish integration tests fail instead of skipping when one of those shells is missing; set it locally to match.

`.github/workflows/fuzz.yml` runs the three fuzz targets (see [Fuzzing](#fuzzing)) every Monday against `main`, and whenever it's started from the Actions tab.

`.github/workflows/release.yml` runs for each `v*` tag: it builds the tarball and `.deb` for x86_64 and arm64, runs `check_debian_package.py` on each `.deb`, and drafts the GitHub release (see [PACKAGING.md](PACKAGING.md#releases)).

The GPU smoke, the soak, benchmarks, the Arch package check, and the packages' `--system-lifecycle` checks don't run in CI. Run them by hand before a release.

Every action the workflows use is pinned to a commit SHA, with its release in a comment (`# v7.0.1`), so a moved tag can't change what runs. To update one, take the commit of the new release's tag (`git ls-remote --tags https://github.com/OWNER/ACTION`) and change the SHA and the comment together. `dtolnay/rust-toolchain` has no releases: it is pinned to the head of its `stable` or `nightly` branch, which selects the toolchain.

### GPU render smoke

The GPU smoke runs the renderer's offscreen checks (text, emoji, ligatures, inline images, themes, background images, and a glyph-atlas stress test) on a software adapter (Mesa llvmpipe, `mesa-vulkan-drivers` on Debian and Ubuntu), then compares one frame with the visual signature in `termeric/src/renderer/visual_baseline.json`:

```bash
WGPU_BACKEND=vulkan cargo test -p termeric --locked -- --ignored gpu_render_smoke
```

Set `TERMERIC_GUI_VISUAL_OUTPUT=path.png` to save the frame, `TERMERIC_GPU_SMOKE_HARDWARE=1` to require a hardware adapter instead, and `TERMERIC_GUI_VISUAL_UPDATE=1` to rewrite the signature after an intended visual change.

### Soak

The soak repeats terminal, search, image, pane, tab, session, transcript, and diff work for a while, then fails if the test process's memory, threads, open files, or child processes grew past fixed limits. It measures the whole test process, so run it on its own, as below, not alongside other tests:

```bash
TERMERIC_SOAK_DURATION_SECONDS=60 cargo test -p termeric --locked -- --ignored scheduled_resource_soak_scenario
```

### Extra shells

CI requires Bash, Zsh, and Fish for the shell-integration tests (`TERMERIC_REQUIRE_SHELL_ACCEPTANCE=1`). Set `TERMERIC_REQUIRE_NUSHELL_ACCEPTANCE=1` or `TERMERIC_REQUIRE_ELVISH_ACCEPTANCE=1` as well to require Nushell or Elvish.

### Benchmarks

```bash
python3 scripts/run_benchmarks.py
```

The runner builds and runs the release benchmark suites, writes every metric's samples and statistics to `target/benchmarks/latest.json`, and fails if a metric exceeds its budget in `benchmarks/budgets.toml`. Use `--repeat N` to pool several runs and `--no-check` to record results without enforcing budgets. Renderer metrics need a GPU or Mesa's software Vulkan; without one they're reported as skipped, which only budgets marked `allow_skipped` accept.

### Profiling and debugging builds

Release builds are stripped, and ordinary dev builds keep only line tables. Two extra Cargo profiles fill the gaps:

```bash
cargo build --profile profiling   # release optimization with symbols, for perf or heaptrack
cargo build --profile debugging   # dev build with full debug information, for GDB or LLDB
```

The binaries land in `target/profiling/` and `target/debugging/`.

### Fuzzing

The fuzz targets live in `fuzz/` and use [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz), which needs a nightly toolchain.

```bash
python3 scripts/materialize_fuzz_corpus.py /tmp/termeric-fuzz-corpus
cargo +nightly fuzz run terminal_bytes /tmp/termeric-fuzz-corpus -- \
  -dict=fuzz/terminal.dict -max_len=65536 -max_total_time=60
cargo +nightly fuzz run domain_protocol fuzz/corpus/domain_protocol -- -max_total_time=60
cargo +nightly fuzz run graphics_protocols fuzz/corpus/graphics_protocols -- -max_total_time=60
```

`materialize_fuzz_corpus.py` turns the reviewable fixtures under `termeric-core/tests/` into binary seeds for the parser target.

### Packages

Every package ships the install tree that `scripts/stage_install_tree.py` stages (see [PACKAGING.md](PACKAGING.md)). The `.deb` packages a tree staged in `target/install-tree`:

```bash
cargo build --release --locked
rm -rf target/install-tree
python3 scripts/stage_install_tree.py --binary target/release/termeric --out target/install-tree
cargo install cargo-deb --locked
cargo deb -p termeric --no-build
python3 scripts/check_debian_package.py target/debian/termeric_*.deb

python3 scripts/build_arch_package.py --package termeric --output target/arch
python3 scripts/build_arch_package.py --package termeric-git --output target/arch
python3 scripts/check_arch_package.py target/arch/termeric-[0-9]*.pkg.tar.zst
python3 scripts/check_arch_package.py target/arch/termeric-git-*.pkg.tar.zst
```

The `termeric` Arch package is built from an archive of the committed `HEAD`, like the release tag it stands for; `termeric-git` from the working tree, uncommitted changes included.

The checkers inspect a package without installing it. Their `--system-lifecycle` mode, which needs root, installs the package, checks the installed files and `termeric --version`, and removes it again, so only use it on a disposable machine or container.

### Generated files

- `assets/man/termeric.1` and the Bash, Zsh, and Fish completions in `assets/completions/` are generated from the command description in `termeric/src/cli/spec.rs`, which also prints `termeric --help`. When you add or change a command-line option or subcommand, update that description (a test checks that the parser accepts every option it lists), then regenerate the files and commit them with `TERMERIC_UPDATE_GENERATED=1 cargo test -p termeric --locked cli::spec`. Without the variable, the same test fails while the committed files are out of date.
- `scripts/generate_icons.py` renders the hicolor icon set in `assets/icons/` from the master artwork.
- `scripts/generate_cldr_annotations.py` regenerates `termeric/src/ui/unicode_picker/cldr_annotations.bin`, the emoji name index used by the Unicode picker, from three pinned downloads (the script checks their SHA-256s): CLDR 48.0.0's [English annotations](https://raw.githubusercontent.com/unicode-org/cldr-json/48.0.0/cldr-json/cldr-annotations-full/annotations/en/annotations.json) (`--base`), its [derived English annotations](https://raw.githubusercontent.com/unicode-org/cldr-json/48.0.0/cldr-json/cldr-annotations-derived-full/annotationsDerived/en/annotations.json) (`--derived`), and Unicode 17.0's [`emoji-test.txt`](https://www.unicode.org/Public/17.0.0/emoji/emoji-test.txt) (`--emoji`). Add `--check` to compare the result with the committed index instead of writing it. CI runs only the script's unit tests, since `--check` needs the network.
- `termeric/THIRD_PARTY_NOTICES.md` is generated by cargo-about; see [Dependencies](#dependencies) for the command.

## Conventions

- **UI and themes:** read the [Style Guide](AGENTS.md#style-guide) before changing anything a person sees. Root themes are named after real roots, and chrome colors come from the active theme.
- **Commits:** use [Conventional Commits](https://www.conventionalcommits.org/) (`feat(scope): …`, `fix(scope): …`, `docs: …`, `refactor(scope): …`, `test(scope): …`, `ci: …`, `chore(scope): …`). Keep each commit to one logical change.
- **Markdown:** write each paragraph and list item on one line; don't hard-wrap prose.
- **Documentation:** there is no separate user guide. When you change behavior a person can see, update what describes it: the [README](README.md), [`assets/config.example.toml`](assets/config.example.toml) (every new setting, shortcut action, and built-in theme goes there), the command-line help in `termeric/src/cli/`, or [`examples/plugins/README.md`](examples/plugins/README.md). When a feature's support level changes, update [FEATURES.md](FEATURES.md); when a platform or install method is added or planned, update the README's [Install](README.md#install) table.
- **Changelog:** [CHANGELOG.md](CHANGELOG.md) summarizes what each release ships for the people using it, not every commit. Until 0.1.0 is released, update its `[0.1.0]` section only when a feature is added or removed or its support level changes; after that, add user-visible changes under `[Unreleased]`. Leave out internal changes such as refactors, tests, tooling, and protocol version bumps.

## Dependencies

`Cargo.lock` is committed, and builds use `--locked`. Update dependencies deliberately, in their own commit, and then:

1. Run `cargo audit` and address new advisories.
2. If the libgit2 version bundled by `libgit2-sys` changed (the part after `+` in its `Cargo.lock` version, such as `0.17.0+1.8.1`), refresh the hand-maintained libgit2 section of `packaging/notices/notices.hbs`: name the new version in its first sentence and replace the license text with that release's `libgit2/COPYING` from the crate source (`~/.cargo/registry/src/*/libgit2-sys-*/libgit2/COPYING`). The script tests fail until the notice names the version `Cargo.lock` bundles.
3. Regenerate the shipped license notices, which list every crate compiled into the binary:

```bash
cargo install --locked --features cli cargo-about
cargo about generate --locked -c packaging/notices/about.toml \
  packaging/notices/notices.hbs -o termeric/THIRD_PARTY_NOTICES.md
```

A dependency under a license that isn't in `packaging/notices/about.toml` fails generation; check that the license is compatible before adding it there.

## Reporting bugs

Open a [bug report](https://github.com/Tjemmmic/termeric/issues/new?template=bug_report.yml) or a [feature request](https://github.com/Tjemmmic/termeric/issues/new?template=feature_request.yml); the forms ask for what helps, such as your distribution, Wayland or X11, desktop, GPU and driver, and the steps to reproduce. Logs help: run `RUST_LOG=warn,termeric=debug termeric --separate-process 2> termeric.log` and attach the relevant output. Review terminal transcripts and logs for private data before attaching them. Report security problems privately, as [SECURITY.md](SECURITY.md) describes.
