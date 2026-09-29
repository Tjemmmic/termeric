# Historical Linux beta-candidate acceptance

This file records candidate-specific evidence separately from unit-test evidence. It is a historical report for the candidate named below, not a claim that the current tree is beta-ready. The authoritative current capability status and release gates live in [feature-support.md](feature-support.md).

## Current working-tree release-gate dry run

This is a non-candidate dry run recorded on 2026-09-02 from the dirty `improvements` working tree rooted at `8c127fc3483a`. A **provisional pass** means the named local evidence passes now; it cannot become a release pass until reproduced for an exact candidate SHA. A configured workflow without an accepted artifact remains **blocked**.

This historical dry run's provisional and blocked dispositions remain unchanged until an exact candidate is re-checked.

| Gate | Evidence mode | Current dry-run result |
|---:|---|---|
| 1 | Automated document contract | **Provisional pass.** The authoritative matrix contains no **Broken** capability row. |
| 2 | Automated | **Provisional pass.** The current core suite passes, including 44 conformance cases and all 28 mirrored `ansi/processor.rs` tests; the exact candidate must reproduce it. |
| 3 | Automated plus native | **Blocked.** Local workspace tests/build/Clippy and headless rendering pass, but fresh candidate-bound X11 and Wayland native artifacts do not exist. |
| 4 | Automated plus native/manual | **Blocked.** Local benchmark budgets and the ten-minute core soak pass; clean-candidate comparative, displayed-process, and vendor-resource evidence remain open. |
| 5 | Automated | **Provisional pass.** The generated terminfo contract passes Vim, less, and tmux locally; the exact candidate must reproduce it. |
| 6 | Automated, manual, unavailable, or accepted limitation | **Blocked.** Provider-free shell/integration fixtures pass, but Nushell/Elvish are unavailable here, desktop rows remain manual, and authenticated Claude/Codex checks were deliberately not run. Each absence needs evidence or an explicit accepted limitation. |
| 7 | Automated package lifecycle | **Blocked.** The fresh-host install/upgrade/uninstall workflow has no accepted candidate artifact, and synthetic-predecessor coverage does not replace a real prior-release upgrade. |
| 8 | Automated contract plus review | **Provisional pass.** README highlights map to explicit capability rows, all non-supported rows state boundaries, and local files and heading anchors resolve. |
| 9 | Manual record validation | **Blocked.** No exact candidate record yet supplies the required SHA, date, environment, tool versions, disposition, and sealed artifact paths. |

Overall result: **not release-ready**. Gates 3, 4, 6, 7, and 9 remain blocked; gates 1, 2, 5, and 8 are only provisional working-tree passes.

The 2026-08-25 source audit found default-path correctness blockers that were not detected by these smokes, including pane routing/events, application input modes, SGR rendering, ligatures, and Kitty graphics. Subsequent working-tree evidence closes the left-to-right Unicode grid-geometry, SGR attribute, and legacy application-keyboard blockers under the documented [Unicode policy](unicode-support.md), [SGR policy](sgr-rendering.md), [keyboard policy](keyboard-input.md), [ligature policy](ligature-rendering.md), and terminal-controlled mouse/focus/paste paths under the [input protocol policy](terminal-input-protocols.md), while bounded OSC title/icon queries and save/restore, OSC 99 notification negotiation/assembly, clipboard/color, and pointer-shape paths follow the [OSC policy](osc-integration.md). Visual font/fallback quality and native desktop interaction, including OSC 22 cursor-theme appearance, remain manual rows. Those findings supersede the earlier release decision below.

## Current automated core-conformance baseline

The working tree derived from `8c127fc` adds a machine-readable VT support manifest and deterministic core-conformance lane. It replays 46 focused state fixtures, mirrors all 31 tests currently declared in `ansi/processor.rs`, and includes raw traces from Vim 9.2, less 704, and tmux 3.7c. Vim and less screen state is compared byte for byte with pinned 80×24 tmux 3.7c `capture-pane` snapshots. The tmux trace is invariant-tested but is not treated as its own independent reference.

The same corpus seeds a bounded libFuzzer target. The weekly `Extended checks` workflow fuzzes it for three minutes, and failures upload replayable artifacts. The terminal target enforces both 512-row and 64-KiB-per-grid history ceilings and validates retained as well as visible row structure; deterministic coverage independently forces eviction under a 4-KiB byte budget. Details and provenance live in [terminal-conformance.md](terminal-conformance.md).

