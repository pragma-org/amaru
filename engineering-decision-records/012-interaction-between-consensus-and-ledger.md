---
type: architecture
status: accepted
---

# Interaction betweeen Consensus and Ledger

The work progressing on deterministic simulation testing has raised questions about where to draw the line: which parts of the codebase shall be executed using `pure_stage` and which parts are outside of its scope?
In a meeting on Aug 8, 2025 between @KtorZ, @stevana, @rkuhn the following response was developed.

## Context

The main complexity of the consensus part of Amaru stems from the concurrent and distributed nature of peer to peer information flow while respecting resource bounds and rejecting adversarial behaviour.
This complexity is tackled by dividing responsibilities among processing stages within the Amaru node that are connected to form a data processing network.
Testing such a network as well as debugging it requires dedicated tooling and a suitable approach as described in [EDR011](./011-deterministic-simulation-testing.md).

On the other hand, the main complexity of the ledger lies in its state management so that it can efficiently support the various required calculations for performing the business function of the Cardano blockchain.
This is implemented using a combination of in-memory data structures and sophisticated database usage, none of which play nicely with the notion of encapsulating each and every interaction with persistent mutable state as a [`pure_stage::ExternalEffect`](https://docs.rs/pure-stage/latest/pure_stage/trait.ExternalEffect.html).

The interaction between consensus and the ledger fundamentally has only two parts, with a third one added merely incidentally (which should be fixed):

- whenever the chain selection finds a new longest chain candidate, it must pass it to the ledger for validation, which may reject the result for a variety of reasons (some indicating malice of a peer, some not)
- at each epoch boundary, the ledger emits a new set of per-stakepool information (including stake, addresses, VRFs) that will need to be used by the consensus for header validation; this may include pools retiring or protocol version updates that may even change consensus rules
- incidentally, the ledger also manages the evolutions of operational certificates of the pool — these should actually be managed by the consensus

## Decision

We represent the interactions listed above using `pure_stage` facilities as follows:

- longest chain validation is an `ExternalEffect` emitted by consensus that receives a detailed validation result as its asynchronous response; handling that effect will send the new chain to a different thread using a channel and receive the response using a oneshot channel
- epoch updates from the ledger are sent to interested consensus stages via the [`StageGraph::input`](https://docs.rs/pure-stage/latest/pure_stage/trait.StageGraph.html#tymethod.input) facility

## Consequences

The consensus stage `SelectChain` will wait for the ledger validation result before proceding to process further inputs.
This implies that a new and better longest chain received from upstreams while ledger validation is ongoing will not interrupt that validation, it will be handled after the validation is complete.
When switching between chains, this may incur significant latency in case of deep rollback.

Ledger updates from epoch boundary processing will arrive asynchronously within the consensus part.
This implies that e.g. updated stake distributions need to be actively awaited by the `ValidateHeader` stage when it detects an epoch change.
It also implies that updated protocol parameters need to be actively awaited by any concerned consensus stages before being able to participate in the epoch after the following epoch change.
These concerns can be handled by handling the ledger updates in a dedicated stage that can then be interrogated from other stages that need the information, asserting back pressure on the incoming data flow until the required information is available.
