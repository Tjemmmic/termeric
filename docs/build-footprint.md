# Cargo build footprint

Termeric keeps ordinary development builds source-attributable without putting
full debugger data into every artifact. The workspace `dev` profile uses
Cargo's `line-tables-only` debug level, and the `test` profile inherits that
setting. File and line information remains available in backtraces.

Use the ordinary profile for editing, tests, and routine diagnosis:

```bash
cargo build --workspace --locked
cargo test --workspace --locked
```

When GDB, LLDB, or another debugger needs local variables and complete type
information, opt into the full-debug profile explicitly:

```bash
cargo build --workspace --profile debugging --locked
gdb target/debugging/termeric
```

The `debugging` profile has its own output directory. Building it is an
intentional disk/diagnostic tradeoff and does not alter release output.
Incremental compilation remains at Cargo's local-development default for both
profiles because fast edit/build cycles are useful. CI or one-off evidence
runners may set `CARGO_INCREMENTAL=0` when reuse is not valuable.

## Inspect before cleaning

The footprint reporter uses only Python's standard library and accepts the
target directory explicitly:

```bash
python3 scripts/build_footprint.py target
python3 scripts/build_footprint.py target --json
```

It sums logical byte lengths for regular files and reports exact regular-file
counts by Cargo profile and stable artifact class. It does not follow or count
symlinks. Incremental state, dependencies, build-script output, metadata,
examples/benches, documentation, and top-level products remain distinct. The
largest regular files are included to make symbol-heavy executables or test
binaries obvious.

Optional ceilings turn the diagnostic into a caller-selected check:

```bash
python3 scripts/build_footprint.py target \
  --warn-bytes 26843545600 --max-bytes 32212254720 \
  --warn-files 40000 --max-files 50000
```

Exceeding a warning limit reports `warning` and returns status 0; exceeding a
maximum reports `fail` and returns status 1. A warning cannot be higher than
its matching maximum. Invalid, missing, non-directory, unreadable, or racing
paths return status 2 instead of producing a misleading zero-sized pass. No
limit is imposed by default because an accumulated local directory is host and
workflow state, not clean-candidate evidence.

The byte total is deterministic logical size, so hard-linked files count at
each visible path and sparse files contribute their stated length. It is not
the filesystem's allocated-block count and must not be confused with runtime
RSS. [`resource-soak.md`](resource-soak.md) owns runtime process-growth evidence.

## Cleanup is explicit

The reporter never removes anything. After inspection, Cargo can narrowly
remove one package or profile when appropriate:

```bash
cargo clean -p termeric
cargo clean --profile debugging
```

Use a full `cargo clean` only when discarding all workspace artifacts is the
intended result. All of these outputs are reproducible but rebuilding the full
GPU, font, image, and desktop dependency graph is expensive.

## Same-host profile comparison

On 2026-09-12, Rust 1.92.0 and Cargo 1.92.0 built commit `be819f3` on the same
x86_64 Linux host with this command and a new empty target directory:

```bash
CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/tmp/termeric-footprint-TAG \
  cargo build --workspace --locked
python3 scripts/build_footprint.py /tmp/termeric-footprint-TAG --json
```

The former full-debug default took 1m 41s and produced 5,405,971,758 logical
bytes across 4,600 regular files; `debug/termeric` was 663,693,400 bytes. The
line-table default took 1m 32s and produced 3,219,011,809 logical bytes across
the same 4,600 files; `debug/termeric` was 228,192,784 bytes. That is a 40.5%
logical-footprint reduction overall and a 65.6% reduction for the executable.
These measurements compare profile output on one host; they are not portable
budgets or runtime-performance claims.

| Default dev debug data | Build time | Logical bytes | Regular files | `termeric` bytes |
|---|---:|---:|---:|---:|
| Full (`debug = 2`) | 1m 41s | 5,405,971,758 | 4,600 | 663,693,400 |
| Line tables only | 1m 32s | 3,219,011,809 | 4,600 | 228,192,784 |
