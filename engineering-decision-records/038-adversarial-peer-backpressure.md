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

**`eff.call` does not time out while it is still queueing.** In the Tokio interpreter the timeout used to start only after `Sender::send` had queued the request. A mux that is not reading keeps the caller inside `call` forever, so the caller's mailbox is not read either. The simulation interpreter already armed the deadline when the effect starts. The external `Sender::call` helper also bounds the enqueue. The stage effect was the one that did not. A caller that ignores `None` and still steps into remote agency then waits out the 60s block-fetch agency timeout for a request that was never sent.

**The mux awaits the handler it is delivering to.** `PerProto::received` and `want_next` used to do `eff.send(&handler, FromNetwork)` and not return until that mailbox accepted. A handler stuck in `call` is not reading. Once its mailbox is full the mux stops reading, so every protocol on that connection stops, and every `call` into that mux stops at enqueue. The same shape appears one level up: the connection awaits the handler (`FetchBlocks`, `NewTip`, `Close`), and the manager awaits every connection (`FetchBlocks`, `NewTip`). `track_peers` awaits `RequestNext` on a per-peer chainsync handler while handling headers from every peer.

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

Both interpreters implement it. The simulation does not park the sender on the destination's `senders` queue.

`Effect::TrySend` is `{ from, to, msg }`. It carries no outcome. The outcome is `StageResponse::TrySend` on the resume. `tm_try_send`, `tm_try_send_type`, and `tm_try_send_match` match the request payload. `tm_resume_try_send` matches the outcome. A test can thus require that a full mailbox skips a peer instead of suspending the sender.

`try_send` is not a license to drop work that must not be lost. The caller coalesces, as specified below. Coalesced state is a constant amount of data, never a queue that grows with the peer's silence.

### `eff.call` agrees on both runtimes

The timeout passed to `Effects::call` and `Effects::call_with_admission` covers admission into the callee mailbox, and it starts when the effect starts. It does not cover work the callee does after it has accepted the request. Dropping the reply oneshot does not pull an admitted request back out: the callee can still enqueue the payload and write it.

`Effects::call` maps both negative results to `None`. `call_with_admission` returns `CallAdmission`:

- `Reply` — a reply arrived before the deadline. For a mux send the reply is the `Sent` token.
- `NotAdmitted` — the deadline fired before admission. The request is never delivered. The variant carries the `CallNotAdmitted` token.
- `TimedOut` — the request was admitted, then the deadline passed. The request stays queued. A late reply is ignored. The variant carries the `CallTimeout` token.

`TimedOut` faults the caller: it logs `protocols::EGRESS_DEADLINE` at warn and terminates. `protocols::EGRESS_DEADLINE` is a public warn event: `proto` and `reason` are required, `peer` is optional.

