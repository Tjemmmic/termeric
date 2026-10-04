# Contributing to Termeric

Thanks for your interest in Termeric. This page covers building from source, the checks a change needs to pass, and the conventions the project follows.

## Building

You need a stable Rust toolchain (1.89 or newer) and the development packages for fontconfig, Wayland, xkbcommon, XCB, and Vulkan; [Build from source](docs/src/installation.md#build-from-source) lists them for Debian, Ubuntu, and Arch Linux. Then:

```bash
cargo build --locked
cargo run --locked
```

The workspace has two crates: `termeric/` is the application (windowing, rendering, UI, agents, plugins) and `termeric-core/` is the terminal emulation library (parser, grid, PTY). [AGENTS.md](AGENTS.md) describes the architecture and the rules that keep input, PTY output, and panes routed correctly. Read it before changing anything in `app/`, `renderer/`, or `tabs/`.

## Before you send a change

Run the core checks CI runs on every pull request:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
python3 scripts/check_markdown_links.py
```

[docs/dev/testing.md](docs/dev/testing.md) covers the rest: the helper-script tests, the GPU render smoke, the resource soak, benchmarks, fuzzing, package checks, and what CI runs.

Add tests with your change. A bug fix should come with a test that fails without it.

### Manual page and shell completions

`assets/man/termeric.1` and the Bash, Zsh, and Fish completions in `assets/completions/` are generated from the command description in `termeric/src/cli/spec.rs`, which also prints `termeric --help`. When you add or change a command-line option or subcommand, update that description (a test checks that the parser accepts every option it lists), then regenerate the files and commit them:

```bash
TERMERIC_UPDATE_GENERATED=1 cargo test -p termeric --locked cli::spec
```

Without the variable, the same test fails while the committed files are out of date.

## Conventions

- **UI and themes:** read [docs/dev/style-guide.md](docs/dev/style-guide.md) before changing anything a person sees. Root themes are named after real roots, and chrome colors come from the active theme.
- **Commits:** use [Conventional Commits](https://www.conventionalcommits.org/) (`feat(scope): …`, `fix(scope): …`, `docs: …`, `refactor(scope): …`, `test(scope): …`, `ci: …`, `chore(scope): …`). Keep each commit to one logical change.
- **Markdown:** write each paragraph and list item on one line; don't hard-wrap prose.
- **Documentation:** the user guide is an mdBook in `docs/`. Update the relevant page when you change behavior a person can see, and update [Feature status](docs/src/status.md) when a feature's support level changes.
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

Open an issue with your distribution, desktop (Wayland compositor or X11 window manager), GPU and driver, and the steps to reproduce. Logs help: run `RUST_LOG=termeric=debug termeric` and attach the relevant output. Review terminal transcripts and logs for private data before attaching them.
