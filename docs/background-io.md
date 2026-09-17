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

The worker combines the already sorted, unique local and remote lists using a
forward-only local cursor. Duplicate checking is linear in the two list lengths,
rather than repeatedly searching the growing result. Local names still come
first, followed by sorted remote-only names; existing `origin/` normalization,
HEAD exclusion, case sensitivity, and completion ranking are unchanged. Retained
strings move into the result without cloning or allocating a separate membership
set. Tests cover every pair of subsets of a mixed-case/Unicode fixture, retained
string storage, 60,000 distinct names with overlap, and a real temporary libgit2
repository. This is an algorithmic improvement, not a measured whole-application
RSS or latency claim; libgit2 work and sorting costs remain outside the linear
merge. The 60,000-name case exercises that merge helper independently of the
production collection limits below.

### Branch completion resource limits

Each scan admits at most 16,384 collected names and 4 MiB of UTF-8 name text,
shared across local and remote branches. Names are charged before copying,
after `origin/` normalization and HEAD exclusion, but before sorting and
deduplication. Duplicates therefore consume collection budget too. An exact
limit is accepted; the next name that would exceed either limit stops collection
and drops the entire partial result. There is no silently truncated suggestion
set or per-keystroke repository fallback.

On overflow, Git status remains available, the old completion cache is released,
and an open suggestion popup is dismissed without changing tracked input or
sending PTY bytes. One path-aware warning explains the limits and directs the
user to shell completion. Identical over-limit refreshes do not repeat the
warning. A later in-budget snapshot restores completion on subsequent input;
a new overflow after recovery can warn again. Generation checks also reject
stale limit results.

These are per-scan collected-name and text-byte bounds, not a 4 MiB process-memory
ceiling. Vector/allocator overhead, simultaneous old/new snapshots, filtered
lists, lowercasing scratch space, tracked user input, Git status, and
libgit2's internal repository/index/ref storage are separate. Native RSS and
large-repository latency still need measurement; collection cannot cancel a
blocked libgit2 call. Regression fixtures cover exact count/byte limits, UTF-8,
remote normalization and duplicates, warning/recovery behavior, and real packed
refs exceeding the production limits without launching Git or a provider.

### Completion rendering storage

Filtered suggestions move into an immutable, reference-counted list shared by
input state and renderer snapshots. The app-to-renderer handoff and renderer
dispatch clone only that shared handle, not every branch string. The maximum
Unicode-scalar name length is computed once on list replacement and reused for
popup sizing; redraws no longer scan every name for width. Selection changes,
window resizing, and font scaling reuse the same names and length metadata.
Ordering, matching, insertion, and the existing scalar-based sizing policy are
unchanged; this does not add grapheme-aware or display-width-aware sizing.

Clearing uses an allocation-free empty state. Existing render snapshots retain
their original immutable names until released, so replacing input suggestions
cannot change an older frame's data or width metadata. The worker cache and
filtered list remain separate allocations, and filtering still allocates on
input. This change removes redundant per-frame copies, not all completion
allocations. Tests check moved string/vector storage, Unicode length metadata,
1,000 render-snapshot clones sharing a full 16,384-name / 4 MiB fixture, and
final-owner release after clearing/replacement. These are deterministic storage
and lifetime checks, not measured native frame-time or process-RSS results.

## Transcript writer

Each active transcript owns one worker and a bounded channel. The terminal path
copies an accepted entry into memory and returns immediately. The defaults are:

- 512 queued entries;
- 2 MiB of queued content/metadata accounting;
- a 50 MiB persisted-file cap;
- a 250 ms buffered-flush deadline while a batch is pending; and
- a 500 ms bounded durable flush during stop or normal teardown.

The recorder and its error/cap state belong to one terminal owner, not to an
entire split tab. They move with that owner on split/collapse. Output, diff
observation, and successful user writes are recorded at that exact owner;
protocol replies and sibling traffic are excluded. Pending UI notices are
bounded to 16 per owner and drained with pane attribution. A failed or capped
worker clears only its owner's recording flag.

[Raw transcript text](transcript-text.md) has independent input/output UTF-8
decoders in each recorder. They retain at most three incomplete bytes per
direction, borrow valid chunks, and cap repair allocation at the queue-byte
limit. Invalid bytes and unfinished final sequences become annotated replacement
characters with bounded warnings; those warnings do not stop a healthy
recorder. Stop/replacement/EOF finalize the old epoch without carrying text into
a new recording. Protocol, sibling, and refused user writes stay excluded.

[Passive diff capture](diff-capture.md) is separately bounded per terminal:
16 KiB line framing, 256 KiB raw patches, and 64 retained reviews within a 4 MiB
owned-storage budget. Complete validated patches enqueue one transcript diff
entry at a file boundary or actual output EOF, even when several files complete
in one PTY read. Empty polls do not complete pending patches. Rejected capture
does not suppress terminal output, and eviction never reuses review identities.

The worker creates the file, redacts common token prefixes, serializes JSONL,
batches writes through `BufWriter`, tracks exact serialized bytes, and calls
`sync_data` during a clean shutdown. It does not create temporary plaintext
files. The size cap is tracked in memory after the metadata line, eliminating
per-entry filesystem metadata calls.

