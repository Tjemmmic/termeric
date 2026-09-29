# Resource soak testing

Termeric has a process-level Linux soak lane for detecting unbounded memory, thread, file-descriptor, child-process, transcript-queue, and GPU-error growth. It complements the short deterministic benchmarks; it does not turn a short CI run into proof of multi-day stability.

Every `/proc` sample also retains privacy-safe diagnostic inventories. File descriptors use a fixed type vocabulary rather than targets, while bounded sanitized thread and direct-child `comm` labels are aggregated by count. No descriptor path, command line, argument, or environment value enters the artifact. Each inventory must reconcile exactly with its sample's aggregate counts or the run fails its coverage check.

A second runner measures the actual displayed Termeric process under a private headless Sway compositor. The lanes are complementary: the core scenario drives broader deterministic storage/session operations, while the native scenario includes Winit, a compositor connection, a WGPU surface and renderer caches, real PTYs, and repeated output-size and pane-lifecycle changes. On Linux it also samples process-attributed memory exported through standardized DRM fdinfo fields when the selected driver provides them.

## Scenario

The ignored `scheduled_resource_soak_scenario` test repeatedly exercises one long-lived terminal while creating and tearing down real PTY-backed panes and tabs. Each iteration includes:

- high-volume ASCII, styled, CJK, combining-mark, and emoji-ZWJ output;
- alternating terminal resizes, scrollback eviction, and full-history search;
- bounded raw image storage and replacement;
- one persistent diff history, repeatedly filled past its count and owned-byte ceilings with fragmented synthetic UTF-8 patches;
- three-pane and four-tab PTY creation, output, exit, and cleanup;
- replacement of an on-disk session snapshot; and
- bounded asynchronous transcript input/output, durable shutdown, and file removal.

The scenario asserts its internal line/byte/image limits and clean PTY exits. The runner independently samples the test process through Linux `/proc` and checks peak RSS, first-to-last RSS/thread/fd growth, concurrent children, exit status, final child cleanup, completion output, and WGPU validation/device-loss messages. Child sampling aggregates and deduplicates every `/proc/<pid>/task/<tid>/children` file so PTYs or helpers spawned by Rust worker threads cannot evade the process ceiling or cleanup check.

The same enumerated task, descriptor, and child snapshots produce both totals and their diagnostic categories. Disappearing `/proc` entries are counted as `unreadable` rather than omitted. Each report summarizes the first- and last-quartile median, growth, and peak for every observed descriptor kind, thread name, and child name. Category summaries diagnose an aggregate failure; they intentionally have no host-specific pass/fail ceilings of their own. Each label is at most 32 characters and each dimension retains at most 64 labels; additional names are combined into a deterministic `<other-labels>` bucket.

Every iteration also reports the real byte backlog held by the bounded background transcript channel before shutdown, the accepted and dropped entry counts, and the remaining backlog after its durable flush. The runner requires all 68 input/output entries to be accepted, no drops, no more than 2 MiB queued by default, and exactly zero bytes after every iteration finishes. Missing or malformed queue fields fail instead of being interpreted as zero pressure. Use `--max-transcript-queue-mib` to tighten the peak backlog rail for a controlled runner class; it cannot disable the no-drop or complete-drain requirements.

Both runners require at least two samples and at least half of the nominal `duration / sample_interval` cadence. A stalled or misconfigured sampler can no longer report zero growth from a single point and pass the memory-stability gate.

Core schema **5** adds sustained diff-history evidence. Each iteration feeds 64 small and 16 roughly 200-KiB patches through one detector retained for the entire run, splitting headers and UTF-8 scalars across 257-byte fragments. All 80 patches must complete once with monotonic identities, old IDs must be evicted, and the workload must reach both 64 retained patches and at least 3 MiB of owned history before eviction. Sampled retention may never exceed 64 patches or 4 MiB. The diagnostic recomputes string/vector capacities and checks the detector's accounting rather than trusting its byte counter alone.

