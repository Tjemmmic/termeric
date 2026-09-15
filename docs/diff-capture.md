# Bounded streaming diff capture

Termeric passively observes unified text diffs from a plugin-owned terminal.
It does not run a provider, inspect files, apply patches, or reconstruct an
interactive editor's screen. Shell terminals do not enable this observer.
Capture rejection never changes the bytes passed to the terminal emulator.

## Completion and text policy

PTY reads are arbitrary byte fragments, not lines or files. The observer keeps
one bounded line across reads, validates UTF-8 only after framing, strips SGR
and OSC sequences across chunk boundaries, and accepts LF or CRLF (including
repeated CRs immediately before LF from PTY ONLCR processing). Whitespace
inside diff lines is preserved, including trailing spaces and empty context
lines represented by a single space. Copied raw patches use LF and a final LF,
including when the final input line ended at EOF without one. Standard
`\ No newline at end of file` markers remain in the raw patch.

A patch needs `---`/`+++` file headers and at least one valid unified hunk.
Declared old/new counts must match the data, ranges must not overlap, and each
hunk must finish before another starts. Counts disambiguate removed text that
looks like a `---` file header. `/dev/null` additions and deletions are accepted;
deletions display the old filename. Tab-separated header timestamps are excluded
from the displayed filename, not the raw patch.

Completion happens at a next-file header, an unrelated complete output line,
or real PTY output EOF. An empty poll, idle time, or the end of a read does not
complete a file: another hunk may still arrive. Consequently, a valid final
patch from a still-running process remains pending until a boundary or EOF.
Closed/error events and a disconnected, drained output receiver finish framing
in both single-terminal and split-pane paths. Repeated EOF cannot duplicate a
completion. A valid final unterminated line is processed at EOF; incomplete
headers, hunks, UTF-8, and escape sequences are rejected.

Malformed/extra patch-looking lines, bare carriage returns, editing controls,
non-SGR CSI, and DCS/APC/PM/SOS control strings reject the current capture.
OSC payloads, even with embedded newlines or diff-like text, are not patch
content. An unterminated control string remains ignored until its terminator
or EOF, without accumulating an unbounded payload. Rejected candidates are
discarded, not offered as truncated complete patches. Scanning can resume at a
later file header. This is a conservative text-stream heuristic, not proof
that a patch applies to any particular working tree. Binary/combined diffs,
mode-only changes, arbitrary screen editing, and full Git filename unquoting
are outside its support contract.

## Per-terminal resource limits

| Resource | Ceiling / policy |
|---|---|
| Pending line | 16 KiB wire bytes, including ignored escape bytes; text capacity also at most 16 KiB. LF is the delimiter, not part of this limit. |
| Pending file header | One 16 KiB line. |
| Raw file patch | 256 KiB, including normalized newlines; capacity growth is capped at this limit. |
| File patch lines | 4,096 total headers, data lines, and no-newline markers. |
| Hunks | 128 per file; individual declared counts cannot exceed 4,096. |
| Owned completed patch | 1 MiB including raw/parsed duplicate strings, filename, headers, vector capacities, and inline values. |
| Retained review history | 64 files **and** 4 MiB charged storage, whichever fills first; includes reviewed entries. |
| Pending notices | Three coalesced detector flags, forwarded into the existing 16-message owner-local notice bound. |

Pending capture stores only raw text and fixed-size counters. Parsing the
bounded raw patch into review hunks happens once on completion. With the raw,
line, and hunk limits, construction stays below 1 MiB of owned patch storage
on supported 64-bit targets; the completed representation is also checked
explicitly before retention. History accounting charges actual string/vector
capacities and all preallocated history slots (with a conservative extra charge
for live inline values). No growing completion queue or repeated whole-patch
copy is created per incoming line. EOF and boundary completion use the same
path. Overall observer-owned storage is bounded by history plus one pending
line/header or patch and one bounded finalization; allocator bookkeeping and
the terminal grid are separate, not an RSS promise.

Oldest-first eviction applies equally to reviewed and unreviewed patches.
Identities increase monotonically and never wrap; exhaustion stops new
captures. A delayed review action retains tab, pane, and diff identities and
revalidates all three. An evicted patch cannot copy or dismiss a replacement.
An already-open review can still display its bounded snapshot; that snapshot
does not grant Copy authority after eviction. It is owned by the single review
overlay, separately from per-terminal detector history.

## Transcript and notice behavior

Each completed bounded patch makes exactly one nonblocking `record_diff`
enqueue attempt when that owner has an active recorder. This includes files
completed inside a read and older files evicted later in that same read. It
does not infer completion from the unreviewed count or clone a batch of patches.
Rejected candidates never create diff entries. Existing transcript queue/file
caps, backpressure, redaction, and failure notices still apply: an enqueue
attempt is not a durability guarantee. See [background I/O](background-io.md).

The separate [raw transcript text path](transcript-text.md) also decodes UTF-8
incrementally, with bounded repair and explicit invalid-byte replacement. It
remains a text recording, not a byte-exact PTY archive. Diff entries use the
recording epoch active at completion, even when capture began before recording;
raw recording stop/start does not prematurely finalize a diff.

Rejection, history eviction, and identity exhaustion produce bounded notices
attributed to the owning terminal. They do not stop or alter terminal display.

## Verification

```bash
cargo test -p termeric diff_detector
cargo test -p termeric session::terminal_state
cargo test -p termeric terminal_session_tests
```

Coverage includes every split point and one-byte chunks through Unicode,
SGR/OSC, CRLF, adjacent files and multiple hunks; whitespace, `/dev/null`,
header-like removed text and EOF; invalid UTF-8/controls/counts/ranges; bounded
long lines/control strings, byte/line/hunk ceilings; 200-file count- and
byte-driven eviction with capacity accounting and reviewed history; exhausted
identities; exactly-once transcript entries after eviction; single/split idle
polls versus closed/error/disconnected output; and stale Copy/Dismiss refusal.
Only synthetic streams and local raw/fake PTYs are used. Provider behavior,
native graphics rendering, and allocator/RSS soak acceptance are not implied.

Tracked by [#127](https://github.com/Tjemmmic/termeric/issues/127), a prerequisite
for broader [plugin-pane workflows](https://github.com/Tjemmmic/termeric/issues/126).