The current corpus also covers UTF-8-safe raw C1 input and terminal-local S7C1T/S8C1T response framing across core and delayed app-mediated reply paths. It adds bounded DECSCL level/C1 negotiation, soft-reset behavior, and status reporting. It does not claim strict historical level-based feature gating or unsupported C1 functions.

This conformance evidence establishes only the named parser/core behavior in the [support manifest](../termeric-core/tests/fixtures/vt_support.toml). It does not by itself establish renderer or input-path compatibility, constitute a vttest pass, or make Termeric an xterm-compatible daily driver. The later [native GUI lane](gui-acceptance.md) independently automates the named full-screen and input workflows.

## Current performance-measurement baseline

The issue #19 working tree was exercised through five complete release-suite runs on 2026-08-25. The checked-in [JSON baseline](../benchmarks/baselines/2026-08-25-linux.json) records the full environment and median/p95 values; raw 25-sample artifacts remain release/CI artifacts rather than hand-maintained prose.

| Workload | Issue #18 baseline median / p95 | Issue #19 five-run median / p95 |
|---|---:|---:|
| 4 MiB printable parser input | 48.7 / 80.9 ms | 74.0 / 93.6 ms |
| 100k styled scrollback input | 202.7 / 365.7 ms | 105.0 / 146.9 ms |
| 10k-history / 50-resize reflow | 421.8 / 460.0 ms | 500.3 / 523.2 ms |
| 100k-line incremental search | 124.0 / 133.2 ms | 83.9 / 88.7 ms |
| Offscreen 120×40 frame on llvmpipe | 2.76 / 3.01 ms | 2.51 / 2.93 ms |
| 100k scrollback process RSS delta | 325.7 / 645.1 MiB | 0* / 107.6 MiB |

The issue #19 values are 25 samples across five complete processes. Compact ownership was 108.0 MiB for more than 99,000 retained history rows. The memory win is substantial; parser/search/reflow costs remain visible and must not be hidden by the RSS result.

`*` Later samples reused allocator-retained pages and therefore had a zero RSS delta. The deterministic 108.0 MiB ownership counter and 107.6 MiB p95 RSS are the meaningful memory values; median RSS delta is not used as a safety rail.

The original displayed Vulkan run measured idle CPU at 2.50% median / 7.50% p95 of one core and idle RSS at 182.9 MiB. After the event-driven scheduler, five 400 ms samples were all 0.0% on both Wayland and X11; idle RSS was 179.9 MiB and 180.0 MiB respectively. The CPU result means less than one scheduler tick per sample, not literally zero instructions. See [event-driven-rendering.md](event-driven-rendering.md) for architecture, artifacts, diagnostics, and the remaining vendor-specific GPU check.

Those historical artifacts predate the displayed-memory contract and remain RSS-only. New artifacts retain RSS with a broad 512 MiB p95 safety rail and add context-bound Linux `smaps_rollup` PSS/private diagnostics. The new rows improve attribution but do not backfill old results or estimate vendor VRAM.

The release runner now also records five native X11 `latency.displayed_key_event_to_present` samples when `xdotool` is available. The interval begins in Winit's key callback, crosses the PTY echo/parser and real renderer, and ends after `SurfaceTexture::present`; compositor scanout and physical input delivery remain outside the measurement. No historical value is backfilled into the 2026-08-25 baseline.

The issue #21 Vulkan release run reused terminal-row geometry and capacity-managed GPU buffers. All steady samples allocated zero new pipeline or cached-row buffers, and each one-row mutation rebuilt exactly one row. A one-row update measured 0.0222 ms median / 0.0278 ms p95; eight panes with 320 unchanged rows measured 0.0043 / 0.0193 ms. The separate full 120×40 offscreen frame measured 1.050 / 1.145 ms. These are internal build/cache/upload timings, not presentation or compositor latency; method and other workloads are in [incremental-rendering.md](incremental-rendering.md).