A clean writer blocks on its command channel without a periodic timer. The
first accepted entry after a flush starts one 250 ms batch deadline; later
entries do not move it. An expired pending batch is flushed before another
command is received, including when cap-rejected traffic keeps arriving.
Periodic and checkpoint flushes clear the deadline, returning the worker to
untimed sleep. Input, a checkpoint, or sender disconnection wakes it normally.
This avoids the former idle loop that repeatedly polled with zero timeout
after an old flush timestamp expired. The deadline is cooperative: it does
not interrupt a blocked sink or guarantee a filesystem latency bound.

### Redaction boundary

Before serialization, the worker filters entry content and all metadata string
values, including the initial metadata record. The recognized prefixes are
`sk-`, `ghp_`, `gho_`, `github_pat_`, `xoxb-`, `xoxp-`, `AKIA`, and `eyJ`.
A recognized prefix is retained with `[REDACTED]` replacing everything after it
up to whitespace (including Unicode whitespace), a single/double quote, comma,
semicolon, or the end of that string. The forward-only scan follows UTF-8
boundaries and has no arbitrary token-length cutoff. It no longer repeatedly
moves the remaining string for each replacement. Metadata types and structural
keys remain unchanged.

This is best-effort filtering, not a secret-free export guarantee. Unknown
credential formats, encoded secrets, matches split across entries or string
values, sensitive metadata keys, and personal data without a known prefix are
not covered. Conversely, ordinary text containing one of these prefixes can be
redacted. Review transcripts before sharing them. Existing files and previously
exported copies are not retroactively sanitized by this fix; the export path
uses the persisted data. See [#104](https://github.com/Tjemmmic/termeric/issues/104).

### Backpressure and failure reporting

If either queue bound is reached, the entry is dropped rather than blocking
input or PTY handling. The first drop produces one visible warning; subsequent
drops remain bounded and silent. File-cap and writer failures wake the UI and
produce one warning with the cap or error context. Stopping a transcript reports
whether the durable flush completed and identifies the partial path when it did
not. Application teardown logs the same incomplete-persistence condition when
there is no remaining UI in which to display it.

Each queued entry owns its byte reservation. Dequeue releases that charge before
the worker serializes the entry; rejected sends and unread entries destroyed
after worker failure or panic release it through the same ownership rule.
Receiver cleanup never resets a shared counter wholesale, so it cannot erase a
still-live producer reservation. The ledger therefore measures admitted queued
content/metadata accounting, not heap/RSS or the worker's current entry. A failed
writer cannot leave freed entries charged as a full queue and mask a subsequent
disconnection as backpressure.

Shutdown finalizes the recorder's text decoders once and disconnects its sole
command sender. Already accepted entries and checkpoints drain in order; no
spare queue slot or extra shutdown command is needed. The worker publishes one
completion result only after flushing, syncing, and dropping its sink and
pending commands. A timed-out `finish()` retains that completion receiver, so
another call cannot report success while persistence or cleanup is still
running. Each call has its own bounded wait, and joins are attempted only for
an already-finished thread. The cached result preserves success or the original
failure kind/message across retries, even if UI notices were already consumed.
Failure to enqueue a final UTF-8 replacement during the first `finish()` also
remains an error rather than disappearing after a timeout. A worker
panic/disconnection is a failure, never
implicit success. A timeout cannot cancel a blocked filesystem operation or
guarantee persistence after process exit.

## Deterministic coverage

Structured export now uses an ordered writer checkpoint plus a separately
bounded streaming worker; it does not read, parse, or write transcript files
inside the UI action. The exact snapshot, publication, memory, and cancellation
contract is documented in [transcript export](transcript-export.md).

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
  shutdown while persistence is slow, including the bounded timeout;
- channel-gated sync and sink destruction, proving repeated shutdown remains
  timed out until actual completion without an unbounded thread join;
- full-queue drain, late-write refusal, final UTF-8 loss across retries, cached
  startup/sync failures after notice consumption, and worker-panic refusal.

Queue-ledger regressions cover unread entries at startup failure and panic,
entry-slot and byte-budget rejection, writes after shutdown, receiver destruction
with a live producer reservation, and concurrent bounded reservation churn.
They assert zero outstanding charges after cleanup without treating an
accounting correction as a measured reduction in resident memory.

Test-only receive observation verifies that a clean recorder makes one blocking
receive attempt across two flush intervals, rather than timer-polling. Synthetic
sinks also prove that periodic flushing returns to sleep, later checkpoints and
output wake it, shutdown syncs once, and ongoing accepted or cap-rejected traffic
cannot starve already accepted data. This observes the worker's actual receive loop;
it is not an estimate of whole-application idle CPU or native power usage.

Redaction regressions cover all prefixes/delimiters, Unicode around the former
40-byte cutoff, long and densely repeated synthetic tokens, nested metadata,
and a real local recorder followed by Markdown export. The recorder test proves
valid JSONL, a subsequent safe entry, a drained queue, successful durable flush,
and no fabricated token suffixes in either output. No provider is invoked.

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
