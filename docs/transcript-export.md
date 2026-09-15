# Background transcript export

Issue [#105](https://github.com/Tjemmmic/termeric/issues/105) makes **Export
Session** a background snapshot operation. It does not run the selected AI
provider, stop recording, or write anything into a terminal.

## Snapshot boundary

For the selected plugin terminal with an attached recorder, the action enqueues one ordered
checkpoint without waiting. A full recorder queue rejects the export with a
retry message; it does not silently move the boundary to a later time. The
recording worker processes earlier entries, flushes its buffered file, and
returns the exact persisted byte length and SHA-256 digest. Recording can
continue after that point.

The export worker opens the captured path, requires a regular file, copies only
that prefix, and verifies both length and digest. Unix opens are nonblocking
and refuse a final-component symlink, so a FIFO cannot wait for a writer before
type validation. Truncation, changed bytes, missing sources, and different
replacement contents fail. Replacement with byte-identical checkpoint content
is harmless and accepted: the digest establishes content identity, not a
continuing claim about a pathname's inode. Later appended entries are excluded.

The exported JSONL is that verified copy. Markdown is generated from the copy,
not a second read of the growing recorder file. A snapshot contains the
recorded prefix, not output that was never recorded or entries dropped under
the recorder's existing backpressure/file-cap policy. Its manifest records
whether the writer had reached the file cap at the checkpoint.

An attached recorder's error is an export error. There is no silent
scrollback fallback for a failed checkpoint, unreadable file, or malformed
record. If that terminal has **no attached recorder**, the action instead captures
retained history/visible rows from its focused active screen and clearly
reports a scrollback-only Markdown export. It does not search for historical
recordings on disk or claim to reconstruct a stopped recording.

## Resource and lifecycle limits

- One request per window; repeat clicks receive a busy message.
- Four workers process-wide, including workers still inside slow filesystem
  calls after their originating windows close. There is no unbounded job queue.
- A five-second checkpoint wait, checked in 50-ms intervals for cancellation.
- A 60-second cooperative export-processing deadline, checked between copy,
  parsing, and write operations. This does not interrupt a blocked filesystem
  syscall or guarantee a real-time I/O deadline.
- At most 50 MiB of checkpointed JSONL and 128 MiB of Markdown output.
- One encoded record at a time, capped at 16 MiB, plus its decoded entry.
  Conversion retains no whole-transcript entry list or Markdown string. A
  second streaming pass preserves the legacy first-metadata-anywhere header.
- A 64-KiB copy buffer and writes split into at most 64-KiB pieces.
- Scrollback-only capture has a 32-MiB text ceiling. It is an in-memory grid
  capture on the UI thread; its file I/O is background work. Oversized captures
  fail explicitly rather than truncating. This is not a claim that arbitrarily
  large grid-to-text capture is constant-time.

The recorder's normal enqueue limit is 2 MiB of content/serialized-metadata
accounting, including 128 bytes of framing allowance. JSON escaping expands a
byte by at most six; known-prefix redaction expansion is smaller than that.
The 16-MiB encoded-record rail therefore accommodates valid maximum queued
records. Initial metadata and serialized records now enforce that rail as
well. The 128-MiB Markdown ceiling exceeds the expansion of the documented
entry headings for a valid 50-MiB transcript; excess or malformed input fails
without publishing partial output.

The encoded buffer's ceiling is not a total-RSS ceiling: one decoded entry,
metadata values, serializer/decoder working memory, allocator overhead, and
the independently bounded live recording queue also consume memory. A
45-MiB synthetic conversion with 512-KiB output entries verifies an encoded
buffer peak of at most 1 MiB and complete content emission. The maximum
escaped-entry test covers nearly 2 MiB of NUL content (roughly 12 MiB JSON).
These are structural/correctness checks, not comparative throughput or RSS
benchmark claims.

## Publication and failure behavior

Each export uses a newly created `transcript-export-*` directory under the
session data directory. On Unix, the directory is created with mode `0700`
and output files with `0600`. Existing exports are never reused or overwritten.

A completed structured export contains:

```text
transcript-export-<unique>/
  transcript.md
  transcript.jsonl
  complete.json
```

`complete.json` is the publication marker. It is installed atomically without
overwrite only after both output files are flushed and synced. Its schema-1
record names the outputs and records the source-prefix byte count, SHA-256,
and recording-limit state. A scrollback-only export has no JSONL and a null
recorded-prefix field. Consumers must treat any directory **without the
marker** as incomplete staging, even if individual files are visible inside.
This is a marker-based publication contract, not an atomic rename of the
whole directory or a crash-proof filesystem transaction.

Ordinary errors and pre-publication cancellation remove the owned staging
directory. A process crash or a filesystem that prevents cleanup can leave an
incomplete private directory without a marker; no success is announced for it.
Publication is the commit point: window closure racing a completed publication
can leave a valid export on disk without a remaining UI to announce it.

Closing a source tab does not retarget the export: the pending checkpoint and
captured path remain owned by the request. Completion is delivered only to the
originating window and copies the Markdown path to its clipboard when
available. Closing that window cancels pending work without joining a thread
inside filesystem I/O. Cleanup occurs when the worker next reaches a
cancellation check, and the process-wide slot remains reserved until it exits.

## Privacy and evidence

Export does not retroactively sanitize data. The writer's
[best-effort redaction](background-io.md#redaction-boundary) remains the only
automatic known-prefix filter, and scrollback-only capture has no such filter.
Review both formats before sharing them.

Hermetic tests cover ordered checkpoints, full queues, flush failure, active
recording, exact raw/Markdown agreement, source mutation/truncation, corrupt
records, private permissions, publication collisions, cancellation, write and
flush errors, output/record ceilings, concurrent worker admission, window-local
completion after source closure, and refusal to substitute scrollback after a
recorder failure. Existing fake-executable PTY workflows now wait for the
asynchronous completion. No native GUI or authenticated provider acceptance is
implied.