The 2026-08-27 cooperative-search release run separated query dispatch from completion over retained history. The final bounded-index run measured 9.02 ms median / 9.58 ms p95 at 10k, while keyboard dispatch measured 0.155 / 0.276 ms. The 100k scan measured 89.13 / 92.67 ms, while dispatch remained 0.176 / 0.198 ms. Thus total first-scan work remains linear, but it is consumed in bounded render-frame slices instead of one keyboard callback. Extending a completed broad match used the prior row vector without copying it and measured 0.034 / 0.045 ms at 10k and 0.040 / 0.041 ms at 100k after the duplicate range index was removed. The 800,000-occurrence dense workload completed in 10.92 / 11.61 ms and reserved 3,211,264 bytes for recent occurrences plus all 10,000 matching rows. All search completion, dispatch, extension, dense-memory, and scale budgets passed; the local artifact is `target/benchmarks/search-bounded-memory.json`.

The issue #22 release fixture interleaved 1,000 transcript input/output entries against an injected 2 ms writer. Producer work measured 0.0909 ms median / 0.0996 ms p95 and explicitly shed 984 entries per sample at the bounded queue, rather than blocking for sink latency or growing memory. Generation/cwd tests also reject stale Git results after a blocked scan. This is deterministic worker evidence, not a long-duration real-filesystem soak; architecture and failure behavior are in [background-io.md](background-io.md).

## Current resource-soak baseline

The issue #23 working tree adds a Linux process-level soak which repeatedly feeds mixed ASCII/Unicode/styled output, resizes and searches bounded scrollback, replaces images, creates and closes real PTY-backed panes/tabs, saves sessions, and durably closes bounded transcript writers. Its runner samples RSS, threads, file descriptors, and children, and scans a companion GPU smoke log for validation/device-loss errors. The current schema-4 core record seals that log, binds the tested binary and exact clean source candidate, and is independently validated by reconstructing every check and summary. The separate schema-6 displayed report seals an exact bounded artifact manifest and reconstructs transcript, log, resource, outcome, and process-attributed DRM memory checks. Missing optional driver fields remain explicit rather than zero. Both reports reconcile privacy-safe descriptor/thread/child inventories and summarize category quartile growth and peaks without retaining paths, arguments, or environment data.

