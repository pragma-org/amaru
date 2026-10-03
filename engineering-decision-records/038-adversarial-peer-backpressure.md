---
type: architecture
status: proposed
---

# Adversarial peer behaviour under the pure-stage actor model

One remote peer must not be able to stop this node talking to any other peer, or to stop a shared stage from finishing its current transition. This record decides how messages move from shared stages through the connection and the mux to a mini-protocol handler when the peer on the far side of a socket can refuse to read, refuse to answer, or answer arbitrarily slowly.

It does not change which wire messages are legal. That stays with the mini-protocol state machines ([EDR-021](./021-switching-to-own-mini-protocols.md), [EDR-036](./036-session-types-typestate-projection.md)).

## Motivation

A pure-stage stage handles one message at a time. While it awaits an effect it does not read its mailbox. `eff.send` does not return until the destination mailbox has accepted the message. The production bulk mailbox holds 10 messages. Those two facts compose:

- A stage that awaits a send to a stage that is itself not reading will stop reading too, once that mailbox is full.
- The stall walks back along every such send. A shared stage that fans out by awaiting each send in turn stops at the first stalled peer and never reaches the rest, and never finishes its own transition.

That is what a live node did. Block fetch adopted slot 124271346 and asked five peers; the batch completed when the first body arrived. The other four block-fetch initiators were left inside `eff.call` to their mux. The next header (slot 124271358) was selected and a new fetch was handed to the manager. The manager's `FetchBlocks` transition never finished, and `block.requested` was never logged, because `PeersAsked` is sent only after every per-connection `eff.send` returns:

```rust
for conn in /* chosen connections */ {
    eff.send(&conn.stage, ConnectionMessage::FetchBlocks { .. }).await;
}
eff.send(&cr, Blocks::PeersAsked(id, contacted)).await;
```

The fetch stage had already returned from its own send (the manager mailbox had accepted the message) and had armed a 5s timeout. Each timeout asked chain selection to resume, which queued another `FetchBlocks` behind the stuck manager transition. When the manager mailbox filled, the next send from the fetch stage blocked *before* the timeout was armed. Chain selection's `may_fetch_blocks` was already false, so later headers updated the candidate tip and were never pushed. Peers stayed connected the whole time.

Two separate defects make this likely.

**`eff.call` does not time out while it is still queueing.** In the Tokio interpreter the timeout starts only after `Sender::send` has queued the request (`crates/amaru-pure-stage/src/tokio.rs`, the `StageEffect::Call` arm). A mux that is not reading keeps the caller inside `call` forever, so the caller's mailbox is not read either. The simulation interpreter does the opposite: it arms the deadline when the effect starts, and on expiry it drops a request that has not yet been admitted (`resume_call_send_internal` in `simulation/running/resume.rs`). The external `Sender::call` helper also bounds the enqueue. The stage effect is the one that does not. A caller that ignores `None` and still steps into remote agency then waits out the 60s block-fetch agency timeout for a request that was never sent.

**The mux awaits the handler it is delivering to.** `PerProto::received` and `want_next` do `eff.send(&handler, FromNetwork)` and do not return until that mailbox accepts. A handler stuck in `call` is not reading. Once its mailbox is full the mux stops reading, so every protocol on that connection stops, and every `call` into that mux stops at enqueue. The same shape appears one level up: the connection awaits the handler (`FetchBlocks`, `NewTip`, `Close`), and the manager awaits every connection (`FetchBlocks`, `NewTip`). `track_peers` awaits `RequestNext` on a per-peer chainsync handler while handling headers from every peer.

Waiting for TCP is already isolated: the per-connection writer is the only stage that awaits a socket write, and the mux keeps a single SDU outstanding (`sending`). That split is right. The stall above happens before any byte waits on the socket. It is a cycle of mailbox admission between stages of one node.

Enlarging every mailbox only moves the same cycle later. Any fixed capacity fills if a stage waits without a bound.

## Decision

### Who may wait for whom

A stage is **shared** when it is not owned by one peer connection. The manager, fetch, chain selection, and `track_peers` are shared. A stage is **peer-scoped** when it belongs to one connection: the connection stage, its mux, its reader, its writer, and each mini-protocol handler.

1. A shared stage never awaits admission of a message into a peer-scoped stage. It uses `try_send` (below). `Full` or `Gone` is that peer's outcome. The shared transition still completes, and the other peers are still attempted.
2. Peer-scoped stages must not form an admission cycle. Concretely the mux does not await a handler, and the connection does not await a handler or the mux. A handler may `call` its own mux. It may await a shared stage: that is ordinary backpressure onto this peer alone, and it does not stop the mux as long as rule 1 and the mux rule below hold.
3. The reader and the writer are the only stages that await the socket. A peer that does not read stops that connection's writer and nothing else.

