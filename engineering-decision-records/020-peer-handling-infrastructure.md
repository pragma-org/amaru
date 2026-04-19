---
type: architecture
status: accepted
---

# Peer Selection Infrastructure

This document describes the infrastructure used in peer handling, which includes peer selection, connection management, failure responses, and block fetching.
It does not cover the algorithms employed to actually select peers, which will evolve over time.
The goal is to create an architecture within which the desirable algorithms can be realised.

## Context

The most important context is the Ouroboros Network Specification, which defines the protocol behaviour between peers using the node-to-node (N2N) protocol.
This specification is very precise regarding when which message may be sent but it doesn’t prescribe a concrete algorithm for deciding which of the possibilities to choose.
It does, however, describe in some detail the Haskell implementation specifics of how peer connections are managed, namely using an outbound governor for opening connections and using those, an inbound governor for using connections accepted from peers, and a connection manager implementing the low-level data transport.
Most of these aspects are internal to a node while some others are observable to peers (the spec was written with only the Haskell implementation in mind, meaning that peers would run either the same software or a slightly different version).

The most important externally observable connection management behaviours are the following:

- a connection can be cold, warm, or hot regarding how the local node will use it:
  - **cold** means no mini-protocols will be run in initiator mode (all other states will always run `keepalive`, `peersharing`)
  - **warm** means that `blockfetch`, `txsubmission` will be run in initiator mode
  - **hot** means that in addition to the warm protocols `chainsync` will be run in initiator mode
- demoting hot → warm → cold will gracefully shut down the mini-protocols by transitioning into StDone, expecting that a later promotion is possible by sending any message that is allowed from the initial state of the respective mini-protocol
- a unidirectional connection (i.e. with `initiatorOnlyMode == true` in the handshake) will be terminated when demoted to cold
- a bidirectional connection will be terminated when the local intent is cold and the remote activity is judged to indicate cold state as well (no mini-protocols active for some time)
- a connection will be terminated upon seeing any non-conformant behaviour from the peer; repeat offenders will not be reconnected to for some time (violation count is reset when connection remains established for some time)
- active churn gracefully demotes the least-performing nodes in order to keep the network adapting to topology changes
- adversarial behaviour transitions peer into **forgotten** state which prevents reconnection for some time
- selection of peer(s) to fetch blocks from is based on EWMA of latency (from `keepalive`) and bandwidth (from `blockfetch` by subtracting latency)

## Decision

### Connection Management

As the network specification does not prescribe binding to the local server port for outgoing connections, we shall not assume that peers do so; this is prudent also in the face of NAT and the general brokenness of today’s Internet.
From this follows that incoming connections’ remote port information shall not be used to deduce peer server port addresses.

In the interest of allowing Amaru node admins to distinguish inbound and outbound connections by local port number, Amaru shall not bind to the server port for outbound connections.

The `amaru-protocols` Manager corresponds to the Haskell connection manager in that it keeps track of TCP connections including connecting outbound and accepting inbound (via sub-stages).
It shall detect remote peer usage of connections (including new connections and disconnects) and forward the observed remote connection temperature as well as the possible duplex modes to the peer selection stage.
It shall also start or gracefully stop mini-protocols according to the desired connection temperature (cold / warm / hot) communicated by the peer selection stage as far as the handshake result allows.

## Peer Selection

This stage is the brain of Amaru when it comes to understanding the network vicinity.
Since the network topology is live and evolves in real-time, the node does not persist the peer tracking information across restarts.
It tracks several **disjoint sets of peers**:

- **forgotten peers** are markers for peers that shall not be connected to; these markers expire after some time
- **candidate peers** are those that the node considers reasonably likely addresses of Cardano peers, but they are unverified
- **connected peers** are those with which a TCP connection currently shall exist (i.e. it might be connecting, connected, or remotely disconnecting)

The connected peers are associated with more information, namely the kind of connection (inbound / outbound, full-duplex capability), the current local and remote usage modes, and the failure count.
A candidate peer only remembers the failure count and a cool-down period (stored as the Instant after which it may connect again).

The system is primed with a set of **static peers**.
These are configured by the node admin and used for initial connections, i.e. they populate the initial candidate list.
Static peers will also never be forgotten and cool-down periods are much shorter than for other nodes.

Nodes are discovered via any viable means, including stake pool registrations in the ledger, peer sharing from other nodes, well-known node lists, etc.

## Peer Performance Tracking

A PeerPerformanceResource is used to track information across all pure-stage stages.

- the `keepalive` protocol handler injects latency measurements
- the `blockfetch` protocol handler injects block delivery timing measurements, which together with latency allows computing bandwidth to a per
- the `chainsync` protocol handler injects when each header was received and from which peer
- the block fetching logic computes candidates for requesting blocks from based on who has sent the header
- the peer selection logic uses performance measurements to demote/promote peers

This information is not persisted across node restarts due to the ephemeral nature of the observed phenomena.

## Block Source Tracking

A BlockSource stage tracks which peers sent a given block in order to classify them as adversarial in case the block fails validation.
This information is not persisted across node restarts because the window of opportunity between receiving an adversarial block and validating it is small: blocks are only fetched for the current best chain.

## Consequences

- There can be two connections with the same peer, and if that peer binds to the server port for outbound connections then both connections have the same remote socket address.
  This means that we cannot use the `Peer` data type for identifying connections.
- A peer that has closed all its mini-protocols and disconnects must not be treated as failed or even adversarial.
- After a peer transitioned a mini-protocol into StDone, that mini-protocol must be reset to its initial state.
- The node will discover the current network topology and peer performance anew after each restart.
