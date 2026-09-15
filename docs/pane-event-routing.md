# Pane terminal-event routing

Every live terminal is polled regardless of tab activity or pane focus. Pane
mode no longer has a parser-only shortcut: it uses the same bounded PTY/event
bridge as a normal tab.

## Authoritative path

1. Each PTY reader sends at most 256 queued 4 KiB `PtyEvent` items. When that
   queue is full, the reader blocks and applies backpressure to the operating
   system PTY instead of allocating an unbounded process queue.
2. `TabManager::poll_events()` visits every tab. A pane-owning tab delegates to
   `PaneManager::poll_events()`, which visits every pane, not only the focused
   pane.
3. Each output chunk is offered to that terminal owner's transcript/diff state and
   then parsed into its terminal. Harvest observes each terminal's semantic
   state separately after polling; inactive-tab activity is marked without
   changing focus.
4. Terminal events are drained after every output chunk. `PtyWrite` replies
   such as DSR/graphics acknowledgements are written immediately through the
   exact `PtyHandle` beside that terminal. They cannot pass through focused
   input routing.
5. App-facing events are tagged with stable `tab_id` and optional `pane_id`.
   App handles them before pane cleanup. An app-mediated response, such as an
   OSC 52 clipboard query reply, uses `TabManager::write_to_origin()` and fails
   if that exact origin is gone; it never falls back to focus.
6. Closed panes are removed only after routed events from the poll have been
   handled. An explicitly user-closed pane drops its receiver, which releases
   a blocked bounded reader instead of leaving a detached producer.

There is no second `process_all_events`/trait path. The public orchestration
verb is `poll_events` at the tab, tab-manager, pane, and pane-manager levels.

## Queue and loss policy

At most 1,024 app-facing terminal events are forwarded by one terminal/pane
manager/tab-manager poll. Protocol replies are always consumed even after the
app-event ceiling is reached. Excess app-facing events from a hostile
bell/notification flood are deliberately discarded and logged once per owner;
terminal pending vectors are still fully drained. This bounds the bridge while
preserving request/response correctness.

Kitty OSC 99 occasion policy is evaluated against the event's exact origin.
Active/focused state, sibling-pane visibility, zoom, and native window
focus/occlusion determine `unfocused` and `invisible`; stale origins are
discarded. Suppression happens before either notification worker, rate-limit
state, icon decoding/cache work, or lifecycle tracking is touched.

Forwarded bells and ordinary notifications enter their existing global rate
limits, then submit platform helpers to a separate 16-request worker queue with
an eight-child ceiling. Identified, explicitly expiring, reporting, buttoned,
non-default-sound, or transmitted-icon Linux OSC 99 requests instead enter a
64-request worker
and retain at most 64
origin-scoped native handles plus 64 pending close reports. One bounded ordered
native signal listener feeds `ActionInvoked` and `NotificationClosed` back
through that coordinator. Requested activation/close reports and alive-query
snapshots retain their captured origin and S7C1T/S8C1T framing;
the app writes them only to that exact live tab or pane. If the origin has
closed, the reply is discarded and never falls back to the focused terminal.
Both paths report bounded, deduplicated launch, worker, capacity, and lifecycle
failures as warning toasts, so an unavailable desktop service cannot silently
fail or grow process/toast state without limit.

Transcript backpressure has its own byte/entry policy documented in
[background-io.md](background-io.md). The pane output hook does not retain an
additional raw-output queue.

## Regression evidence

Automated tests cover:

- two real simultaneous pane PTYs issuing DSR requests and reading their own
  cursor-position replies;
- an inactive pane-owning tab emitting bells/notifications and receiving DSR
  replies without changing the active tab;
- an app-mediated clipboard reply addressed to an unfocused pane while proving
  the focused pane receives nothing;
- a four-times-over-budget bell/notification stream that is capped and fully
  drained while its protocol reply still succeeds;
- close during event-heavy output; and
- a deliberately undrained PTY reader that stops at the 256-item queue ceiling.
- fake desktop helpers that prove success, failure deduplication, recovery,
  message bounds, and application-visible warning routing.

OSC title, clipboard, and dynamic-color generation now use this stable origin
path and are documented in [osc-integration.md](osc-integration.md). Focused
operation routing is documented in [focused-pane-routing.md](focused-pane-routing.md);
process ownership and
divider rendering are documented in [pane-lifecycle.md](pane-lifecycle.md).