Every progress record contains completed, evicted, retained, peak-count, and peak-owned-byte metrics. The runner and independent validator require full coverage, exact workload completion, both pressure thresholds, bounded peaks, and per-iteration retention/eviction reconciliation against the previous iteration. Missing/type-confused fields, skipped pressure, weakened fixed thresholds, or a resealed over-limit record cannot pass. Schema-4 and earlier core evidence remains historical and is rejected as current-schema evidence; it cannot establish this newer workload. The displayed schema remains 6.

## Running it

### Launch provenance contract

The provenance policy introduced in **4 core / 6 displayed**, and retained in current core schema 5, fixes the reproduced launch-attribution defect in [#108](https://github.com/Tjemmmic/termeric/issues/108). Earlier core records read Git identity and hashed the executable pathname after the scenario; older displayed records also hashed that pathname after execution. A checkout/rebuild could relabel a run with bytes that were not executed. A private reproduction demonstrated this acceptance before the fix; those old schemas are now rejected by the current validators, not silently upgraded into stronger evidence.

Both runners now use one read-only, nonblocking, no-follow, close-on-exec Linux descriptor for a regular executable of at most **2 GiB**. Its length, SHA-256, device/inode, and nanosecond modification/change times are measured through that descriptor, using at most 1 MiB read chunks and checks for changes during measurement. Children execute through the runner's `/proc/<pid>/fd/<fd>` path, not the original build pathname; they do not inherit the descriptor. Pathname replacement therefore cannot substitute another executable after a launch is prepared, and core GPU smoke and the sustained scenario use the same pinned object. No extra executable copy or accumulating staging directory is created. The runner closes its descriptor on normal return, launch/setup failure, timeout, and interruption. Restricted `/proc` execution fails; there is no fallback to launching an unmeasured pathname.

An unavailable `/proc` sample does not terminate a still-live core scenario. In particular, an exited zombie/dead process can lose `VmRSS` before the next child-status poll. The sampler reports that observation as unavailable, then the runner polls again; it does not invent a zero-RSS sample. Missing memory fields on a live process remain errors, and minimum sample coverage still applies.

Core source identity/cleanliness is captured **before building**, checked again after the binary is measured, recorded at each launch, and checked after the run. Displayed runs apply the same checks around their supplied prebuilt binary and native setup/launch. A changed source refuses a later launch; a final source change, original-path replacement, pinned-object change, missing final identity, or incomplete launch inventory makes the provenance checks fail. The initial source and measured binary remain the record's identity rather than being replaced by a newer checkout or file digest. Both validators check the bounded, typed provenance fields, their linkage to the main record, and reconstruct all three provenance checks. Dirty-but-unchanged runs remain ineligible diagnostic runs; observed drift makes the record failed. The `launches` entries record measured attempts, including refused or failed attempts; their presence does not claim that a process started or completed. The separate exit-status and workload checks must also pass.

Keep source and build outputs unchanged throughout a run. Checks detect **observed drift** in a trusted runner, not arbitrary malicious same-user edits that are completely restored between observations. This is not signed compiler provenance; in particular, the displayed runner's supplied binary must still be built for the chosen candidate using the documented workflow. It pins exactly that supplied executable, not proof of which source compiled it.

Failures after scenario setup are retained as diagnostic records. A missing or replaced original source/binary can prevent independent validation even with `--allow-ineligible`; that flag never waives the validator's external identity checks. Constructor/build/pre-artifact failures abort without claiming a run. Neither runner overwrites an existing evidence destination. Historical measurements below remain observations under their stated older schemas.

### Commands

Run a short 30-second soak locally:

```bash
WGPU_BACKEND=vulkan python3 scripts/run_soak.py \
  --duration-seconds 30 --gpu-smoke \
  --output target/soak/local.json \
  --log target/soak/local.log
```

The weekly `Extended checks` workflow runs it for ten minutes. To reproduce it locally:

```bash
WGPU_BACKEND=vulkan python3 scripts/run_soak.py \
  --duration-seconds 600 --gpu-smoke \
  --output target/soak/release.json \
  --log target/soak/release.log
```

For an offscreen Mesa GL run on a host where the default GL enumeration exposes only a hardware adapter, explicitly select software rendering:

```bash
LIBGL_ALWAYS_SOFTWARE=true WGPU_BACKEND=gl python3 scripts/run_soak.py \
  --duration-seconds 30 --gpu-smoke \
  --output target/soak/software-gl.json \
  --log target/soak/software-gl.log
```

This combination passed the offscreen smoke on llvmpipe on the current Arch host. `WGPU_BACKEND=gl` alone exposed only its Radeon adapter and failed the software-only test. Retain such failures; a later successful probe must not be spliced into an earlier failed record. No desktop window is created by this core/offscreen lane, and the environment selection is scoped to that command.

The runner writes the report and log, prints each check, and exits non-zero if any ceiling is breached. CI uploads both even when a ceiling fails.

## Displayed Wayland process soak

The native fixture continuously emits styled ASCII, CJK, combining marks, emoji-ZWJ sequences, long lines, and bounded Kitty image replacement through a real PTY. Each replacement requests an exact Kitty success response, and the runner requires a duration-scaled minimum number of `i=91;OK` acknowledgements; plain byte progress cannot stand in for graphics acceptance. The runner alternates the Sway output between two sizes, repeatedly creates and destroys a focused split pane, injects PID-unique keyboard tokens, and requires exact single-pane routing. It samples the real application PID through `/proc`, including direct children spawned by any thread, then closes the native window and requires every observed fixture child to disappear.

For every sample, the runner scans the bounded application process tree's `/proc/<pid>/fdinfo/<fd>` files. It follows the Linux kernel's [DRM client usage-statistics contract](https://docs.kernel.org/gpu/drm-usage-stats.html): `drm-client-id` plus `drm-pdev` prevent shared descriptors from being counted twice, `drm-total-<region>` records requested buffers, and `drm-resident-<region>` records instantiated backing storage. The deprecated AMDGPU `drm-memory-<region>` alias is accepted only as resident memory and never added to a simultaneous `drm-resident-*` value. Engine cycle counters are not memory. Drivers may omit these optional kernel fields, so each sample records a bounded explicit unavailable reason instead of inventing a zero.

Run a 30-second displayed soak after building the application:

```bash
cargo build -p termeric --locked
WGPU_BACKEND=gl python3 scripts/run_native_gui_soak.py \
  --duration-seconds 30 \
  --binary target/debug/termeric \
  --artifacts target/native-gui-soak
```

The artifact destination must be new or empty; the runner refuses to mix results from an earlier run. Use a distinct directory for every run.

Ordinary software-rendered CI permits all GPU-memory samples to be explicitly unavailable. For a vendor-GPU run where the driver is expected to export DRM memory, require complete coverage and retain a separate bundle:

```bash
WGPU_BACKEND=vulkan python3 scripts/run_native_gui_soak.py \
  --duration-seconds 600 \
  --require-gpu-memory \
  --binary target/debug/termeric \
  --artifacts target/native-gui-soak-vendor
```

`--max-gpu-resident-mib` and `--max-gpu-resident-growth-mib` tighten the default 1024 MiB peak and 256 MiB first-to-last-quartile resident-memory rails. The required mode fails if any sample is unavailable or if availability changes during a run. A driver that exposes total but not resident memory still records the total exactly; resident peak/growth remain explicitly absent rather than being inferred.

The weekly `Extended checks` workflow runs both the core scenario above and this displayed scenario for ten minutes. Native defaults allow 768 MiB peak RSS, 192 MiB first-to-last-quartile RSS growth, eight threads and sixteen file descriptors of growth, and eight concurrent direct children. The report also requires continuous PTY progress, enough resize/pane actions for the configured duration, zero runtime/GPU validation errors, a normal application exit, and complete child cleanup. Resize and pane-churn counts have independent minimums; one workload cannot compensate for the other in the aggregate action count. These are broad regression rails, not expected steady-state consumption targets.

The native report records UTC time, backend/compositor/WGPU identity, every raw process sample, exit/driver/cleanup/workload outcomes, resource summaries, DRM memory snapshots and their summary, thresholds, and each check. A missing application log fails the run rather than reading as an empty, error-free log. Run the regression detectors with:

```bash
python3 -m unittest scripts.test_drm_memory scripts.test_run_native_gui_soak
```

Defaults are intentionally broad host-independent safety rails: 512 MiB peak RSS, 128 MiB first-to-last RSS growth, four threads of growth, eight file descriptors of growth, no more than 16 concurrent direct children, at most 2 MiB of transcript backlog, no dropped transcript entries, complete queue drainage, no surviving observed children, successful scenario/GPU exit, a completion marker, and no recognized GPU validation/device-loss message. The first and last quartiles are compared by median so process startup and one noisy sample do not define the growth result. Tighten these budgets only with repeated artifacts from the same class of runner; do not hide a regression by raising them without an investigation.

The core runner JSON records UTC time, WGPU backend, pass/fail status, every raw sample, terminal outcomes, scenario progress (which must be contiguous and end with one completion record), resource summaries, thresholds, and each check. The companion log holds the Rust test output and the optional headless GPU smoke. CI uploads both even when a ceiling fails.

## Regression-detector tests

The ceiling evaluator has deliberate flat, file-descriptor leak, RSS leak, thread leak, excessive transcript backlog, incomplete drain, dropped-entry, missing-queue-metric, malformed inventory, mismatched inventory, category classification, label-sanitization/cardinality, and category-summary fixtures. A synthetic multi-thread `/proc` tree separately proves that children of non-leader threads are included exactly once:

```bash
python3 -m unittest scripts.test_run_soak scripts.test_run_native_gui_soak
```

`scripts.test_run_soak` also drives the runner with a fake scenario to cover a clean pass, launch failure, interruption, timeout, a GPU-smoke timeout, and a transient `/proc` sample.

The ten-minute headless and displayed soaks run in the weekly `Extended checks` workflow (`.github/workflows/extended.yml`), which can also be started by hand.

These tests prove the evaluator rejects the relevant monotonic-growth shapes; they do not intentionally leak resources from the test runner process.

## Evidence and limitations

### 2026-09-18 release-binary refresh

Clean `946eb65` completed a schema-6 600-second displayed run using `target/release/termeric` under private headless Sway and AMD/radeonsi OpenGL (`software=false`). The binary was 25,994,904 bytes with SHA-256 `da9140702571c8df8445407f01d1bf1f277884c17cd0f64a799dca33e9646817`, matching that candidate's validated release-preflight binary. All 24 checks passed; independent validation reconstructed the manifest, provenance, transcript, samples, resource summaries, and cleanup before this documentation-only update. The local bundle is `/tmp/termeric-native-soak-946eb65-release/results.json` and its sibling artifacts.

The adapter reported `AMD Ryzen 9 7900X3D` / `raphael_mendocino` on Linux `7.2.6-arch2-1`; this is not a measurement of the RX 7900 XTX used in some earlier GUI acceptance records.

All 954 samples supplied total and resident DRM memory. Peak process-attributed GPU resident memory was 115,601,408 bytes (110.2 MiB), with 4,096 bytes (4 KiB) first-to-last-quartile median growth. Peak RSS was 150,876 KiB (147.3 MiB), with 1,008 KiB quartile growth. Thread and descriptor quartile growth was **+1/+3**, not zero. The inventories attribute those changes to `pty-reader` and `device-path`. All 478 one-child samples had exactly 20 threads and 23 fds; all 476 two-child samples had exactly 21 threads and 26 fds. This is consistent with the fixture's alternating pane state rather than monotonic accumulation in this record; it does not establish indefinite leak freedom.

The workload produced 956 PTY progress records, 238 output resizes, 238 pane actions, and 612 accepted Kitty replacements. Peak concurrent children was two; all 120 observed children disappeared after normal native close. No recognized runtime/renderer error or driver failure was recorded. Image acknowledgements are protocol evidence, not a visual inspection of the displayed images.

This run explicitly used installed **Noto Sans Mono** with Noto Color Emoji as the required fallback. Font inventory is not proof of rendered face or glyph coverage. The command was:

```bash
env -u WLR_RENDERER -u LIBGL_ALWAYS_SOFTWARE \
  WGPU_BACKEND=gl TERMERIC_GUI_FONT_FAMILY='Noto Sans Mono' \
  python3 scripts/run_native_gui_soak.py \
  --duration-seconds 600 --require-clean --require-gpu-memory \
  --binary target/release/termeric \
  --artifacts /tmp/termeric-native-soak-946eb65-release
python3 scripts/validate_native_gui_soak.py \
  /tmp/termeric-native-soak-946eb65-release/results.json --source .
```

Three separate 30-second probe destinations preserve setup outcomes:

- `/tmp/termeric-native-probe-946eb65-release`: failed before app launch because the default DejaVu Sans Mono family was absent.
- `/tmp/termeric-native-probe-946eb65-release-noto`: with Noto and an explicit pixman compositor override, the workload completed on llvmpipe but correctly failed required DRM coverage (0/49 samples); missing memory was not zero.
- `/tmp/termeric-native-probe-946eb65-release-auto`: with the renderer overrides removed, AMD/radeonsi passed all 24 checks and 49/49 required DRM samples.

Both failed bundles validated only with `--allow-ineligible`; the successful probe validated normally. No probe samples were merged into the sustained run. No real provider, notification service, or user's desktop session was exercised. This is a local release-build observation, not a controlled comparison with the older builds/font profiles below, a whole-app CPU/power result, a specific notification-worker stress test, or proof of multi-day stability. Hosted history, other adapters/backends, real-desktop visuals/input/accessibility, and portable signed evidence remain open. Local `/tmp` bundles are not published artifacts; the exact-checkout/binary portability boundary above still applies.

### Earlier observations

On 2026-09-16, clean `bd70faa` completed a schema-6 600-second displayed run under private headless Sway with AMD/radeonsi OpenGL (`software=false`), requiring DRM memory on every sample. All 24 checks passed, and the independent validator accepted the manifest, source/executable identity, samples, transcript, and cleanup outcomes before this documentation-only follow-up. The local bundle is `/tmp/termeric-vendor-soak-bd70faa/results.json` and its sibling artifacts.

All 877 samples supplied both total and resident process-attributed DRM memory; none were unavailable. Peak resident memory was 115,601,408 bytes (110.2 MiB), with 5,255,168 bytes (5.0 MiB) first-to-last-quartile median growth. The kernel driver was `amdgpu`; these totals cover the deduplicated clients and exported regions, including GTT and VRAM, not whole-device VRAM occupancy. Peak application RSS was 155,364 KiB (151.7 MiB), with 2,816 KiB (2.75 MiB) quartile growth and zero thread/descriptor growth. The workload completed 954 PTY progress records, 218 output resizes, 218 pane-churn actions, and 613 accepted Kitty replacements. Peak concurrent fixture children was two; all 110 observed children were gone after normal native close. No recognized runtime/renderer error or driver failure was recorded.

The separate required-memory 30-second probe at the same candidate also passed and validated, with 49/49 measured samples, in `/tmp/termeric-vendor-probe-bd70faa/results.json`. Its samples were not merged into the ten-minute record. This supplies one accepted local vendor-memory observation, not a signed/published release artifact, a controlled comparison with the software-renderer results below, broad driver qualification, or multi-day stability. Hosted scheduled history, real-desktop observation, other adapters/backends, native IME/accessibility, and visual quality still need their own evidence. Revalidate before changing the checkout or measured binary; the portability limits above apply.

On 2026-09-16, clean `e446dfd` completed the current schema-6 600-second displayed-process lane under private headless Sway, using GL/llvmpipe and a pixman compositor. All 24 checks passed, and the independent native validator accepted the complete manifest, exact source/executable identity, reconstructed transcript activity, samples, and outcomes before this documentation-only follow-up. The local bundle is `/tmp/termeric-displayed-soak-e446dfd`, with `results.json` as its record; it is not a published release artifact.

Across 950 samples, peak application RSS was 242,580 KiB (236.9 MiB), with 3,680 KiB (3.6 MiB) first-to-last-quartile median RSS growth and zero thread/descriptor growth by the same quartile metric. The run completed 953 PTY progress records, 237 compositor resize actions, 237 pane-churn actions, and 611 accepted Kitty image replacements. Peak concurrent fixture children was two; all 120 observed fixture processes were gone after normal native close. No recognized renderer/runtime error or driver failure was recorded.

All 950 DRM memory samples were explicitly unavailable (`no-drm-client-fd`), with null peak/growth values, not zero GPU-memory usage. Passing the optional coverage/disposition checks does not establish vendor-GPU memory bounds. Likewise, the quartile growth metric reduces sensitivity to warm-up and single-sample noise; it is not the first-to-last raw RSS difference. These broad regression rails do not establish an optimal memory footprint or a controlled improvement/regression relative to older, differently scoped records. That software-renderer observation did not establish hosted scheduled history, real-desktop/vendor-GPU behavior, visual quality, native IME/accessibility, or multi-day stability; the separate AMD observation above has its own scope.

On 2026-09-16, clean candidate `5a5e79d` completed the current schema-5 600-second core lane with Vulkan/llvmpipe offscreen smoke. All 28 checks passed, and the independent validator accepted the source/executable identity, sealed log, reconstructed samples, inventories, workload, and outcomes. The local record and log are retained at `/tmp/termeric-core-soak-5a5e79d/results.json` and its sibling `scenario.log`. They were validated before this documentation-only follow-up; they are evidence for that exact candidate, not a claim that subsequent commits were measured.

The run retained 1,200 samples and completed 6,913 workload iterations. Peak RSS was 51,748 KiB (50.5 MiB); first-to-last-quartile median RSS growth was 712 KiB, with zero thread and descriptor growth, four peak concurrent children, and no surviving observed children. Transcript backlog peaked at 10,590 bytes; all 68 entries per iteration were accepted without drops and every queue drained to zero. The persistent detector completed 553,040 synthetic patches, reached 64 retained patches and 3,822,686 owned bytes under pressure, and reconciled eviction throughout the run. The software-GPU smoke passed with no recognized validation/device errors.

This supplies one green local ten-minute observation for the newer diff-history workload and launch-provenance contract. It does not promote the older failed record below, establish a throughput improvement over its different workload, or prove displayed/vendor-GPU, hosted scheduled-CI, real-desktop, or multi-day stability. The portability limitations above still apply; the `/tmp` bundle is local, not a published release artifact.

On 2026-09-14, the unchanged `5eeded9` checkout completed a schema-3 600-second core scenario in `/tmp/termeric-core-soak-5eeded9-600s/results.json`: 1,199 resource samples, 7,428 complete iterations, 45,828 KiB peak RSS, −64 KiB first-to-last-quartile RSS growth, zero thread/fd growth, four peak children, 10,590-byte peak transcript backlog, no drops, zero queued bytes after every finish, and complete observed-child cleanup. Eighteen of nineteen checks passed; the initial software-only GPU smoke failed because default GL enumeration provided no CPU adapter. The overall record is **failed/ineligible**, and was validated only with `--allow-ineligible` to verify diagnostic consistency.

A separate, explicitly software-selected 30-second run at the same unchanged checkout passed all nineteen checks and normal schema-3 validation in `/tmp/termeric-core-soak-5eeded9-software-30s/results.json`: 61 samples, 422 iterations, 44,004 KiB peak RSS, 8,184 KiB quartile RSS growth, zero thread growth, −1 descriptor growth, complete transcript drainage with no drops, complete observed-child cleanup, and a passing llvmpipe offscreen smoke. An independent offscreen probe also passed with that environment. The short run overlapped the final portion of the long scenario; these are resource-ceiling observations, not throughput benchmarks. Neither short result repairs the long-run GPU failure, closes #108's launch-provenance gap, or supplies native desktop/provider acceptance. JSON and scenario logs were retained separately and validated before the documentation-only follow-up commit.

On 2026-08-25, the local 30-second change-gate scenario plus headless Vulkan smoke completed 322 iterations with 61 samples and all ten checks in the then-current runner passing. It peaked at 33.9 MiB RSS, measured 7.0 MiB first-to-last-quartile RSS growth and zero thread/fd growth, emitted its completion marker, cleaned up children, retained 16.0 MiB of scrollback, and kept images at 512 KiB. The artifact is `target/soak/issue-23-final.json`; it is historical local evidence, not a checked-in baseline or a scheduled/release- duration result. It predates the fail-closed queue-drain fields and therefore does not satisfy the current acceptance contract.

On 2026-09-02, the then-current fail-closed contract completed a fresh 30-second core run plus headless GL submission in `target/soak/issue-23-current-core.json`. It recorded 61 resource samples and 524 complete workload iterations, peaked at 42,900 KiB RSS, and measured 4,680 KiB first-to-last-quartile RSS growth with zero thread/fd growth. All children were cleaned up; all 68 transcript entries per iteration were accepted with no drops, the queue peaked at 10,526 bytes and drained to zero after every durable finish, and no GPU validation/device error was present. All 18 current checks passed. This replaces the obsolete local artifact for the short core lane, but it still does not substitute for either ten-minute candidate soak.

The same working tree then completed the full 600-second core duration in `target/soak/issue-23-current-core-10m.json`: 1,200 resource samples, 9,160 complete iterations, 42,996 KiB peak RSS, only 244 KiB quartile-to-quartile RSS growth, zero thread/fd growth, four peak children, a 10,590-byte transcript queue peak, zero post-finish queue bytes, zero drops, complete child cleanup, and zero GPU errors. All 18 checks passed. Because the tree is intentionally dirty roadmap work and this artifact predates the embedded dirty-state, binary-digest, and schema-2 resource-inventory fields, it is strong local duration evidence but not an accepted current-schema clean-candidate or scheduled-CI record.

On 2026-09-10, clean commit `9a70020` passed the current 30-second core contract in `target/soak/issue-23-current-clean.json`: 61 samples, 505 complete workload iterations, 40,972 KiB peak RSS, 6,696 KiB first-to-last-quartile RSS growth, zero thread/fd growth, four peak children, complete child cleanup, complete queue drainage with no drops, and no GPU validation/device error. All 18 gates passed. It predates the schema-2 diagnostic inventories and remains clean historical short-lane evidence; the earlier 600-second result is still not a clean-candidate or scheduled-CI record.

Clean commit `d623154` then passed the first local 30-second displayed-process contract in `target/soak/issue-23-d623154-native`: 49 samples, 142,732 KiB peak RSS, 11,556 KiB first-to-last-quartile RSS growth, one thread and three file descriptors of growth, and two peak fixture children. The real application completed 49 progress records, 12 compositor resizes, 12 pane create/route/ close actions, and 31 accepted Kitty image replacements before a normal native close. All observed children exited and the log contained no recognized renderer, GPU, or runtime error; all 14 gates passed. The artifact is local short-lane evidence rather than a ten-minute scheduled or release-candidate record.

Clean commit `a53d097` passed the strengthened schema-2 600-second displayed- process contract in `target/native-gui-soak/issue-23-a53d097-schema2-10m`. The report itself records `worktree_dirty: false`, the complete commit, and tested binary SHA-256 `9cfc391dd5fd4b321c002c3afeb38568f677012fd25081e20e9ecaaae86541b0`. Across 957 resource samples it peaked at 159,644 KiB RSS and measured only 524 KiB first-to-last-quartile RSS growth, with zero thread or file-descriptor growth and two peak children. It completed 239 compositor resizes, 239 exact pane create/route/close actions, 614 accepted Kitty replacements, normal native close, and cleanup of all 121 observed fixture children. All 14 gates passed with no recognized runtime/GPU error. It predates the schema-3 diagnostic inventories but remains clean local release-duration aggregate evidence, not a scheduled-CI or real-desktop result.

The core soak still does not create a window; the native lane does so under a private compositor. The historical displayed records above predate schema 5; the schema-6 `e446dfd` record supplies newer local duration evidence but still has no vendor-GPU memory measurements. Neither lane substitutes for a long real-desktop observation, validates macOS/Windows beyond the separate bounded native-PTY smoke, or proves every terminal protocol. Those remaining boundaries stay separate platform/manual rows in [platform-support.md](platform-support.md) and [release-acceptance.md](release-acceptance.md).