The deadline the handler passes is not a flat `NETWORK_SEND_TIMEOUT` once the payload is known. See egress below. `NETWORK_SEND_TIMEOUT` (1s) is the floor of that deadline, the worst-case start.
nt`.

### `CallNotAdmitted` stays in the switch state

A handler that takes `CallNotAdmitted` has not put a message on the wire. It stays in the switch state (block-fetch `Idle`), does not send `WantNext`, and does not arm the agency timer. The pipeline slot is free for the next local request.

`NoBlocks` means no block was obtained for this range. It is not `FetchBlocksMsg::Timeout`. Timeout scores peers that were asked and did not deliver before the batch timer. `NoBlocks` is the empty reply: the handler already sends it when the peer answers `NoBlocks` on the wire, and it is the same reply when this node was too slow to receive the request and therefore never submitted it. `no_blocks` does record a fetch failure for that peer. Scoring a peer that produced no block, including because the request never left this node, is the decision. There is no separate local-admission message, and this path does not tear the connection down.

A connection whose `try_send` of `FetchBlocks` returns `Full` does not emit `NoBlocks`. That peer was never asked, is absent from `PeersAsked`, and is not scored for this request.

The result of typestate `call` is not `Option<Reply>`. `Option` lets the handler discard the reply and `finish` into the next state, which is what `let (_, s) = ….call(…).await; s.finish()` does today on the drivers that have not moved. The result is `CallAdmission`, and each negative variant holds a token. The token is fed back into the session, and that is what yields the remainder for that arm. Feeding a token the call did not return does not compile: the token types have a private field, and only the call constructs them.

The nested block is not a `|` the handler may pick. `Sent` and `CallNotAdmitted` are single-input receives. A token from another call is a different type and does not implement the receive for this remainder. (This approach may change once the typestate API ramifications are fully worked out, see [#1463](https://github.com/pragma-org/amaru/issues/1463).)

The block-fetch responder uses the same deferred call. Any admission other than `Reply` faults the handler through `protocols::EGRESS_DEADLINE`, with reasons `start_batch`, `block`, `batch_done`, and `no_blocks`. `WantNext` for the batch is sent only after `BatchDone` is accepted. The peer is not recorded as adversarial as it may just be slow.

The generic driver (`commit_send`) is the same contract for keepalive, peer sharing, handshake, tx-submission, and chain-sync. `Reply(Sent)` then, and only then, sends `WantNext` and stores the new protocol state. `NotAdmitted` logs `protocols::EGRESS_DEADLINE` with reason `not_admitted`. `TimedOut` logs it with reason `deadline`. The generic driver has no peer in scope, so `peer` is omitted (the `mini_protocol` driver will eventually go away once all handlers are written using typestate). Both return without a post-send state, and the driver terminates. That becomes `ChildDied` and connection teardown, with no adversarial score. A step with nothing to send still emits `WantNext`. Chain-sync stage logic is unchanged; it meets this only through the driver. Moving the chain-sync initiator onto typestate, and teaching it to report a request that never hit the wire, is deferred.

Projection changes to match ([EDR-036](./036-session-types-typestate-projection.md)):

- A `Call` is ignored. It is mux admission, not a wire send, even when its payload is in `wire_payload`. Today the opposite is true: `Call<ToResponder, RequestRange>` is why the projected machine has `Idle --> Busy: RequestRange`. The projected edge is `Idle --> Requested: RequestRange`, and the agency timeout sits on `Requested`.
- The wire send is deduced only from a success token being supplied. `feed` of `Sent` is the edge `!RequestRange`. Structural equality compares that edge to the network spec, and `check_timeouts` / `check_want_next` follow only that arm into remote agency. `Sent` staying in `Requested` is single-input, so it adds no peer-visible edge.
- `CallNotAdmitted` emits no wire edge. The arm is checked, not compared: no peer payload, no `WantNext`, no agency timer, finishes in `Idle`, and its only visible effect is the collector signal.
- A `Call` whose success token is never supplied contributes no wire edge, so the spec comparison fails. Performing the call is not enough to claim the bytes were sent.
- A choice that is not the token continuation of a `Call` is unchanged. `MixedHidableWireChoice` still rejects a local input that mixes a wire arm and a hidable arm on its own.

The network-spec diagram does not gain an edge. This is a change to what the projection treats as a send, not to which wire messages are legal.

### The mux never waits on a handler

Ingress (`FromNetwork`, `Registered`) is delivered with `try_send`.

- `Queued`: credit is consumed as today.
- `Full`: the frame stays in the per-protocol buffer. That buffer is already capped (`ingress_limit`). Credit is not consumed. The mux finishes the transition.
- `Gone`: the message is dropped. The handler stage is already gone, so the bytes will not be admitted later.

One coalesced retry per mux retries deferred deliveries. `INGRESS_RETRY_SLOT` is 1. Slot 0 is the default `Effects::set_timeout` and must not replace this timer. The delay is `NETWORK_SEND_TIMEOUT` (1s). The timer is re-armed only while some protocol still has a deferred frame. It is one timer, not one per frame, and it stays inside the priority-mailbox budget.

`Register` is `{ protocol, frame, handler, max_buffer }`. It carries no per-protocol deadline. Every protocol waits the same local-stall bound, `INGRESS_DEADLINE` (5s), measured from the moment the frame became deferred. A frame still deferred at that deadline closes this connection. The mux warns `protocols::mux::FAILED` with operation `muxing` and terminates; the connection supervisor tears **that** connection down. The peer is not scored as adversarial. The mux does not spin, and no other connection is involved. Agency timeouts are the peer's time, not this stall, so they are not the ingress bound.

Egress keeps the split with the writer, and fills a bounded buffer instead of rejecting a message that does not fit in the free space:

- At most one SDU is outstanding (`sending`). The handoff to the writer is `try_send`. `Full` leaves the segment queued and retries on egress slot 2 after `NETWORK_SEND_TIMEOUT` (1s). Slot 2 is armed only for that `Full`. It is not armed because bytes are still waiting to be copied. Ingress stays on slot 1. The three slots do not replace each other. `Gone` toward the writer terminates the mux. A `try_send` of `Full` is the writer mailbox being full, not a reason to block.
- Each protocol's unsent egress holds at most one segment (`MAX_SEGMENT_SIZE`, 65535). A larger payload is split. Bytes of several messages can share a segment. The remainder of a message waits, in arrival order, until a segment leaves room. Nothing jumps the queue.
- `Sent` is delivered when the last byte of **this** message is copied into that buffer. That includes the transition that hands a segment to the writer, when that handoff is what frees the room and the following copy exhausts the message. `Sent` is not "written to the socket", and it is not "the writer has accepted the bytes".
- The caller's deadline is the 1s floor plus the wire time of **this message's own bytes** at the per-lane floor:

```text
wire(0) = 0
wire(n) = n + ceil(n / MAX_SEGMENT_SIZE) * 8
deadline = NETWORK_SEND_TIMEOUT
         + ceil(wire(payload_len) * 8 * 1000 / MIN_PEER_BANDWIDTH_BPS) milliseconds
