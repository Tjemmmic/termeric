# Native Linux candidate evidence

Unit and private-compositor tests cannot establish real desktop policy, input
method behavior, screen-reader usability, font quality, fractional scaling, or
vendor-GPU behavior. Candidate evidence for those boundaries uses the
schema-versioned native acceptance record instead of free-form release notes.

## Deferred OSC 99 notification lifecycle check

Run these commands inside the candidate Termeric build on a Linux desktop with
a freedesktop notification service. They do not invoke an AI provider.

1. Show and then replace one identified notification. The second command should
   update the existing desktop notification rather than add another bubble:

   ```bash
   printf '\033]99;i=termeric-replace;Replacement version one\033\\'
   printf '\033]99;i=termeric-replace;Replacement version two\033\\'
   ```

2. Show and explicitly close one notification. The second command should remove
   only the notification created by the first:

   ```bash
   printf '\033]99;i=termeric-close;Close this notification\033\\'
   printf '\033]99;i=termeric-close:p=close;\033\\'
   ```

3. Show a notification with a five-second lifetime. It should disappear without
   further terminal output or scrolling:

   ```bash
   printf '\033]99;i=termeric-expiry:w=5000;Expires after five seconds\033\\'
   ```

4. Open two panes and run one show command in each. Two independent
   notifications should appear despite sharing an ID. Run the close command in
   the first pane; its notification should close without closing the second:

   ```bash
   # 1. First pane
   printf '\033]99;i=termeric-origin;Notification from pane one\033\\'

   # 2. Second pane
   printf '\033]99;i=termeric-origin;Notification from pane two\033\\'

   # 3. First pane
   printf '\033]99;i=termeric-origin:p=close;\033\\'
   ```

5. From Bash, request a close report, then dismiss the notification using the
   desktop notification UI. The command temporarily disables canonical input,
   waits at most 20 seconds, restores the TTY, and prints the reply as hex. It
   should contain one OSC 99 `i=termeric-report:p=close` reply ending in
   `1b 5c`; it must not appear a second time:

   ```bash
   old_stty=$(stty -g)
   trap 'stty "$old_stty"' EXIT INT TERM
   stty -echo -icanon min 1 time 0
   printf '\033]99;i=termeric-report:c=1;Dismiss this notification\033\\'
   IFS= read -r -t 20 -d '\' reply
   read_status=$?
   stty "$old_stty"
   trap - EXIT INT TERM
   if [ "$read_status" -eq 0 ]; then printf '%s\\' "$reply" | od -An -tx1; else printf 'timed out waiting for close report\n'; fi
   ```

   Repeat using an explicit `p=close` command and a positive-expiry request.
   Both should wait for the desktop's `NotificationClosed` signal before
   reporting. If the service accepts the close but never confirms it, Termeric
   should show one warning and reply with the literal payload `untracked` after
   about five seconds.

6. From Bash, create two non-expiring named notifications and query their live
   IDs. The hex output should decode to one owner-local, sorted reply ending in
   `termeric-alive-a,termeric-alive-b` (with the normal OSC terminator). Closing
   either notification and querying again should remove only that ID:

   ```bash
   printf '\033]99;i=termeric-alive-b:w=0;Alive B\033\\'
   printf '\033]99;i=termeric-alive-a:w=0;Alive A\033\\'
   old_stty=$(stty -g)
   trap 'stty "$old_stty"' EXIT INT TERM
   stty -echo -icanon min 1 time 0
   printf '\033]99;i=termeric-query:p=alive;\033\\'
   IFS= read -r -t 5 -d '\' reply
   read_status=$?
   stty "$old_stty"
   trap - EXIT INT TERM
   if [ "$read_status" -eq 0 ]; then printf '%s\\' "$reply" | od -An -tx1; else printf 'timed out waiting for alive reply\n'; fi
   printf '\033]99;i=termeric-alive-a:p=close;\033\\'
   printf '\033]99;i=termeric-alive-b:p=close;\033\\'
   ```

   Repeat the query in a second pane that did not create the notifications; its
   reply must have an empty payload. Optionally send `printf '\033 G'` before
   the query to select S8C1T: the reply must then begin with byte `9d` and end
   with `9c`. Restore S7C1T afterward with `printf '\033 F'`.

