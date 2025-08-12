---
type: architecture
status: accepted
participants: abailly, etorreborre, rkuhn, stevena
---

# Time in Amaru

## Context

Time in distributed systems is a notoriously challenging topic, even more so in adversarial settings presented by blockchain infrastructure.
In addition to the general literature on the topic, there is a proposal for using Ouroborous to manage time as well, called [Ouroborous Chronos](https://iohk.io/en/blog/posts/2021/10/27/ouroboros-chronos-provides-the-first-high-resilience-cryptographic-time-source-based-on-blockchain/);
for the purposes of this EDR this research is not (yet) pertinent, though.
A more immediately applicable context is given by the technical report on [The Cardano Consensus and Storage Layer](https://ouroboros-consensus.cardano.intersectmbo.org/assets/files/report-3517eb074dfc031c6b753c58ceb63169.pdf), in particular chapter 17 and section 11.5.

Key insights from this report are:

- due to the flexibility of the Cardano network, converting between slot numbers and wall clock time is challenging
- in particular, since changes to the slot length are permitted at era boundaries, handling those boundaries becomes tricky
- the wall clock time source for Cardano is UTC as ensured by [NTP](https://datatracker.ietf.org/doc/html/rfc5905) infrastructure external to Cardano
- network messages contain slot numbers that need to be converted to wall clock time to validate the message (basically: headers from the far future are treated as possibly adversarial behaviour)

## Decision

The current state as well as the history of Cardano do not require us to implement the ability to change the slot length.
We therefore use a hard-coded slot length of 1 second (while keeping the genesis offset configurable).
We also follow the lead of the Haskell implementation in assuming that NTP is correctly maintaining the local wall clock.

## Consequences

1. We will need to clearly document the requirement that for the node to function properly on any network, the administrator needs to ensure proper time synchronisation using NTP.
   Failure to do so may lead to losing network connectivity or inability to mint blocks even when winning the slot lottery.

2. As soon as a governance action is proposed to change the slot length to some other value than 1 second, we will need to implement support for this era change.
   Whether we do this in full generality (like chapter 17 of the report linked above) or using a more specific approach will need to be decided at that time.