```

`SEGMENT_HEADER_LEN` is 8. A 96 KiB block is `wire(98304) = 98320` bytes: `ceil(98320 * 8 * 1000 / 500_000) = 1574` ms, plus the 1s floor, 2.574s.

`MIN_PEER_BANDWIDTH_BPS` is 500_000. A peer is expected to sustain 100 Mbps. The fault floor is 500 kbps **per lane**. Below that floor the connection is closed and the peer is not recorded as adversarial. Other lanes are not part of this message's budget. Bytes already queued ahead on the same lane are not part of it either. With a clear lane, an honest 500 kbps peer admits inside 2.574s: the last byte enters the buffer about one segment before it is on the wire. One earlier 96 KiB block on the same lane still admits, at about 2.10s. Two earlier 96 KiB blocks push the last byte to about 3.15s, past 2.574s, and the caller is faulted.

### The connection never waits on a child

Forwarding `FetchBlocks`, `RequestSharePeers`, `NewTip`, `Close`, and `Done` is `try_send`.

- Block fetch: on `Queued`, the connection sends `PeersAsked(id, [this peer])` to the collector. On `Full`, or when no initiator is running, it sends nothing for that peer. The manager no longer emits `PeersAsked`; it does not know whether the child admitted the request. The fetch stage unions late `PeersAsked` messages.
- The manager emits `NoPeersAvailable` only when it had no initiating connection to attempt. When at least one candidate existed and none admitted the request, the manager sends `Blocks::NoneAccepted(id)` at once. Those peers were not asked. The fetch stage's existing timeout retries. A broadcast already covered every initiating connection, so the widen wakeups do not immediately select those full mailboxes again.
- `NewTip`: on `Full` toward the chainsync responder, the connection stores that one tip (`pending_tip`) over any previously stored tip and flushes it with `try_send` at the start of its next transition. `Queued` or `Gone` drops the stored tip. The latest tip is the only one that matters. A `NewTip` that does not fit in the **connection's** mailbox is skipped by the manager; the next header retries. No per-peer queue is kept on the manager. There is no coalesced wakeup: a quiet connection holds the tip until some other message arrives.
- `RequestSharePeers`: on `Full` toward the peer-sharing initiator, the connection stores that one `Start` (`pending_share`) and flushes it the same way. A newer `Start` replaces the stored one. The stored share is cleared when that child is stopped or dies.
- Shutdown `Close` / `Done`: on `Full`, the connection still records the child in `stopping` and still arms the stop timer. Parent termination already aborts a child that does not leave by itself. A child that is not reading must not be able to postpone `Disconnect` or `ChildDied`.

`PeersAsked` stamps latency by the instant this attempt chose the peer, not by `fetch_peers` being empty. The initial selection shares `fetch_started_at` and records that instant in `asked_at` before the send to the manager. A widen stamps the peers it adds at the widen clock. A broadcast names nobody up front, so each later `PeersAsked` falls back to `fetch_started_at`. `record_peers_asked` is grouped by that instant. A peer already settled (`NoBlocks`) is not put back by a late `PeersAsked`.

While a connection is still `Initial` or `Handshake` it reschedules `FetchBlocks`, `NewTip`, `RequestSharePeers`, and `SetLocalUse` onto itself after the reconnect delay (2s by default). That wait does not touch another peer. Whether the number of outstanding reschedules is bounded by the priority mailbox is open; see below.

### Shared fan-out

The manager's `FetchBlocks` and `NewTip` arms, and `track_peers`'s `RequestNext` and `Done` sends, are `try_send` loops. A `Full` or `Gone` peer is that peer's outcome; the loop continues; the transition returns.

`track_peers` keeps a per-peer owed counter, capped at `PIPELINE_DEPTH` (10). The counter counts `RequestNext`s that still have to be admitted. It is not a count of bytes on the wire.

- A newly generated `RequestNext` whose `try_send` returns `Full` increments the counter, saturating at `PIPELINE_DEPTH`. The handler never saw that request.
- A newly generated `RequestNext` whose `try_send` returns `Queued` does not decrement. Any earlier miss is still owed.
- A retry of a slot already counted, whose `try_send` returns `Full`, does not increment. The same missing request is still owed once.
- A retry whose `try_send` returns `Queued` decrements by one. The decrement means the handler admitted that one request.
- `Gone` zeroes the counter immediately. That handler is never offered again. It is not retried until some later `Terminated`.

While any session still owes a `RequestNext`, one coalesced wakeup is armed: `REQUEST_RETRY_SLOT` is 1, and `REQUEST_RETRY_DELAY` is 100ms. Slot 0 stays the default timeout. One wakeup drains **every** owed `RequestNext` the handler will accept. `Full` stops that session for this wakeup; the slot is still owed once, and another try in the same wakeup would not admit it. The timer is re-armed only while something remains owed.

`Done` is one `try_send` and is not in the counter. `Full` and `Gone` on `Done` are not retried. The unknown-intersection path sends `Done` once and returns.

The handler-side report that a `RequestNext` was admitted and then never hit the wire is deferred. Chain sync's initiator is still on the legacy `miniprotocol()` driver, so it cannot yet feed a failure token back into this counter. A `Queued` result today means the handler mailbox accepted the message. It does not mean the bytes were submitted to the mux.

### Pipeline slots do not die when they are busy

The block-fetch pipeliner (`N = blockfetch_pipeline_n`) used to terminate the handler when a local request arrived and the send cursor was not in the switch state. Termination is delivered as `ChildDied`, and the connection tears the socket down. A further range must not do that.

Production depth is `NonZeroU8::MIN` (1), from `ManagerConfig::default` and `Config::default`. `BLOCKFETCH_PIPELINE_N = 2` is tests only.

The stash lives in the pipeliner, not only on the lock-step instance's `pending_fetch`. One latest range is kept. A newer range replaces the older one. The stashed range is delivered only after `after_network`, and only when that slot is idle **and** is the send cursor. Delivering it earlier mis-attributes the next body: with N=2, both slots busy, send cursor and receive cursor both sit on slot 0, and a range sent from slot 0 before the receive cursor moves takes slot 1's body. A slot that stays idle because the mux never admitted the range (`NotAdmitted`, `NoBlocks` to the collector) is still the send cursor, so it takes the stash on that same turn. The offered range is not put back if that call is also `NotAdmitted`.

`Close` is sticky until every in-flight slot is idle or finished. Then one `ClientDone` goes out on the send cursor. A range that was only stashed is dropped once `Close` arrives. A range that arrives after `Close`, and a range that arrives after `ClientDone` has been written, are not sent. The collector gets nothing for those ranges: `NoBlocks` is only for an attempt the handler ran. A `closed` latch is set only after that write returns, so the next slot cannot start a range. `NotAdmitted` or `TimedOut` on `ClientDone` terminates the handler and does not set the latch.

An in-flight range whose success token was fed is in remote agency. Nothing in this design aborts it on the wire. `ClientDone` is only legal from `Idle`. That slot stays busy until the peer answers or the agency timer fires, and during that wait the handler is reading its mailbox. Other peers are unaffected because nobody awaits this handler.

The lock-step instance (`N = 1`) is the production path. After `ClientDone` that instance is `Done`, and a later range faults the handler the way it already did. The pipeliner latch is the N>1 path.

New pipeliner fields (`stashed`, the sticky close, `closed`) are ordinary state. A trace that omits them does not decode. `#[serde(default)]` is not how this record stays compatible with older snapshots: a trace is read by the version that wrote it.