Blocking `eff.send` stays the right tool between shared stages. Fetch may still await the manager. Validation may still await the ledger. Those waits are local backpressure, and they are safe only because a shared stage's transition no longer contains an unbounded wait on a peer.

### `try_send`

`Effects::try_send` admits a message or returns immediately:

- `Queued` — the message is in the destination mailbox.
- `Full` — it was not admitted, and it will not be admitted later.
- `Gone` — the destination stage no longer exists. Same as today's failed `send`.

Both interpreters implement it. The simulation does not park the sender on the destination's `senders` queue. The trace records the attempt and the outcome so a test can require that a full mailbox skips a peer instead of suspending the sender.

`try_send` is not a license to drop work that must not be lost. The caller coalesces, as specified below. Coalesced state is a constant amount of data, never a queue that grows with the peer's silence.

### `eff.call` agrees on both runtimes

The timeout passed to `Effects::call` covers admission into the callee mailbox, and it starts when the effect starts. It does not cover a request that has already been admitted. Dropping the reply oneshot does not pull an admitted request back out: the callee can still enqueue the payload and write it.

- If the deadline fires before admission, the send is cancelled. The request must not enter the callee mailbox afterwards. Tokio drops the `send` future; the simulation removes the pending sender. This outcome is `NotSent`. The bytes will not be transmitted.
- If the deadline fires after admission, that is not `NotSent` and it is not `Sent`. Typestate `call` has no token for it, and the handler is not resumed onto either arm. `NotSent` would let the handler submit a second copy while the first is still in the egress buffer; the peer's answer to the first would then arrive in the wrong local state. `Sent` would claim a reply the mux has not made. The connection is faulted instead.
- The mux must make that second case unreachable. In the transition that dequeues a `Send`, before any further await, it either replies with the mux message `Sent` or rejects the payload. The reply `Sent` means the bytes are in the one-segment cap, including when the writer is already busy. It is not deferred until `next_segment` runs. A rejection appends nothing. An explicit rejection is `NotSent`. A `Sent` reply becomes the success token. Either reply means only what the mux decided about its own buffer. Neither means the peer has read the bytes.

This is the bug fix, not a new timeout value. `NETWORK_SEND_TIMEOUT` (1s) is already what handlers pass, and it bounds admission only. A regression test has a callee that never reads: the caller observes `NotSent` within the timeout, and the callee's mailbox does not gain the request afterwards. A second test admits the request and only then lets the deadline fire: the caller must not observe `NotSent` or `Sent`.

### `NotSent` stays in the switch state

A handler that takes the failure token has not put a message on the wire. It stays in the switch state (block-fetch `Idle`), does not send `WantNext`, and does not arm the agency timer. The pipeline slot is free for the next local request. Entering `Busy` and waiting 60s for an answer to a message that was never sent is what turns one slow mux into an occupied slot.

The `NotSent` arm tells the collector `Blocks::NoBlocks(id, peer)`. `NoBlocks` means no block was obtained for this range. It is not `FetchBlocksMsg::Timeout`. Timeout scores peers that were asked and did not deliver before the batch timer. `NoBlocks` is the empty reply: the handler already sends it when the peer answers `NoBlocks` on the wire, and it is the same reply when this node was too slow to receive the request and therefore never submitted it. `no_blocks` does record a fetch failure for that peer. Scoring a peer that produced no block, including because the request never left this node, is the decision. There is no separate local-admission message, and this path does not tear the connection down.

A connection whose `try_send` of `FetchBlocks` returns `Full` does not emit `NoBlocks`. That peer was never asked, is absent from `PeersAsked`, and is not scored for this request.

The result of typestate `call` is not `Option<Reply>`. `Option` lets the handler discard the reply and `finish` into the next state, which is what `let (_, s) = ….call(…).await; s.finish()` does today. The result is an outcome enum whose variants each hold a token. The token is fed back into the session, and that is what yields the remainder for that arm. `finish` is not available on the session `call` returns. Feeding a token the call did not return does not compile: the token types have a private field, and only `call` constructs them.

```rust
enum Submitted<T> {
    Sent(Sent<T>),
    NotSent(NotSent),
}

on_receive!(Idle as PipelineIdleIn {
    Fetch => {
        Call<ToMux, RequestRange> => {
            Sent<RequestRange> => Busy
            | NotSent => { SendAny<ToCollector> => Idle }
        }
    }
});

let (outcome, session) = idle.receive(&fetch, eff).call(&mux, range).await;
match outcome {
    Submitted::Sent(token) => session.feed(token).finish(), // Busy
    Submitted::NotSent(token) => {
        session.feed(token).send_any(&collector, Blocks::NoBlocks(id, peer)).await.finish()
    }
}
```

