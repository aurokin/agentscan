# AUR-173 Issue Plan: Add Daemon Subscription Fan-Out With Bounded Subscriber Cleanup

## Scope

Implement long-lived daemon socket `subscribe` clients on top of the snapshot socket server from `AUR-172`.

This issue adds:

- subscribe-mode registration after a compatible hello, current snapshot availability, and subscriber capacity check
- `hello_ack` plus bootstrap snapshot for accepted subscribers
- latest-wins fan-out for later `SnapshotEnvelope` updates
- bounded pending handshakes and bounded registered subscribers
- paired EOF/protocol monitoring so disconnected subscribers are retired without waiting for another tmux event
- post-handshake client-write handling as a protocol violation that retires the subscriber
- focused synthetic socket tests for bootstrap, updates, latest-wins mailbox behavior, capacity limits, EOF cleanup, and protocol-violation cleanup

## Non-Goals

- Do not migrate the TUI to subscriptions yet; that is `AUR-177`.
- Do not migrate one-shot commands to socket snapshots; that is `AUR-176`.
- Do not add detached lifecycle commands, lock/log sidecars, daemon status subscriber fields, or stop/restart ownership; those remain `AUR-174`.
- Do not remove cache writes or cache command surfaces.
- Do not add delta frames or per-pane update frames; every update remains a full `SnapshotEnvelope`.
- Do not add platform-specific socket peer credential checks.

## Implementation Outline

1. Extend IPC reasons narrowly.
   - Add shutdown/unavailable variants for server pressure and subscriber refusal, likely:
     - `ShutdownReason::ServerBusy` for pre-hello pending-handshake refusal.
     - `UnavailableReason::SubscriberLimitReached` for valid subscribe clients when subscriber capacity is full.
   - Keep protocol/schema mismatch behavior unchanged: no `hello_ack`, explicit shutdown.
   - Keep `SubscribeUnavailable` only for transitional or unsupported states if still useful; accepted subscribe mode should no longer use it.

2. Add bounded daemon socket state and accept handoff.
   - Add constants near existing daemon socket limits:
     - `MAX_PENDING_HANDSHAKES`: small bounded count, default `8`.
     - `MAX_SUBSCRIBERS`: moderate bounded count, default `64`.
     - `SUBSCRIBER_WRITE_TIMEOUT`: short timeout, default `250ms`, separate from one-shot writes.
   - Track `pending_handshakes`, `subscribers`, and `next_subscriber_id` inside `DaemonSocketStateInner`.
   - Reserve a pending-handshake slot before spawning the per-connection handler thread. If the cap is already reached, handle the connection inline by sending an opportunistic current-wire `server_busy` shutdown and closing it.
   - The inline `server_busy` refusal path must set a short write timeout, attempt one best-effort shutdown frame write, and close regardless of write success so the accept loop is not stalled by non-reading clients.
   - Pass a pending-handshake guard into the handler. It decrements when hello handling finishes, including EOF, malformed hello, protocol/schema mismatch, and successful transition into snapshot or subscribe handling.
   - This issue bounds daemon-owned handler work at the accepted-connection handoff. It does not attempt to tune the kernel listen backlog.
   - Keep lock scope short: mutate counters and clone shared frame handles while locked, then perform socket writes outside the lock.

3. Add latest-wins subscriber mailboxes.
   - Use a small `Arc<Mutex<SubscriberMailboxState>>` plus `Condvar` per subscriber rather than unbounded channels.
   - Store encoded snapshot frames as immutable shared bytes, `Arc<[u8]>`, so fan-out clones pointers rather than up to 4 MiB per subscriber.
   - Mailbox state stores at most one pending encoded snapshot frame handle.
   - Publishing replaces the pending frame with the newest frame and notifies the writer.
   - The subscriber writer takes and clears the pending frame before writing so slow socket writes do not hold the mailbox lock.
   - Retiring a subscriber marks the mailbox closed and notifies the writer.
   - Add direct mailbox tests for latest-wins replacement and closed-mailbox behavior, separate from socket I/O tests.

4. Register subscribe clients only when ready.
   - After a valid subscribe hello, call `try_register_subscriber()`.
   - Registration succeeds only when startup state is `Ready`, a current encoded snapshot frame exists, and subscriber capacity is available.
   - Subscriber insertion and bootstrap-frame selection are atomic under the daemon state lock. The subscriber must either receive the exact latest frame chosen at insertion time, or later publications must see its mailbox and enqueue the newer frame.
   - If the daemon is initializing, startup-failed, or closing, return the same unavailable semantics as snapshot clients.
   - If subscriber capacity is full, return `hello_ack`, `subscriber_limit_reached`, and EOF.
   - On success, return a `SubscriberRegistration` with subscriber id, bootstrap snapshot bytes, and mailbox.

5. Serve subscriber connections.
   - Accepted subscribers receive `hello_ack`, the bootstrap snapshot, then future snapshot frames from their mailbox.
   - Split the connection into a short-timeout writer and a reader/protocol monitor.
   - Use one idempotent `retire_subscriber(id)` path for writer failure, monitor EOF, protocol violation, daemon closing, and normal test cleanup. It removes the subscriber exactly once, closes its mailbox exactly once, and returns whether it actually retired an active subscriber.
   - The writer retires the subscriber on write/flush failure or mailbox close.
   - The monitor retires the subscriber on EOF or any post-handshake bytes. Any client write after the hello is a protocol violation; no additional client command surface or protocol-violation response frame is introduced.
   - Avoid joining a blocked writer from the monitor path; writer timeouts and mailbox closure are enough to retire it.
   - Add a tiny writer abstraction if needed to deterministically test write failure/timeout retirement without relying on filling a Unix socket buffer.

