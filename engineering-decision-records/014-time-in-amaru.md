---
type: architecture
status: accepted
participants: abailly, etorreborre, rkuhn, stevena
---

# Time in Amaru

## Context

Time in distributed systems is a notoriously challenging topic, even more so in adversarial settings presented by blockchain infrastructure.
In addition to the general literature on the topic, there is a proposal for using Ouroboros to manage time as well, called [Ouroboros Chronos](https://iohk.io/en/blog/posts/2021/10/27/ouroboros-chronos-provides-the-first-high-resilience-cryptographic-time-source-based-on-blockchain/);
for the purposes of this EDR this research is not (yet) pertinent, though.
A more immediately applicable context is given by the technical report on [The Cardano Consensus and Storage Layer](https://ouroboros-consensus.cardano.intersectmbo.org/assets/files/report-3517eb074dfc031c6b753c58ceb63169.pdf), in particular chapter 17 and section 11.5.

Key insights from this report are:

- due to the flexibility of the Cardano network, converting between slot numbers and wall clock time is challenging
- in particular, since changes to the slot length are permitted at era boundaries, handling those boundaries becomes tricky
- the wall clock time source for Cardano is UTC as ensured by [NTP](https://datatracker.ietf.org/doc/html/rfc5905) infrastructure external to Cardano
- network messages contain slot numbers that need to be converted to wall clock time to validate the message (basically: headers from the far future are treated as possibly adversarial behaviour)

Further noteworthy points are:

- UTC includes leap seconds; NTP implementations may step or smear time. The node must define how leap seconds affect slot mapping (e.g., ignore leaps or adopt smear) to avoid slot timing ambiguity.
- Define the permissible clock skew (e.g., ±Δ seconds) when validating “future” headers so operators understand the tolerance and monitoring can alert when skew exceeds Δ.
- Scheduling should prefer a monotonic clock for timers to avoid backward jumps when NTP steps the wall clock; wall-clock is used for conversion/validation only.

## Decision

The current state as well as the history of Cardano do not require us to implement the ability to change the slot length.
We therefore use a hard-coded slot length of 1 second (while keeping the genesis offset configurable).
We also follow the lead of the Haskell implementation in assuming that NTP is correctly maintaining the local wall clock.

Concretely:

- Slot-to-time mapping: utcTime(slot) = genesisUtc + slot * 1s. All values are in UTC.
- The genesis offset is configured as Unix time (seconds since the Unix epoch) in configuration, and MUST be unambiguous (UTC).
- Timers and scheduling SHOULD use a monotonic clock to mitigate NTP steps; wall-clock is consulted only for conversion and validation.

## Consequences

1. We will need to clearly document the requirement that for the node to function properly on any network, the administrator needs to ensure proper time synchronisation using NTP.
   Failure to do so may lead to losing network connectivity or inability to mint blocks even when winning the slot lottery.

   Operational guidance:

   - Recommended NTP client: chrony (preferred) or ntpd, configured with multiple, diverse, authenticated sources where possible.
   - Document acceptable maximum clock skew (e.g., ±500ms or as decided) and expose metrics/health checks to alert when exceeded.
   - State the expected leap-second behaviour (step vs smear) and how the node handles it (e.g., smear-compatible).

2. If governance proposes changing the slot length from 1 second, we must implement support for that era change.
   Whether we implement the full generality (as per chapter 17 of the linked report) or a narrower approach will be decided then.
   Follow-up: draft an EDR outlining era-schedule representation, header validation across era boundaries, and rollout/activation mechanics.