`N = 1` and `N = 2` mailboxes stay 10. `N = 4` is 12. See the table.

### Liveness timers are armed first

A stage that uses a timeout as its liveness mechanism arms it before any send that might wait. The fetch stage schedules the 5s timeout, and records `fetch_started_at`, before it sends `FetchBlocks` to the manager. The timeout stays at 5s and still calls `FetchNextFrom`. The next widen delay is armed from `fetch_started_at` while the batch is open, including when that wakeup itself asks nobody.

### Mailbox capacity

The default bulk mailbox stays 10 (`DEFAULT_MAILBOX_SIZE`). Capacity is per stage, a plain `usize`, chosen when the stage is built, on both interpreters. `StageGraph::stage_with_mailbox_size` and `Effects::stage_with_mailbox_size` take that `usize`. There is no `MailboxSize` type, and `StageBuildRef` has no `with_mailbox_size`. The builder's `with_mailbox_size` remains the default for every stage that does not opt in, and for children those stages later create with `stage`. Raising the global default is not the fix.

The mailbox has to hold the messages **this node** may have in flight while a handler awaits one bounded `call`. It does not have to hold a peer's pipeline. Peer bursts sit in the mux byte buffer and are delivered by the deferred retry above.

| Stage | Capacity | Why |
| --- | --- | --- |
| Shared stages, connection, reader, writer, responders that do not pipeline | 10 | Unchanged default. `stage`, not an explicit size. |
| Block-fetch handler | `max(10, 2 * N + 4)` | `blockfetch_handler_mailbox`. One local request and one network message per pipeline slot, plus `Registered`, `Close`, and the stashed newer range. `N = 1` and `N = 2` stay within 10. `N = 4` is 12. |
| Chain-sync initiator | `PIPELINE_DEPTH + 4` (14) | `CHAINSYNC_INITIATOR_MAILBOX`. Up to `PIPELINE_DEPTH` (10) local `RequestNext`s may be admitted while one `call` is in progress, plus a few network and control messages. |
| Mux | 24 | `MUX_MAILBOX_SIZE`, set in `do_initialize`. A hot duplex connection runs up to ten handlers (initiator and responder for keep-alive, peer sharing, chain-sync, block fetch, and tx submission). One `Send` and one `WantNext` from each, plus `FromNetwork` and `Written`, is 22; 24 leaves room for a `Register` or `SetSduTimeout` in the same burst. |