6. Fan out on snapshot publication.
   - `publish_prepared_snapshot` and successful `publish_later_snapshot` update the latest-good snapshot and collect subscriber mailboxes while holding the state lock.
   - After releasing the state lock, enqueue the encoded frame into each subscriber mailbox.
   - Oversized later snapshots retain existing `AUR-172` behavior: preserve last good socket snapshot and do not fan out the oversized frame.
   - Subscribers should remain connected after an oversized skipped update and receive the next good update.
   - Snapshot publishing must not block on socket writes or slow subscribers.

7. Keep one-shot behavior intact.
   - Snapshot-mode clients still receive `hello_ack` plus exactly one snapshot/unavailable frame and EOF.
   - Snapshot-mode clients never affect subscriber count.
   - Protocol/schema mismatch behavior remains explicit and no-ack.

## Edge Cases

- Subscribe before initial snapshot returns `hello_ack`, `daemon_not_ready`, and EOF; no subscriber is registered.
- Subscribe after startup failure returns `hello_ack`, `startup_failed`, and EOF.
- Subscribe while closing returns `hello_ack`, `server_closing`, and EOF.
- Subscribe at capacity returns `hello_ack`, `subscriber_limit_reached`, and EOF.
- Pending-handshake capacity reached before hello returns an opportunistic no-ack `server_busy` shutdown frame using the current daemon wire format even though compatibility has not been negotiated, then closes.
- Client EOF before hello decrements pending count and does not register a subscriber.
- Client EOF after subscribe registration retires the subscriber even if tmux is quiet.
- Any post-handshake client bytes retire the subscriber as a protocol violation.
- Slow subscriber writers keep at most one pending update; newer updates replace older pending updates.
- Writer failure or timeout retires the subscriber and closes its mailbox.
- Daemon closing marks state closing, idempotently retires all subscribers, and closes subscriber mailboxes so writers can exit.
- New subscribers after closing receive `server_closing` and do not register.

## Test Plan

Focused tests:

- `cargo test daemon_socket`
- Subscribe bootstrap: valid subscribe client receives `hello_ack`, current snapshot, and remains connected for updates.
- Subscribe live update: `publish_later_snapshot` sends a later snapshot to an existing subscriber.
- Latest-wins mailbox: multiple enqueues before writer drain leave only the newest frame.
- Subscriber limit: accepted subscribers up to capacity, then the next subscribe receives `subscriber_limit_reached` and does not increment count.
- Subscriber capacity recovery: EOF, protocol violation, writer failure, and daemon closing each free capacity exactly once.
- Registration/publication ordering: deterministic state-level test proves subscriber insertion and bootstrap selection cannot miss a concurrent publish; after register plus immediate publish, the subscriber has either bootstrap N or pending N+1, never neither.
- Pending-handshake limit: held pre-hello connections exhaust the pending cap before handler spawn, and the next connection receives a clear opportunistic `server_busy` shutdown.
- EOF cleanup: dropping a subscribed client retires it without another snapshot publication.
- Protocol violation cleanup: writing any post-handshake bytes retires the subscriber and permits a replacement subscriber.
- Oversized skipped update: an oversized later snapshot is not delivered, the subscriber stays connected, and the next good snapshot is delivered.
- Closing: existing subscriber mailboxes close, and new subscribers get `server_closing`.
- Closing idempotency: duplicate retire paths during closing do not double-decrement capacity; the important assertion is closed mailboxes/writer exit plus stable subscriber count, not accepting new subscribers after closing.
- Snapshot-mode regression: snapshot clients still receive one snapshot/unavailable frame and do not increment subscriber count.
- Live daemon integration: connect a subscribe client to a foreground daemon socket, trigger a tmux title/metadata update, and assert the subscriber receives the new snapshot through the real daemon loop.

Regression checks after implementation:

- `cargo fmt --all --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test`

Run the complexity gate because this issue adds daemon concurrency paths:

- `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_arguments`

## Documentation Impact

No user-facing docs rewrite in this issue. Add or adjust narrow internal comments only if the mailbox, pending-handshake guard, or subscriber cleanup contract would otherwise be unclear. Durable socket lifecycle and TUI subscription docs remain part of `AUR-180`.

## Plan Review Notes

Fresh plan review required these changes before implementation:

- Reserve pending-handshake capacity before spawning handler threads, not inside already-spawned handlers.
- Make subscriber retirement single-shot and idempotent through one state API, with tests proving capacity recovery.
- Add a long-lived subscribe client test harness instead of reusing snapshot helpers that close the write side after hello.
- Define pre-hello `server_busy` as an opportunistic current-wire shutdown before compatibility is known.
- Make subscriber insertion and bootstrap-frame selection atomic with publication to avoid lost updates.
- Use shared immutable frame bytes (`Arc<[u8]>`) instead of cloning full frames per subscriber.
- Add deterministic mailbox/writer tests plus one live daemon integration test for real fan-out.