`Sent<T>` is the success token and carries the wire payload. `NotSent` is the failure token. A reply value, when a call has one, rides inside the success variant; the token, not the value, selects the remainder. `ClientDone` uses the same enum. Its success token is the only way into `Done`.

The nested block is not a `|` the handler may pick. Both arms sit behind the `Call`, and `feed` is the only way into either of them. `Feed<Sent<RequestRange>>` yields the `Busy` remainder. `Feed<NotSent>` yields the collector remainder. A token from another call is a different type and does not implement `Feed` for this remainder.

Projection changes to match ([EDR-036](./036-session-types-typestate-projection.md)):

- A `Call` is ignored. It is mux admission, not a wire send, even when its payload is in `wire_payload`. Today the opposite is true: `Call<ToResponder, RequestRange>` is why the projected machine has `Idle --> Busy: RequestRange`.
- The wire send is deduced only from a success token being supplied. `feed(Sent<RequestRange>)` is the edge `!RequestRange`. Structural equality compares that edge to the network spec, and `check_timeouts` / `check_want_next` follow only that arm into remote agency.
- `feed(NotSent)` emits no wire edge. The arm is checked, not compared: no peer payload, no `WantNext`, no agency timer, finishes in the switch state it started from, and its only visible effect is the collector signal.
- A `Call` whose success token is never supplied contributes no wire edge, so the spec comparison fails. Performing the call is not enough to claim the bytes were sent.
- A choice that is not the token continuation of a `Call` is unchanged. `MixedHidableWireChoice` still rejects a local input that mixes a wire arm and a hidable arm on its own.

The network-spec diagram does not gain an edge. This is a change to what the projection treats as a send, not to which wire messages are legal.

### The mux never waits on a handler

Ingress (`FromNetwork`, `Registered`) is delivered with `try_send`.

- `Queued`: credit is consumed as today.
- `Full`: the frame stays in the per-protocol buffer. That buffer is already capped (`ingress_limit`). Credit is not consumed. The mux finishes the transition.
- One coalesced priority wakeup per mux retries deferred deliveries. It is re-armed only while some protocol still has a deferred frame, with a delay on the order of `NETWORK_SEND_TIMEOUT`, so a handler that was inside a single `call` has resumed and read. It is one timer, not one per frame, and it stays inside the priority-mailbox budget.

If a deferred frame is still undeliverable after the handler's agency timeout, that handler is faulted and the connection supervisor tears **that** connection down. The mux does not spin, and no other connection is involved.

Egress keeps today's split with the writer, tightened:

- At most one SDU is admitted to the writer. The mux therefore cannot observe a full writer mailbox; a `try_send` of `Full` toward the writer means the invariant broke, the segment stays queued, and the mux still does not block.
- Each protocol's unsent egress is capped at one segment (64KiB, the existing `MAX_SEGMENT_SIZE`). The mux message `Sent` means "accepted into that cap", not "written to the socket", and it is sent in the dequeue transition, before the mux awaits the writer or anyone else. Typestate `call` turns that reply into `Submitted::Sent`. When the cap is full, the same transition rejects the payload and appends nothing; typestate `call` turns that rejection into `NotSent`. Unbounded `PerProto::outgoing` growth while a writer is stuck on TCP is not allowed. A `Send` that has been dequeued and then left unanswered is not given either token.

### The connection never waits on a child

Forwarding `FetchBlocks`, `RequestSharePeers`, `NewTip`, `Close`, and `Done` is `try_send`.

- Block fetch: on `Queued`, the connection sends `PeersAsked(id, [this peer])` to the collector. On `Full`, or when no initiator is running, it sends nothing for that peer. The manager no longer emits `PeersAsked`; it does not know whether the child admitted the request. The fetch stage already unions late `PeersAsked` messages.
- The manager emits `NoPeersAvailable` only when it has no candidate connection to attempt. Every candidate being `Full` is not "no peers": nobody was asked, the fetch stage's existing timeout retries, and those peers are already in `asked`, so the three widen wakeups do not select them again.
- `NewTip`: on `Full` toward the chainsync responder, the connection stores that one tip over any previously stored tip and flushes it with `try_send` at the start of its next transition. The latest tip is the only one that matters. A `NewTip` that does not fit in the **connection's** mailbox is skipped by the manager; the next header retries. No per-peer queue is kept on the manager.
- Shutdown `Close` / `Done`: on `Full`, the connection still records the child in `stopping` and still arms the stop timer. Parent termination already aborts a child that does not leave by itself. A child that is not reading must not be able to postpone `Disconnect` or `ChildDied`.