24 and 14 are the ceilings, not a starting point for tuning upward. A protocol that wants a deeper window states a larger `N` and gets the matching mailbox from the formula. Anything beyond that is a bug in the caller's coalescing, not a reason to grow the channel.

## Consequences

- A peer that stops reading, or a handler that stops reading, delays only its own socket's writer and its own protocol. The manager finishes every fan-out. Fetch keeps arming timeouts. Chain selection keeps being asked for the next tip, so a newer header is pushed once the current attempt ends. A handler that stays full for 5s closes that connection only, and the peer is not scored as adversarial.
- `PeersAsked` names peers whose block-fetch handler actually admitted the request, which is what the timeout scorer wants. The stamp is the instant this attempt chose that peer. A peer skipped because its connection mailbox was full is not asked and is not scored. That skip does not emit `NoBlocks`. `NoneAccepted` is the manager telling fetch that candidates existed and none admitted. `NoBlocks` is the empty reply from an attempt the handler did run.
- Typestate `call` returns `CallAdmission::Reply(Sent)`, `NotAdmitted(CallNotAdmitted)`, or `TimedOut(CallTimeout)`. `CallNotAdmitted` means the bytes will not be transmitted. `TimedOut` means the deadline passed after admission; the caller faults and does not pretend the bytes were either sent or unsent. Handlers that discard the reply and `finish` (`let (_, s) = ….call(…).await; s.finish()`) stop compiling once they move to this call. The network-spec diagrams do not gain a wire message. `project` ignores `Call` and compares the edge deduced from the success token.
- Chain sync keeps a per-peer owed counter so a `RequestNext` the handler did not admit is retried without restarting the mini-protocol.
- Simulation traces of manager and `track_peers` fan-out change from a suspending `Send` to a `TrySend` whose outcome is on the resume. Tests that match those sends need to expect a skip as a normal result, not as a stalled sender.
- Per-stage mailbox size is a `usize` on `WireStage`. The default for every stage that does not opt in stays 10, including in tests that construct a `SimulationBuilder` and do not call `with_mailbox_size`. `amaru-node` tests do call it, with 10000.
- Shutdown of a mini-protocol that is not reading is bounded by the existing stop timer, not by that handler's mailbox draining.
- The 60s agency timer is unchanged and still applies once a request has actually been accepted by the mux (`Sent`). This design does not add a wire-level cancel for a batch that already completed on another peer. A lane that cannot sustain 500 kbps faults the connection without an adversarial score.

