---
type: architecture
status: accepted
---

# Switching to our own mini-protocol implementations

## Motivation

Up until late 2025, Amaru used [`pallas_network`][pallas-network] for the implementation of node-to-node protocols.
That crate is being developed by txpipe, with the main driver being the [Dolos data node][dolos].
Consequently, implementations of the responder role were not a priority for txpipe, leading to different design choices than would have been desired by Amaru (cf. the [`pallas_network2` PR][pallas-network2]).

In addition, Amaru is expending great effort to ensure protocol conformance through rigorous simulation testing, going so far as to create the [`pure_stage`][pure-stage] library for pure, explicitly effectful, and fully deterministic [Actors][actor-model].
The Ouroboros mini-protocols defined in the [network specification][network-spec] can be modelled as pure stages, including static verification of conformance by analysing the state machine structure.

## Decision

We implement the network mini-protocols on top of pure-stage, creating a thin layer for splitting each protocol handler into a strictly specified network state machine and a local decision-making part.
Conformance of the network part is statically ensured by the underlying state machine structure, while proper interaction of the decision-making part with the network is regulated by runtime checks.

The overall structure is that each peer connection runs its own muxer, also as a pure stage, and has its own set of mini-protocol handlers.
Initiator and responder roles are implemented by separate handlers that can be attached to or detached from a connection as desired by the peer selection mechanism that governs the use of network connections at a higher level.

## Consequences

In effect, we are ascertaining protocol conformance by installing a verified behavioural monitor between the local node decision-making processes and the network stack.
This will allow us to declaratively ensure not only conformance of data types and of the sequence of actions, but also generically handle timeouts and data buffering limits as well as protocol pipelining (see [this discussion][protocol-pipelining]) up to a specified maximum depth.

The split of handling muxing and each mini-protocol role in separate actors allows the efficient distribution of these tasks across multiple execution threads to make good use of provided computational resources; we trust that Tokio is mature enough to do a decent job at allocating tasks to CPUs.

## Discussion points

\-

[actor-model]: https://en.wikipedia.org/wiki/Actor_model
[dolos]: https://github.com/txpipe/dolos
[network-spec]: https://ouroboros-network.cardano.intersectmbo.org/pdfs/network-spec/network-spec.pdf
[pallas-network]: https://github.com/txpipe/pallas/tree/a302ddd30dff55ecee08d78b569dcba25bb1c391/pallas-network
[pallas-network2]: https://github.com/pragma-org/amaru/pull/420#issuecomment-3257941889
[protocol-pipelining]: https://github.com/cardano-foundation/CIPs/pull/1167#issuecomment-4217223369
[pure-stage]: https://github.com/pragma-org/amaru/tree/main/crates/pure-stage