While a connection is still handshaking it may keep today's behaviour of rescheduling the message onto itself after the reconnect delay. That wait is bounded and does not touch another peer.

### Shared fan-out

The manager's `FetchBlocks` and `NewTip` arms, and `track_peers`'s `RequestNext` and `Done` sends, become `try_send` loops. A `Full` peer is skipped; the loop continues; the transition returns.

`track_peers` does not collapse a failed `RequestNext` into one owed retry. The initiator reaches depth `PIPELINE_DEPTH` only on `IntersectFound`, which is the one send of `RequestNext(PIPELINE_DEPTH)`. After that the window moves by one local `RequestNext(1)` per header, and it shrinks by one for every request that does not hit the wire. Nothing else refills it. Restarting the mini-protocol is the only path back to a full window today, so a forgotten drop is a permanently shorter pipeline.

The counter counts requests that still have to be sent. It is capped at `PIPELINE_DEPTH`. A retry of a request already in the counter is not a new drop.

- A newly generated `RequestNext` (the replenishment for a header just processed) whose `try_send` returns `Full` or `Gone` increments the counter, saturating at `PIPELINE_DEPTH`. The handler never saw that request.
- A retry of a slot already counted, whose `try_send` returns `Full` or `Gone`, does not increment. The same missing request is still owed once. Incrementing again would record it twice, and two later `Queued` results would send two `RequestNext`s for one drop, past `PIPELINE_DEPTH`.
- `Queued` decrements by one. The decrement means the handler admitted that one request. It does not mean the bytes are on the wire.
- If the handler then feeds the failure token, the bytes were not sent. The handler does not increment `CanAwait` / `MustReply` and does not send `WantNext`. It reports the drop to `track_peers` (a local result, not a wire message), and that report increments the counter. The earlier `Queued` had removed the slot; this puts it back. That is a new fact, not a second count of the retry. `track_peers` cannot see the token on its own. Chain sync's initiator is not on typestate yet; the failure token is how this drop is reported once that handler moves.

While the counter is non-zero, `track_peers` tries to admit one counted `RequestNext` at a time: on the next event for that same peer, and from one coalesced self-wakeup. Only `Queued` decrements. Further newly generated drops once the counter sits at `PIPELINE_DEPTH` are not counted. The window is never deeper than that, and a stuck handler stops producing new headers once its own mailbox and the `track_peers` mailbox have drained, so the uncounted tail does not grow with the peer's silence. Header processing for other peers still continues. Forgetting a new drop would leave this peer's window short until the next intersection.

### Pipeline slots do not die when they are busy

The block-fetch pipeliner (`N = blockfetch_pipeline_n`, default 2) currently terminates the handler when a local request arrives and the send cursor is not in the switch state. Termination is delivered as `ChildDied`, and the connection tears the socket down. A third range must not do that.

When no slot is idle the pipeliner stashes one `Fetch`. A newer stash replaces the older one: only the latest range is worth sending when a slot returns to `Idle`. `Close` stays sticky, as `pending_close` already is on the lock-step instance. The instance's existing `pending_fetch` path is the behaviour; the pipeliner has to stop rejecting the message before the instance can see it.

An in-flight range whose success token was fed is in remote agency. Nothing in this design aborts it on the wire. `ClientDone` is only legal from `Idle`. That slot stays busy until the peer answers or the agency timer fires, and during that wait the handler is reading its mailbox. The next range uses another idle slot or the one stash. Other peers are unaffected because nobody awaits this handler.

### Liveness timers are armed first

A stage that uses a timeout as its liveness mechanism arms it before any send that might wait. The fetch stage today sends `FetchBlocks` to the manager and only then schedules the 5s timeout. Once the manager cannot block on a peer, that send returns quickly; the reorder still means a later regression cannot both stall the manager and disarm the retry. The timeout stays at 5s and still calls `FetchNextFrom`.

### Mailbox capacity

The default bulk mailbox stays 10. Capacity becomes per stage, chosen when the stage is built, on both interpreters. Raising the global default is not the fix.

The mailbox has to hold the messages **this node** may have in flight while a handler awaits one bounded `call`. It does not have to hold a peer's pipeline. Peer bursts sit in the mux byte buffer and are delivered by the deferred retry above.

