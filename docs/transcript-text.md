# Streaming transcript text

Raw transcript input and output are separate UTF-8 text streams owned by one `TranscriptRecorder`. They are not independent strings per PTY read. A valid character split over any number of reads/writes is preserved, along with valid text before and after it. Terminal control sequences, CR/LF, tabs, and NUL are preserved as text; the transcript observer does not alter terminal display.

## Text and loss policy

Valid text is preserved independent of transport chunk boundaries, provided the existing recorder queue/file limits admit it. Entry segmentation and timestamps may differ: a pending character is recorded when it becomes decodable, not when its first byte arrived. Input and output never share a pending character, and their interleaved entries are not a byte-exact global event timeline.

Invalid UTF-8 follows Rust's lossy UTF-8 replacement policy: invalid sequences become U+FFFD (`�`), without discarding unrelated valid prefixes/suffixes. An incomplete final sequence becomes one replacement at EOF or recording stop. Entries that contain generated replacements carry `metadata.utf8_replaced = true`. Original valid U+FFFD characters alone do not set that metadata.

The recorder emits at most one replacement notice per direction per recording epoch. These warnings flow through the existing owner-local, 16-message pending notice bound. A replacement warning **does not stop recording**; file-cap and worker-failure events retain their existing stop/notice behavior. Persisted replacements and their metadata remain visible when their entries are admitted. This is a text recording, not a lossless binary archive or an alternate screen model.

## Resource and backpressure boundaries

- Each direction retains at most three incomplete bytes between calls, in a fixed four-byte array plus a length field (16 bytes per decoder on tested 64-bit targets). There is no pending line or growing stream buffer.
- Already-valid chunks, and valid prefixes followed by an incomplete suffix, are borrowed for enqueueing; decoding adds no repair allocation for them.
- Repair uses at most one temporary string capped at the recorder's queue-byte limit: 2 MiB by default. Capacity growth, not just text length, is bounded. One raw input call is also capped at that limit before repair begins.
- One call creates at most one text entry, or none while waiting for a complete character. Repair never creates a growing queue of tiny entries or splits one call into multiple entries merely to fit it through the redactor.
- Queued owned entries remain under the separate 2 MiB/512-entry limits. Entry metadata and accounting overhead are included in admission, so a text buffer exactly at the repair ceiling need not fit into the queue.
- An oversized call/repair or full queue rejects the call through the existing nonblocking backpressure result. Pending carry for that direction is cleared across the loss; later bytes cannot silently join discarded text. File caps and worker failures still refuse new records.

Thus decoding adds at most one bounded repair string to the already-bounded queue, plus fixed per-recorder state. This is an owned-storage bound, not an RSS or allocator-overhead promise. See [background I/O](background-io.md) for the writer, durable flush, and existing persistence limits.

## Recording epochs, ownership, and exports

Recorder state moves with the terminal owner through split/collapse. Successful authorized user writes alone feed its input decoder; sibling output, protocol replies, and refused writes cannot complete its pending characters.

Starting recording does not seed the decoder from unrecorded traffic. Starting mid-character consequently treats later continuation bytes as invalid text. Stopping or replacing a recording finalizes pending input/output into the old recorder before detaching it. A new recorder starts empty, even when it occupies the same terminal. Closing an owner or directly dropping/finishing a recorder uses the same bounded finalization. Actual PTY output EOF finalizes both raw directions; repeated EOF/stop/finish does not duplicate their suffixes. An empty poll is not EOF. Recorder teardown still has its existing bounded shutdown deadline; failure to enqueue a final suffix is surfaced, not retried as a new character later.

An export checkpoint is different: it freezes the already-enqueued decoded prefix without forcing a pending character to become a replacement. That character can complete in the ongoing recording after the checkpoint. Existing JSONL and Markdown exporters remain compatible with replacement metadata.

[Diff capture](diff-capture.md) stays independent. Its completed patch entry is attributed to the recorder active **when the patch completes**, even if capture began before recording started. This does not replay earlier raw output into the new transcript. Stop/start of raw recording is not a diff boundary, and completed bounded patches still produce one diff enqueue attempt each.

## Redaction boundary

The background writer still applies its existing known-prefix redaction to each entry and metadata before persistence/export. Repaired text from a single call stays in one entry, including long tokens, so repair does not introduce an extra redaction boundary within that call. This remains best-effort filtering: unknown secrets or tokens divided across separate transcript entries are not universally protected. Review exports before sharing them.

## Verification

```bash
cargo test -p termeric session::transcript
cargo test -p termeric session::terminal_state
cargo test -p termeric transcript_utf8_carry_follows_owner
```

The original regression discarded both halves of `before 🌱 after`, losing all text in both input and output. Coverage now includes every split point and one-byte valid/malformed streams, deterministic randomized malformed bytes, exact decoder/repair bounds, newline-free overflow, a blocked writer, replacement metadata/export, checkpoint continuation, long-token redaction, EOF/stop/Drop and recording replacement, independent directions, bounded notices, completion-epoch diffs, and real local PTY owner movement with protocol and refused-write exclusion. No provider, native GUI, or external account is required. Tracked by [#128](https://github.com/Tjemmmic/termeric/issues/128).
