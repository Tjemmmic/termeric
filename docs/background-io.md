# Background Git and transcript I/O

Issue [#22](https://github.com/Tjemmmic/termeric/issues/22) moves repository
inspection and transcript persistence out of rendering, keyboard input, and PTY
output processing. Both paths use small dedicated workers because their
libraries are blocking and do not justify an application-wide async runtime.

## Git snapshots

One Git worker owns all libgit2 repository access. The application publishes a
cwd plus a monotonically increasing generation and continues without waiting.
Only one scan may be queued or running; cwd churn replaces one pending refresh
instead of growing a request queue. When a scan completes, a Winit user event
wakes the event loop.

Results carry their cwd and generation. The UI discards a result unless both
still match the active request, so a slow old repository cannot overwrite a
new cwd. Status polling is debounced to two seconds, while explicit Git command
execution invalidates the current generation. A periodic refresh for the same
cwd does not cancel an in-flight scan: if the poll deadline passes during slow
repository I/O, that snapshot remains publishable and exactly one follow-up
refresh is retained. Repeated identical worker errors produce one actionable
warning until a successful scan resets the error state.

The immutable result contains status and the deduplicated local/remote branch
list. Branch completion on each keystroke only filters that list in memory; it
never opens a repository.

## Transcript writer

Each active transcript owns one worker and a bounded channel. The terminal path
copies an accepted entry into memory and returns immediately. The defaults are:

- 512 queued entries;
- 2 MiB of queued content/metadata accounting;
- a 50 MiB persisted-file cap;
- a 250 ms periodic buffered flush; and
- a 500 ms bounded durable flush during stop or normal teardown.

The worker creates the file, redacts common token prefixes, serializes JSONL,
batches writes through `BufWriter`, tracks exact serialized bytes, and calls
`sync_data` during a clean shutdown. It does not create temporary plaintext
files. The size cap is tracked in memory after the metadata line, eliminating
per-entry filesystem metadata calls.

If either queue bound is reached, the entry is dropped rather than blocking
input or PTY handling. The first drop produces one visible warning; subsequent
drops remain bounded and silent. File-cap and writer failures wake the UI and
produce one warning with the cap or error context. Stopping a transcript reports
whether the durable flush completed and identifies the partial path when it did
not. Application teardown logs the same incomplete-persistence condition when
there is no remaining UI in which to display it.

## Deterministic coverage

Injected backends cover:

- a blocked Git scan followed by cwd churn, proving calls return promptly and
  the old result is rejected;
- a same-cwd scan that crosses the polling deadline, proving it publishes once
  and coalesces the overdue refresh into one follow-up rather than starving;
- an injected Git failure, including one-shot path-aware reporting;
- a slow transcript sink under sustained input/output enqueue pressure;
- exact cap reach without `metadata()` calls per entry;
- disk-full/write failure and unavailable-directory startup failure;
- worker-event wakeups and one-shot notices; and
- clean shutdown proving the batched sink receives one durable `sync_data`, and
  shutdown while persistence is slow, including the bounded timeout.

The release suite adds
`latency.transcript_enqueue_under_slow_disk`. It interleaves 1,000 input/output
entries while an injected sink delays every write by 2 ms. On the 2026-08-25
acceptance host, five release samples measured 0.0909 ms median and 0.0996 ms
p95 for the complete producer loop. The bounded queue explicitly shed 984 of
1,000 entries in every sample instead of inheriting roughly two seconds of sink
latency. Displayed idle CPU remained 0.0% in all five samples. The release p95
safety rail is 25 ms.

This workload proves bounded producer latency and explicit degradation. It does
not model a particular disk, filesystem, repository, or real user's acceptable
loss policy. Release soak testing remains responsible for long-duration writer
throughput, filesystem behavior, thread/fd stability, and operator review of
warning UX.

The exact 2026-09-02 tree is also recorded in
`target/benchmarks/issue-22-final-current.json`. Its 1,000-entry slow-sink loop
measured 0.0949 ms median and 0.1622 ms p95 while explicitly shedding 984
entries in every sample; the 25 ms p95 producer-latency rail passed.
