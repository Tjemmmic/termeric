# Testing

Most of Termeric's tests are ordinary Rust unit and integration tests that run anywhere. A smaller set needs a real GPU, a compositor, or several minutes, and runs by hand. Helper scripts in `scripts/` drive those.

## Everyday checks

Run these before sending a change. They are what CI runs on every pull request.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
python3 -m unittest $(git ls-files 'scripts/test_*.py' | sed 's|/|.|; s|\.py$||')
python3 scripts/check_markdown_links.py
python3 scripts/check_terminfo.py --require-apps
```

`check_terminfo.py --require-apps` compiles `assets/terminfo/xterm-termeric.terminfo` and runs Vim, less, and tmux against it, so those three need to be installed. The Python tests cover the helper scripts themselves and need Python 3 with Pillow (`python3-pil` on Debian and Ubuntu, `python-pillow` on Arch).

To build the user guide, install [mdBook](https://rust-lang.github.io/mdBook/) and run `mdbook build docs`; the output goes to `target/book/`.

## CI

`.github/workflows/ci.yml` runs the everyday checks above, plus the user-guide build, on pull requests and pushes to `main`. It sets `TERMERIC_REQUIRE_SHELL_ACCEPTANCE=1`, so the Bash, Zsh, and Fish integration tests fail instead of skipping when one of those shells is missing; set it locally to match.

`.github/workflows/fuzz.yml` runs the three fuzz targets (see [Fuzzing](#fuzzing)) every Monday against `main`, and whenever it's started from the Actions tab.

The native GUI tests, soaks, benchmarks, and package checks don't run in CI. Run them by hand before a release.

## Native GUI tests

`scripts/gui_acceptance.py` launches the built application inside a private compositor and drives it with synthetic input. Every run needs `tic`, `fc-list`, and Pillow. Wayland scenarios need `sway`, `swaymsg`, `wtype`, and `wl-clipboard`, plus `weston` for `--start-weston`; X11 scenarios need `xvfb-run`, `xdotool`, `xclip`, and `openbox`. The `graphics` scenario also needs `kitten` (from kitty), `img2sixel`, and `timg`, and the `vim`, `less`, and `tmux` scenarios need those programs. The script names anything missing before it starts.

```bash
cargo build -p termeric --locked
WGPU_BACKEND=gl python3 scripts/gui_acceptance.py --backend wayland --start-sway \
  --scenario interaction --skip-visual-checks --font-family 'Noto Sans Mono' \
  --binary target/debug/termeric --artifacts target/gui-acceptance/wayland
```

Repeat `--scenario` to run several; run the script with `--help` for the full list. The default font family is DejaVu Sans Mono; pass `--font-family` with an installed family otherwise. Logs, PTY transcripts, and screenshots land in the artifacts directory, and `results.json` records each scenario's result.

The headless render smoke runs the renderer's offscreen checks (text, emoji, ligatures, inline images, themes, background images, and a glyph-atlas stress test) on a software adapter (Mesa llvmpipe, `mesa-vulkan-drivers` on Debian and Ubuntu), then compares the frame with Termeric's visual signature:

```bash
TERMERIC_GPU_SMOKE=1 TERMERIC_GUI_VISUAL_OUTPUT=$PWD/target/gui-acceptance/renderer.png WGPU_BACKEND=vulkan \
  cargo test -p termeric --locked linux_headless_gpu_render_smoke_when_requested -- --nocapture
python3 scripts/check_gui_visual.py target/gui-acceptance/renderer.png
```

## Soaks

The soak runners look for resource leaks by running a busy workload while sampling the process through `/proc`. They print every check and exit non-zero if any limit is exceeded.

```bash
WGPU_BACKEND=vulkan python3 scripts/run_soak.py --duration-seconds 30 --gpu-smoke \
  --output target/soak/local.json --log target/soak/local.log

cargo build -p termeric --locked
WGPU_BACKEND=gl python3 scripts/run_native_gui_soak.py --duration-seconds 30 \
  --binary target/debug/termeric --artifacts target/native-gui-soak
```

`run_soak.py` runs the headless workload. `run_native_gui_soak.py` runs the displayed application in headless Sway while it resizes the output and opens and closes panes; it needs `sway`, `swaymsg`, `tic`, and `wtype`, and an artifacts directory that is new or empty. Run `--help` on either for the adjustable limits.

## Integration runners

`scripts/run_shell_acceptance.py --output DIR` runs the shell-integration test against the real Bash, Zsh, and Fish on your `PATH`; add `--include-extended-shells` to require Nushell and Elvish as well. `scripts/run_fake_provider_acceptance.py --output DIR` runs every agent command against fake Claude Code and Codex executables. Neither needs an account or network access. Both write `results.json` and their logs to `DIR`.

## Benchmarks

```bash
python3 scripts/run_benchmarks.py --skip-displayed \
  --baseline benchmarks/baselines/2026-08-25-linux.json
```

The runner builds and runs the release benchmark suite, writes `target/benchmarks/latest.json`, and fails if a metric exceeds its budget in `benchmarks/budgets.toml`. `benchmarks/metrics.toml` defines each metric. Use `--no-check` to record results without enforcing budgets.

## Fuzzing

The fuzz targets live in `fuzz/` and use [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz), which needs a nightly toolchain.

```bash
python3 scripts/materialize_fuzz_corpus.py /tmp/termeric-fuzz-corpus
cargo +nightly fuzz run terminal_bytes /tmp/termeric-fuzz-corpus -- \
  -dict=fuzz/terminal.dict -max_len=65536 -max_total_time=60
cargo +nightly fuzz run domain_protocol fuzz/corpus/domain_protocol -- -max_total_time=60
cargo +nightly fuzz run graphics_protocols fuzz/corpus/graphics_protocols -- -max_total_time=60
```

`materialize_fuzz_corpus.py` turns the reviewable fixtures under `termeric-core/tests/` into binary seeds for the parser target.

## Packages

```bash
cargo install cargo-deb --locked
cargo deb -p termeric --locked
python3 scripts/check_debian_package.py target/debian/termeric_*.deb

python3 scripts/build_arch_package.py --output target/arch
python3 scripts/check_arch_package.py target/arch/termeric-git-*.pkg.tar.zst
```

The checkers inspect a package without installing it. Their `--system-lifecycle` mode installs, upgrades, and removes the package, so only use it on a disposable machine or container.

## Generated files

- `scripts/generate_icons.py` renders the hicolor icon set in `assets/icons/` from the master artwork.
- `scripts/generate_cldr_annotations.py` regenerates the emoji name index used by the Unicode picker from pinned CLDR data.
- `termeric/THIRD_PARTY_NOTICES.md` is generated by cargo-about; see [Contributing](../../CONTRIBUTING.md) for the command.