## Discussion points

- Growing every mailbox until the incident's queue fits was rejected. The manager queued one fetch per 5s timeout and then stopped forever; a larger bound would have stopped later, still without asking another peer. The formulas above are only large enough for our own window.
- Making every `send` non-blocking was rejected. Shared stages should still exert backpressure on each other. The hazard is a wait whose other side is a remote peer, not a wait in general.
- Aborting the other four in-flight block fetches when the first body arrives was rejected. Those handlers, once their `call` has returned `Sent`, are in remote agency and the spec only allows `ClientDone` from `Idle`. The failure mode to remove is the unbounded `call` admission, not the legal wait for a peer that has agency.
- Replying `Sent` only after the TCP write completes was rejected. It would put the handler's `call` on the writer, and the writer is allowed to sit until the SDU timer (30s) when the peer does not read. `Sent` is the moment the last byte of this message is copied into the one-segment egress buffer. Waiting until the writer has that byte would hold an honest 500 kbps peer for about one extra max-segment drain after the byte could already have been buffered, and a message larger than the buffer would not be admitted until its tail segment was handed off.
- Rejecting the whole message as soon as the cap is full was rejected. The sender would have to invent its own retry, and the implicit flow control of a bulk sender (it waits inside the mux call) would break. The buffer is filled incrementally. A payload larger than one segment is split. The remainder waits in arrival order.
- Budgeting the other lanes on the shared writer was rejected. The floor is per lane, at least 500 kbps, and interleaving is not part of this message's deadline. Counting only raw payload bytes, with no segment headers, was rejected: `wire` includes this message's own headers. Arming the 1s egress retry for deferred bytes as well as writer `Full` was rejected: the turn that frees room already copies bytes and delivers `Sent`.
- Routing block fetch from the manager straight at the handler, skipping the connection stage, was rejected. The connection is what reschedules across handshake and what decides the initiator exists. The connection answers `PeersAsked` itself so the manager does not have to wait to find out whether the child admitted the message.
- A handler-chosen `|` between "request sent" and "not sent", next to the `Call` rather than behind it, was rejected. The handler could take the wire arm after a failed call, or the failure arm after a successful one, and projection would believe it. `Sent` and `CallNotAdmitted` are single-input receives. The success token is the only evidence the projection accepts that the bytes were submitted.
- Treating every expired `call` deadline as `CallNotAdmitted` was rejected. After the mux has the payload, the deadline no longer proves that the bytes will not be written. That case is `TimedOut`. It faults the connection. It does not take the unsent arm, and it does not take `Sent`.