| Stage | Capacity | Why |
| --- | --- | --- |
| Shared stages, connection, reader, writer, responders that do not pipeline | 10 | Unchanged default. |
| Block-fetch handler | `max(10, 2 * N + 4)` | One local request and one network message per pipeline slot, plus `Registered`, `Close`, and the stashed newer range. `N = 2` stays within 10. |
| Chain-sync initiator | `PIPELINE_DEPTH + 4` (14) | Up to `PIPELINE_DEPTH` (10) local `RequestNext`s may be admitted while one `call` is in progress, plus a few network and control messages. |
| Mux | 24 | A hot duplex connection runs up to ten handlers (initiator and responder for keep-alive, peer sharing, chain-sync, block fetch, and tx submission). One `Send` and one `WantNext` from each, plus `FromNetwork` and `Written`, is 22; 24 leaves room for a `Register` or `SetSduTimeout` in the same burst. |

24 and 14 are the ceilings, not a starting point for tuning upward. A protocol that wants a deeper window states a larger `N` and gets the matching mailbox from the formula. Anything beyond that is a bug in the caller's coalescing, not a reason to grow the channel.

## Consequences

- A peer that stops reading, or a handler that stops reading, delays only its own socket's writer and its own protocol. The manager finishes every fan-out. Fetch keeps arming timeouts. Chain selection keeps being asked for the next tip, so a newer header is pushed once the current attempt ends.
- `PeersAsked` names peers whose block-fetch handler actually admitted the request, which is what the timeout scorer wants. A peer skipped because its connection mailbox was full is not asked and is not scored. That skip does not emit `NoBlocks`. `NoBlocks` is the empty reply from an attempt the handler did run.
- Typestate `call` returns `Submitted::Sent` or `Submitted::NotSent`, never a post-admission timeout. `NotSent` means the bytes will not be transmitted: cancelled before admission, or rejected by the mux with nothing appended. Handlers that discard the reply and `finish` (`let (_, s) = ….call(…).await; s.finish()`) stop compiling. The network-spec diagrams do not gain a wire message. `project` ignores `Call` and compares the edge deduced from the success token.
- Chain sync keeps a per-peer deficit counter so a `RequestNext` that never hits the wire is retried without restarting the mini-protocol. A failed retry of a slot already in the counter does not increment it. The counter and the local "not sent" result are new. The wire messages are not.
- Simulation traces of manager and `track_peers` fan-out change from a suspending `Send` to a `TrySend` with an admission result. Tests that match those sends need to expect a skip as a normal result, not as a stalled sender.
- Per-stage mailbox size is a new stage-graph knob. The default for every stage that does not opt in stays 10, including in tests that construct a `SimulationBuilder`.
- Shutdown of a mini-protocol that is not reading is bounded by the existing stop timer, not by that handler's mailbox draining.
- The 60s agency timer is unchanged and still applies once a request has actually been accepted by the mux. This design does not add a wire-level cancel for a batch that already completed on another peer.

## Discussion points

- Growing every mailbox until the incident's queue fits was rejected. The manager queued one fetch per 5s timeout and then stopped forever; a larger bound would have stopped later, still without asking another peer. The formulas above are only large enough for our own window.
- Making every `send` non-blocking was rejected. Shared stages should still exert backpressure on each other. The hazard is a wait whose other side is a remote peer, not a wait in general.
- Aborting the other four in-flight block fetches when the first body arrives was rejected. Those handlers, once their `call` has returned, are in remote agency and the spec only allows `ClientDone` from `Idle`. The failure mode to remove is the unbounded `call` admission, not the legal wait for a peer that has agency.
- Replying `Sent` only after the TCP write completes was rejected. It would put the handler's `call` on the writer, and the writer is allowed to sit until the SDU timer (30s) when the peer does not read. `Sent` meaning "inside the one-segment cap" keeps that wait on the writer alone. The caller finds out the send was refused when the cap is full, instead of discovering it 30s later.
- Routing block fetch from the manager straight at the handler, skipping the connection stage, was rejected. The connection is what reschedules across handshake and what decides the initiator exists. The connection answers `PeersAsked` itself so the manager does not have to wait to find out whether the child admitted the message.
- A handler-chosen `|` between "request sent" and "not sent", next to the `Call` rather than behind it, was rejected. The handler could take the wire arm after a failed call, or the failure arm after a successful one, and projection would believe it. The success token is the only evidence the projection accepts that the bytes were submitted.
- Treating every expired `call` deadline as `NotSent` was rejected. After the mux has the payload, the deadline no longer proves that the bytes will not be written. That case faults the connection. It does not take the unsent arm, and it does not take the sent arm.