Both lanes now launch from a pinned Linux executable descriptor and retain initial, ready, launch, and final provenance. A changed checkout/build, missing final identity, or incomplete launch inventory is a failed record, not a new candidate label. Older core/native schemas do not satisfy this stronger contract. These are observed-drift checks, not signed compiler attestation; the displayed binary still needs to be built for the selected candidate. See [the launch contract](resource-soak.md#launch-provenance-contract).

A 30-second local change-gate run plus headless Vulkan smoke on 2026-08-25 completed 322 iterations and all ten configured checks. It peaked at 33.9 MiB RSS, measured 7.0 MiB first-to-last-quartile RSS growth, retained 16.0 MiB of bounded scrollback and 512 KiB of images, ended with zero quartile-comparison thread/fd growth, and emitted a clean completion marker. This is short implementation evidence, not the required ten-minute candidate run or a visible-window/vendor GPU soak. The method and exact ceilings are in [resource-soak.md](resource-soak.md).

The current fail-closed runner was captured again on 2026-09-02 at `target/soak/issue-23-current-core.json`. Across 61 samples and 524 iterations, it peaked at 42,900 KiB RSS, grew 4,680 KiB between the first/last quartile medians, showed zero thread/fd growth, accepted every transcript entry with no drops, drained every queue to zero, cleaned up all children, and reported no GPU validation/device error. All 18 checks passed; the ten-minute and displayed candidate rows remain open.

A local full-duration core run also passed at `target/soak/issue-23-current-core-10m.json`: 1,200 samples and 9,160 iterations with 42,996 KiB peak RSS, 244 KiB quartile growth, zero thread/fd growth, zero transcript drops or residual queue bytes, complete child cleanup, and zero GPU errors. It was produced from the dirty roadmap tree before the report embedded dirty-state and binary SHA-256 provenance, so it does not replace a clean scheduled-candidate artifact.

The repository also configures a 30-second and scheduled ten-minute private- Sway displayed-process soak. It continuously renders real PTY output, resizes the compositor output, churns split panes with exact routing, samples the application PID, and checks normal native close plus complete child cleanup. Clean `bd70faa` supplies a validated schema-6 ten-minute AMD/radeonsi OpenGL observation with required DRM total/resident memory in all 877 samples, bounded RSS/GPU memory, and complete child cleanup. It supplements the local software- renderer observations and older `a53d097` aggregate record described in [resource-soak.md](resource-soak.md). Accepted scheduled-CI history, a longer real-desktop observation, other adapter/backend coverage, and multi-day stability remain open; one private-Sway run is not general driver qualification.

## Current Debian package-lifecycle gate

The working tree now replaces archive-only shell checks with `scripts/check_debian_package.py`. Read-only inspection validates populated `dpkg-shlibdeps` output, exact file types/modes, desktop launch/action fields, the PNG icon, and the compiled terminfo entry. A guarded CI-only mode refuses non-Debian systems, missing explicit opt-in, rootless execution, and any host where Termeric was already installed.

On a disposable Ubuntu runner, that mode installs a synthetic older package, upgrades it to the unmodified candidate artifact with APT, runs only the non-GUI `termeric --version` probe, removes it, rejects owned-file residue, and checks that synthetic user configuration remains unchanged. The synthetic package has current payload and metadata with only its Debian version lowered, so this proves package-manager lifecycle and cleanup but not migration from an actual prior Termeric release. The uploaded schema-1 report binds the result to the artifact's SHA-256 digest, size, package metadata, candidate SHA, and UTC recording time.

This Arch evidence host lacks `dpkg-deb`, so only the seven pure checker fixtures have been run locally. The Debian lifecycle remains unaccepted until its CI lane produces a green artifact; no Debian pass is inferred from workflow configuration.

## Current Arch package-lifecycle gate

The working tree also contains a maintained `termeric-git` PKGBUILD, checked `.SRCINFO`, and an exact-checkout staging helper. A read-only checker has passed locally against a real x86_64 archive derived from the current local Git revision, including exact payload/mode checks, declared runtime dependencies, desktop/PNG/terminfo validation, dynamic-library resolution, and `termeric --version`. Adversarial fixtures cover unsafe archive paths and types, duplicates, unexpected/missing members, metadata drift, and mode regressions.

The separate lifecycle mode fails closed without explicit opt-in, root, an Arch-family host, and an initially absent `termeric-git`/`termeric` package. On 2026-09-11, exact commit `cbdc12c46cd1f99c32e4ca98a18f7a23b36035d0` was built as an unprivileged user in the pinned official `archlinux:base-devel-20260906.0.587075` container. The resulting `termeric-git-0.1.0.r164.gcbdc12c-1-x86_64.pkg.tar.zst` archive passed the read-only contract and guarded synthetic-predecessor install, candidate upgrade, non-GUI version probe, removal, owned-file cleanup, and user-state retention. Its 11,611,118-byte artifact has SHA-256 `873e77129a9796281df90c0fc7479bbbbaf5ae6857086a4e01e45a0f0a72f3ab`. The configured hosted lane still needs its first uploaded green artifact, and this evidence does not claim migration from a real prior release.

## Candidate and environment

- Candidate branch/commit at start: `improvements` / `8276753`
- Date: 2026-08-12
- Host session: Wayland with an Xwayland display available
- GPU: AMD Radeon RX 7900 XTX (RADV NAVI31), Vulkan
- Shell available and exercised by the app: Bash
- Fonts discovered by the app: 5 monospace families; system includes Hack, Liberation Mono, FreeMono, Adwaita Mono, and Noto emoji coverage
- External CLIs detected: Claude Code 2.1.228; Codex CLI 0.137.0

## Automated and agent-driven evidence

| Area | Result | Evidence |
|---|---|---|
| Wayland launch | Pass | Isolated 8-second release launch created a window, selected the discrete Vulkan GPU, loaded Noto Sans Mono, initialized a 98x32 grid, and spawned Bash without an error log. |
| X11 launch | Pass | Equivalent isolated release launch passed against `DISPLAY=:1`. |
| X11 input/resize smoke | Pass, non-visual | Window-targeted input sent Unicode/combining text, ANSI color and 5,000 output lines; opened a second tab; invoked search/settings/font shortcuts; switched tabs; and resized from 1100x700 to 900x600. No application error was logged. |
| Normal X11 close | Pass | Window-manager Alt+F4 produced `Close requested`, unloaded plugins, and exited with status 0. |
| Abrupt X11 drawable loss | Pass | Acceptance initially found an XGetGeometry/WGPU panic and redraw storm. The candidate now pauses redraws after one surface-loss notice and waits for a real resize; forced drawable destruction no longer panics or spins. |
| Headless GPU render | Pass | Vulkan WGPU test selected an adapter and submitted a render pass with `TERMERIC_GPU_SMOKE=1`. |
| Shell semantics | Partial | Real Bash PTY/OSC 133/Harvest fixture passes. Zsh and Fish binaries are not installed on this host. |
| Protocol/core behavior | Partial (automated) | ANSI parsing, Unicode-safe classic CSI editing/tabulation, DECLRMM/DECSLRM rectangular margins, VT400 DECCRA/DECFRA/DECERA/DECSERA and DECSACE/DECCARA/DECRARA operations, opt-in/default-deny DECRQCRA/DECCKSR checksums, DECRQPSR/DECRSPS cursor-and-tab round trips, G0-G3 single/locking shifts, protected DEC selective erase, complete supported DECSC/DECRC state, non-destructive DECSTR soft reset, OSC 8/22/66/133 parsing, bounded screen-local OSC 22 pointer stacks/queries with exhaustive native icon mappings, bounded Kitty OSC 66 explicit widths and integer/fractional multicell geometry with edit/history/session/domain replay tests, reflow, alt-screen behavior, bounded static Kitty/Sixel/iTerm2 profiles, DECRQSS SGR/margin/cursor/protection/attribute-extent/legacy-geometry/XTFMTKEYS replies, DECRQM live-mode reports, application cursor shape/blink/visibility, search, pane layout, sessions, plugin quick-exit paths, left-to-right Unicode geometry, SGR attributes, legacy/xterm, exact xterm resource-zero negotiation and runtime/configured legacy/VT220 keyboard types with shared private-mode save/restore and bounded DECUDK dispatch plus bounded modifyCursorKeys/modifyFunctionKeys/modifyKeypadKeys, encoder-neutral reserved modifyStringKeys state, modifyModifierKeys/modifySpecialKeys/modifyOtherKeys with CSI-27/CSI-u formatting, Kitty progressive keyboard encoding, and focused-pane mouse/focus/paste byte streams have automated coverage. The native lane runs real `img2sixel` and `timg -pi`; the headless GPU lane submits image geometry and Unicode/SGR pixels. Visual quality for every installed font/fallback, native OSC 66 scaled-font/clipping/cursor appearance, native compositor mouse interaction and OSC 22 cursor-theme appearance, visible `kitten icat`, and native Kitty-keyboard client behavior are not established. |
| Plugin executable contracts | Partial | Installed Codex parses open and `resume --last` vectors without authentication; Claude/Codex authenticated interactive GUI workflows were not executed. |
| Packaging | Partial | Clean-index crate archives and Debian lifecycle remain CI-gated. Exact Arch candidate `cbdc12c` passes package construction, read-only inspection, and the disposable Pacman lifecycle in the pinned official container; hosted artifact history, publication, and a real prior-release migration remain absent. |

Candidate rows use four explicit dispositions so an absent environment cannot silently become a pass:

| Disposition | Current scope |
|---|---|
| Automated | Workspace quality, conformance/fuzz, headless GPU, native X11/Wayland acceptance, performance budgets, Debian/Arch archive checks, short/scheduled Linux resource soak, and macOS/Windows build/unit plus bounded native-PTY lifecycle CI. |
| Manual | Real-compositor visual quality and policy, SGR font/decorations and fallback quality, vendor-GPU behavior, assistive technology, audible output, and authenticated plugin workflows. |
| Unavailable | Shells, tools, hardware counters, accounts, or native runners absent from the evidence host; record the exact absence. |
| Accepted limitation | An explicitly approved partial/experimental/unsupported boundary allowed by the release gates; never re-label it as a pass. |

The authoritative platform tiers and promotion rules are in [platform-support.md](platform-support.md).

## Performance baseline and budgets

Initial beta regression budgets are intentionally conservative:

- GUI startup with a short-lived `/bin/true` shell: median under 0.5 seconds.
- 100,000 styled terminal lines / 4 MiB input: under 3 seconds.
- 200 repeated large-history reflows: under 3 seconds.

Measured on the environment above:

| Workload | Measurement |
|---|---:|
| Wayland startup, 5 samples | median 0.110s; 0.099–0.115s |
| X11 startup, 5 samples | median 0.098s; 0.094–0.102s |
| 100,000 styled lines / 4.0 MiB | 0.356s final gate run |
| 200 reflows after 10,000 seeded lines | 2.129s final gate run |

The short and local ten-minute core process soaks are measured. Both configured ten-minute CI candidate rows still need accepted clean artifacts, and a real-desktop/vendor-GPU observation remains required before a broad resource-stability release claim.

## Pending automated rows

The rows below are not manual substitutes. Their checked-in lanes must produce fresh candidate-bound artifacts before promotion:

- The private-Sway keyboard/copy/paste/pane interaction lane, including lost/outdated-surface and device-loss recovery before continued PTY input.
- Required Zsh and Fish editing plus generated shell-hook execution in the normal CI shell matrix.
- Private-X11 and private-Wayland left-tab scenarios covering F6 controls, overflow/reordering, shifted terminal/pane/search/image geometry, and restoration to top tabs.
- Static PNG/JPEG background scenarios covering all fit modes, opacity bounds, top/left tabs, panes/chrome/resize, replacement failure, renderer recovery, and memory ceilings.
- The configured ten-minute core and private-Sway displayed-process soaks.
- The configured macOS and Windows build/unit plus native-PTY lifecycle jobs; the checked-in wiring is not accepted green runner history.
- Fresh-host Debian install, synthetic-predecessor upgrade, non-GUI probe, uninstall cleanup, and user-state retention, including its generated dependency field verified by `dpkg-shlibdeps`; the configured Arch lane also still needs its first hosted artifact even though its exact local container reproduction passes.

## Required manual rows still open

New Linux candidate evidence for the rows below must use the validated native acceptance record; prose or source inspection alone cannot promote a row.

Shell, Harvest, desktop-integration, fake-provider, and authenticated-provider results additionally use the candidate-bound record in integration acceptance. Authenticated `pass` or `fail` results are invalid without explicit approval metadata and redacted, secret-reviewed, SHA-256-sealed artifacts; the record tool itself never runs a provider.

These cannot be truthfully inferred from synthetic input or source inspection:

- Visual inspection on real Wayland and X11 compositors for scale-factor changes, resize quality, wide/combining/emoji fallback, ligatures, selection shapes, image quality, and vendor-specific presentation. Deterministic X11 workflows, modal hit targets, Vim/less/tmux, and surface recovery now run in the [native GUI lane](gui-acceptance.md).
- Native CJK IME and dead-key composition in single and split panes, color and ZWJ emoji quality/cache-pressure observation, and Orca/VoiceOver/Narrator verification of the AccessKit tree. See [modern input and accessibility](accessibility-and-input.md).
- Real-compositor background-image quality and contrast review after the automated functional cases listed above. See [background images](background-images.md).
- The `vttest` keyboard screen on a host that provides it.
- Native Kitty-keyboard client interoperability, including platform lock-state and uncommon Hyper/Meta modifier behavior.
- Visible current-`kitten icat` image quality and replies on Wayland and X11, using both `--transfer-mode=stream` and `--unicode-placeholder --transfer-mode=stream` (including transparent pixels, reflow, scrollback, overwrite, and alternate-screen cases), plus a direct `P/Q/H/V` probe covering ordinary and Unicode parents, signed offsets, parent movement, chains, and cascading deletion, stdin-backed `--transfer-mode=file`, and `--transfer-mode=memory` (the native harness invokes both and checks object cleanup, but visible judgment remains operator-owned); test an existing path in file mode first with `allow_kitty_file_transfers = false` (expect denial), then with it set to `true` (expect a visible image and the source file to remain unchanged). Restore the default-disabled setting after the test.
- Clipboard round trips, URL launching, visual/audio bell, and desktop notifications under the user desktop's permission/policy environment.
- Subjective settings/theme UX, divider drag feel, and malformed/cap messaging; deterministic tab/pane/search/settings input paths are automated.
- Authenticated Claude and Codex open/resume/restart/context/diff/transcript/ profile flows; these may consume network/service state, require the release operator's accounts and prior approval, and must be captured in the versioned integration record rather than edited into this historical prose.
- A longer real-desktop visible-window and vendor-GPU-memory observation where supported; the deterministic ten-minute software lanes are listed above.

## Historical release decision

**Not ready to merge or tag.** Automated gates are strong and both display backends launch, but the required visual, shell-matrix, authenticated-plugin, fresh-machine package, and soak rows remain open. Keep `improvements` as the release-candidate line. Once those rows pass or are explicitly accepted as beta limitations, fast-forward `main`, wait for green CI on `main`, then create an annotated `v0.1.0-beta.1` tag from that exact commit.

That decision is now additionally blocked by every default-path **Broken** row in [feature-support.md](feature-support.md). A new candidate must produce a new dated evidence section rather than editing these historical measurements in place.