## Open questions (temporary — remove before acceptance)

**Q4. Chain-sync failure token.** The initiator stays on the legacy driver, and the handler-side report that a request never hit the wire is deferred, so `track_peers` cannot put a `Queued` slot back when the mux later refuses it. Decides: Roland.

**Q5. Exhaustiveness of rules 1 and 2.** Manager→connection `Disconnect`, `SetLocalUse`, and `RequestSharePeers`, and connection→mux `Register`, `SetSduTimeout`, and `install_done_trap`, are still blocking sends; connection→manager `HandshakeComplete` and `LocalUseApplied` are too. Left blocking on the reading that the mux and the manager do not wait on a peer. Decides: Roland.

**Q6. Default pipeline depth.** Production `blockfetch_pipeline_n` is 1 (`NonZeroU8::MIN`); this record previously said 2. The pipeliner stash in the decision above is the N>1 behaviour, and `BLOCKFETCH_PIPELINE_N = 2` is tests only. Decides: Roland.

**Q7. Local-stall ingress deadline.** One 5s `INGRESS_DEADLINE` covers every protocol, in place of a per-protocol agency timeout. Decides: Helen, on whether that matches timeouts that bound the peer's agency.

**Q9. Flush of a stored tip.** `pending_tip` and `pending_share` flush at the next transition and a quiet connection does not wake itself. Decides: Roland.

**Q10. `Done` outside the counter.** `Done` is one `try_send`, not counted, and `Full` or `Gone` is not retried. `Gone` on `RequestNext` already zeroes the owed counter and is not retried. Decides: Roland.

**Q11. `Sent` and §3.2.** `Sent` means the last byte is in the bounded egress buffer, not on the wire and not accepted by the writer. Decides: Helen, on whether that is consistent with network-spec §3.2.

**Q12. Test mailbox.** `amaru-node` tests use mailbox 10000 (`setup.rs`, `configuration.rs`); the world loop throttles on `mailbox_size()`. Production stages that do not opt in stay at 10. The 10000 configuration is unchanged. Decides: Roland.

**Q13. Handshake reschedule.** `Initial` and `Handshake` reschedule `FetchBlocks`, `NewTip`, `RequestSharePeers`, and `SetLocalUse` with `schedule_after` onto the priority mailbox (`PRIORITY_MAILBOX_SIZE` is 10; exceeding it panics), and nothing caps how many such schedules are outstanding. The manager addresses a connection only after the handshake completes. Decides: Roland.

**TimedOut under CPU overload.** A mux mailbox backed up longer than the egress deadline yields `CallAdmission::TimedOut`; the stated design faults the connection and does not score the peer. Whether that overload is an acceptable reason to drop the peer is open. Decides: Roland.