7. From Bash, request activation and close reports for a notification with two
   buttons. Click the second button. The two hex records should contain exactly
   one activation reply with `i=termeric-action;2` and one
   `i=termeric-action:p=close` reply. Their order should match the desktop
   service's ordered signals, and neither may be delivered to another pane:

   ```bash
   old_stty=$(stty -g)
   trap 'stty "$old_stty"' EXIT INT TERM
   stty -echo -icanon min 1 time 0
   buttons=$'Open\u2028Ignore'
   printf '\033]99;i=termeric-action:d=0:p=buttons:a=report,-focus:c=1;%s\033\\' "$buttons"
   printf '\033]99;i=termeric-action;Choose an action\033\\'
   for expected_reply in 1 2; do
     IFS= read -r -t 20 -d '\' reply
     read_status=$?
     if [ "$read_status" -eq 0 ]; then printf '%s\\' "$reply" | od -An -tx1; else printf 'timed out waiting for reply %s\n' "$expected_reply"; fi
   done
   stty "$old_stty"
   trap - EXIT INT TERM
   ```

   Repeat by clicking the notification body rather than a button. The
   activation reply should then have an empty payload after the semicolon.
   Optionally query `p=?` first and confirm the response includes
   `p=title,body,close,alive,buttons` and `a=report`.

8. With desktop notification sounds enabled, compare the default, silent, and
   error policies. The first follows daemon policy, the second must request
   suppression, and the third should use the desktop's error sound when the
   daemon supports notification sounds:

   ```bash
   printf '\033]99;i=sound-default:s=c3lzdGVt;System sound policy\033\\'
   printf '\033]99;i=sound-silent:s=c2lsZW50;This notification is silent\033\\'
   printf '\033]99;i=sound-error:s=ZXJyb3I=;Error sound policy\033\\'
   ```

   A daemon may ignore named sounds; record its name/version and advertised
   sound capability. `p=?` should still report Termeric's exact bounded
   `s=system,silent,error,warn,warning,info,question` vocabulary.

Record the desktop environment and notification-daemon version, whether each
observation passed, and a screenshot or short recording. A visible warning
toast is the expected failure mode if the session bus or notification service
cannot complete an operation.

## Create a candidate record

Generate one complete template from the exact candidate commit:

```bash
python3 scripts/native_acceptance_record.py template \
  --output target/native-acceptance/record.json \
  --operator "YOUR NAME"
```

The tool records the current Git SHA, Termeric version, and UTC time and refuses
to overwrite an existing record. It also refuses a dirty worktree or a supplied
SHA that does not equal checked-out `HEAD`; candidate evidence cannot represent
uncommitted source. The template contains exact cases for:

- real X11 and Wayland visual, fractional-scale, font, emoji, resize, and
  fullscreen behavior;
- Kitty OSC 66 integer/fractional text sizing, explicit-width placement,
  clipping, overwrite behavior, and expanded block/beam/underline cursors;
- clipboard/primary selection, OSC 52, URL, notification, and audible-bell
  desktop policy on both display stacks;
- lost/outdated-surface and device recovery, PTY survival, idle behavior, and
  observed GPU memory on a named vendor driver;
- IBus and Fcitx 5 CJK/dead-key/preedit/focus-cancellation behavior under X11
  and Wayland; and
- Orca/AT-SPI focus, text, updates, actions, modal isolation, and hidden-content
  behavior under X11 and Wayland.

Each case records its own distro, kernel, desktop session, backend, compositor,
GPU/driver, scale, locale, and applicable input-method or screen-reader version.
It also records `termeric --version` output and the tested executable's SHA-256
digest. This avoids treating evidence from one build or environment as proof for
another.

## Record results

Every check uses one of five exact values: `pass`, `fail`, `unavailable`,
`accepted_limitation`, or `unrecorded`. The case disposition must be the value
derived from its checks: any failure is `fail`; all-pass is `pass`; an accepted
limitation takes precedence over unavailable; and any unrecorded check keeps the
case unrecorded. Non-pass cases require explanatory notes. `unavailable` is
evidence of a missing prerequisite, never a pass.

Pass or fail checks require at least one evidence file. Keep evidence under the
record's directory and use relative paths. Useful artifacts include compositor
and Termeric logs, PTY transcripts, screen-reader speech/log output, screenshots,
short recordings, desktop-policy observations, and GPU-tool captures. Terminal
artifacts may contain secrets or private output; review and redact them before
sharing.

After adding artifact paths and descriptions, bind the files to the record:

```bash
python3 scripts/native_acceptance_record.py seal \
  target/native-acceptance/record.json
```

Sealing refuses missing files, symlinks, absolute paths, and paths outside the
record directory, then records each SHA-256 digest.

## Validate evidence

During testing, validate template structure while allowing unfinished rows:

```bash
python3 scripts/native_acceptance_record.py validate \
  target/native-acceptance/record.json --allow-incomplete
```

Validate a completed record and every local artifact/digest:

```bash
python3 scripts/native_acceptance_record.py validate \
  target/native-acceptance/record.json \
  --expected-sha "$(git rev-parse HEAD)"
```

For a release policy that requires every native row to pass rather than allowing
an explicitly reviewed limitation or unavailable prerequisite, add
`--require-pass`. The validator rejects missing/extra cases or checks,
environment placeholders, inconsistent dispositions, evidence-free pass/fail
claims, path traversal, missing files, and digest changes.

Authenticated Claude, Codex, or other provider workflows are outside this
record and must not be invoked as part of native Linux acceptance.
