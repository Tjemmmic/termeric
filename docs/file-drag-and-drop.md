# File drag and drop

In an ordinary local shell/profile tab, Termeric accepts native desktop file and directory drops as a deliberately small convenience. A successful local drop inserts one absolute path at the focused terminal cursor. That route never reads the dropped object, uploads its contents, or presses Enter. Live-domain files and directories use the separate confirmed transfer contract below.

## Supported path

The complete supported route is:

1. the compositor supplies one absolute local path;
2. Termeric validates the path text without opening the object;
3. the path is encoded as one POSIX single-quoted shell word followed by one space;
4. the existing focused-terminal paste route applies the terminal's current bracketed-paste mode and writes the bytes once; and
5. a toast tells the user that Enter is still required.

Spaces, glob characters, command substitutions, semicolons, and single quotes therefore remain literal path text when a POSIX-compatible shell parses the result. Termeric preserves the compositor-provided absolute spelling rather than canonicalizing symlinks or changing the selected name.

The insertion is capped at 4,096 UTF-8 bytes. Relative paths, non-UTF-8 names, control characters, line separators, and bidirectional formatting controls are rejected. A rejected drop writes no terminal bytes.

## Confirmed live-domain upload

Dropping one real regular file or directory on an attached local or OpenSSH live-domain pane opens a request-scoped confirmation. The notice names the local source, exact domain and stable pane, and destination name. A file shows its observed byte size. A directory truthfully remains in a preparation state until its bounded local archive size is known. Cancel, Escape, stale actions, window closure, and any replaced confirmation send no content.

After confirmation, a background worker reopens the source and verifies its type and Unix device/inode identity before reading it. The authenticated domain protocol v15 streams contiguous 64 KiB frames, with a 64 MiB per-transfer limit, a 30-second per-frame server idle timeout, at most four concurrent daemon uploads, and no UI-thread content I/O. The exact daemon-owned pane resolves its current absolute working directory; neither the GUI path nor a shell command is used as the remote destination.

The GUI keeps one persistent, in-place progress notice per window and forwards progress to the UI at bounded one-MiB intervals (plus the initial known total and completion), rather than once per wire frame. **Cancel Upload** in the command palette sets an exact-request cancellation flag. At the next pre-commit chunk boundary the client sends an explicit protocol cancellation frame; the daemon acknowledges the number of bytes received and removes the private temporary file. Stale, out-of-order, wrong-total, and post-completion progress events cannot update a different transfer. Cancellation is best-effort until commit: if atomic publication wins the race, Termeric truthfully reports success instead of claiming that an already-published file was cancelled.

Transient transport failure is retried automatically up to three times with the same random 128-bit upload identity. The daemon reports its retained contiguous offset and the SHA-256 digest of that prefix. The client hashes its local prefix before seeking and sending the remaining suffix, so a changed or incorrect source is refused without discarding a manually retained partial. Progress is a monotonic sent-byte high-water mark across retries. A lost final response is idempotently replayed from the same identity; that identity cannot be rebound to another domain, pane, name, declared size, or file/directory kind.

Directory preparation uses a deterministic, sorted Termeric archive and never invokes `tar` or another shell command. It supports at most 4,096 entries, 64 path levels, 4,096 UTF-8 bytes per relative path, and 64 MiB for the complete archive including headers and content. Empty directories are preserved. Nested non-UTF-8 or control/bidirectional names, symlinks, mount crossings, devices, sockets, and FIFOs are rejected. Source identity, size, and metadata are rechecked while walking and reading so a changed tree fails instead of silently mixing versions. Cancellation is checked during local traversal and file reads as well as during transport.

The daemon writes a unique mode-`0600` temporary stream in that directory. Only an explicit final commit publishes the requested name, and any existing destination is never replaced. Files publish as a hard link. On Linux, directories are validated and extracted into a private mode-`0700` sibling, then published atomically with `renameat2(RENAME_NOREPLACE)`. Source directory modes are normalized to `0700`; regular files become `0600`, except that any source execute bit produces `0700`. Owner, group, timestamps, ACLs, xattrs, hard-link relationships, sparse layout, and other metadata are deliberately not preserved. An ordinary transport interruption retains private staging data until a 60-second expiry (removed within 65 seconds by the five-second janitor cadence) so the same request can retry. A CLI upload started with `--retain` instead remains available for up to 24 hours and prints its upload ID before streaming. An authenticated later CLI process can list retained partials and resume or cancel one by that ID. At most four partial transfers reserve at most 256 MiB in aggregate; directory transfers reserve both archive and extraction capacity. Explicit cancellation and clean daemon shutdown remove staging immediately. After success, Termeric sends the exact POSIX-quoted remote path through the captured pane's paste mode without Enter. If that pane closes after publication, the file remains and a warning distinguishes successful upload from failed path insertion.

The same transport is scriptable without a GUI:

```bash
termeric domain upload DOMAIN PANE ./artifact.bin --name artifact.bin
termeric domain upload DOMAIN PANE ./project --name project-copy
termeric domain --host build.example upload DOMAIN PANE ./project --retain
termeric domain --host build.example uploads --json
termeric domain --host build.example upload DOMAIN PANE ./project --resume UPLOAD_ID
termeric domain --host build.example cancel-upload UPLOAD_ID
```

`--host-profile NAME` is supported wherever `--host` is supported. CLI upload does not inject the resulting path into terminal input. The GUI progress surface is not used by the CLI; interrupting the CLI disconnects the pre-commit stream, which leaves either the ordinary bounded short-lived staging record or the explicitly requested 24-hour record. `domain uploads` reports only partial records; successful replay metadata remains private and expires after 60 seconds. `--resume` requires an existing descriptor match and never silently starts over. Upload IDs select records but do not grant access: the same private Unix-socket or OpenSSH authentication is still required. Retention survives CLI process exit, not daemon exit or restart; live domains and their PTYs are themselves owned only by that daemon process.

## Trust-boundary refusals

Drops fail closed while a modal panel, search, copy mode, tab rename, Harvest surface, or destructive-command confirmation owns input. Pane broadcast must be inactive. Closed tabs and missing terminals are rejected.

Only ordinary local shell/process-profile tabs accept local path insertion, and only authenticated live-domain tabs offer confirmed upload. Termeric refuses drops into:

- plugin tabs, so a local path is not accidentally disclosed to a provider or plugin process;
- serial tabs, where a shell path has no defined meaning; and
- live domains without a captured authority, exact stable pane, known daemon cwd, or an explicitly confirmed real file/directory within the limits.

These refusals are visible and do not fall back to the currently focused local tab.

## Explicitly unsupported

Live-domain upload intentionally does not follow symlinks or mount boundaries, preserve owner/group/timestamps/ACLs/xattrs/hard links/sparse layout, expose overwrite or rename collisions, select an arbitrary destination directory, expose contents to restart-persistent resume or plugins, or forward remote clipboards. If the connection is lost after atomic publication but before the success response, the same-call retry replays the completed result. If every retry path is unavailable until the bounded replay record expires, the remote result can still require manual inspection.

Native compositor delivery and cursor presentation still require acceptance on real X11 and Wayland desktops. The quoting and routing contract is covered by display-free tests; that is not evidence that every file manager/compositor combination delivers native drops correctly.
