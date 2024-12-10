# Amaru Consensus

This component aims at implementing Ouroboros Consensus, in Rust, within the Amaru node. It is also an integration point for more generic code defined in [ouroboros](https://github.com/pragma-org/ouroboros) repository.

## Testing strategy

Ouroboros consensus is a somewhat complex and deceptive beast: While formulating its behaviour is relatively simple and straightforward, there are many corner cases and potential attack vectors one needs to guard against, therefore testing strategy needs to be an integral part of the design and development of a solution.

Furthermore, one of the goals of the Amaru projects is to document and generalise the behaviour of Cardano core components in order to ease comparisons, analysis, investigations, alternative implementations, benchmarks, tests,... without requiring retro-engineering an existing implementation.

The following picture summarizes core ideas of the current testing process for the consensus:

![Testing consensus flow](./testing-strategy.jpg)

* The overall approach is _Model based_, using _State Machine_ models of the expected behaviour of interacting parts of the system to generate test traces and validate _SUT_'s behaviour
* A complete chain is generated, possibly with varying characteristics: length of tines, frequency of forks, distance between blocks, etc. This simulates how a node would see the chain progressing,
* The _Leader_ and _Observer_ are test-only components that are controlled by the _Test Driver_ through a "script" which basically defines their behaviour as a state machines (see below)
* The _Leader_ and the _SUT_ start at some random point in the generated chain,
* The _Observer_ plays the role of _downstream_ peers and collect `ChainSync` messages issued by the _SUT_
* The _Leader_, _SUT_ and _Observer_ are connected through direct channels, abstracting the details of low-level TCP connections, multiplexing, etc.
* The _Test Driver_ ensures the _SUT_ behaves according to the expectations, and can also drives the _Leader_ and _Observer_ to increase coverage, using _logs_ from the _SUT_

### Chain Sync specification

We would like to have a formal description of the protocol(s) that we can use to drive _Test doubles_. Here is a draft specification loosely based on [spex](https://spex-lang.org) language. Spex is currently very basic and does not allow for guarded transitions like the one we define below, so for the time being we could resort to "fake it" and have this state machine only in code.

```
-- Chain-sync node-to-node protocol

data Chain blk =
     Genesis
  |  Chain blk (Chain blk)

node Leader (chain : Chain a, tip : a) where

  StIdle & Follower?MsgDone & StDone

  StIdle & Follower?MsgFindIntersect(points)
     , [ points ∩ chain = {p,...} ] Follower!MsgIntersectFound(p, tip) & StIdle
     | [ points ∩ chain = ∅ ] Follower!MsgIntersectNotFound & StIdle

  StIdle & Follower?MsgRequestNext , Follower!MsgRollForward & StIdle
                                   | Follower!MsgRollBackward & StIdle
                                   | Follower!MsgAwaitReply & StMustReply

  StMustReply & Follower!MsgRollForward & StIdle
              | Follower!MsgRollBackward & StIdle

node Follower where

  StIdle & Leader!MsgDone & StDone

  StIdle & Leader!MsgFindIntersect , Leader?MsgIntersectFound & StIdle
                                   | Leader?MsgIntersectNotFound & StIdle

  StIdle & Leader!MsgRequestNext , Leader?MsgRollForward & StIdle
                                 | Leader?MsgRollBackward & StIdle
                                 | Leader?MsgAwaitReply & StMustReply

  StMustReply & Leader?MsgRollForward & StIdle
              | Leader?MsgRollBackward & StIdle
```

## References

* [Cardano Consensus and Storage Layer](https://ouroboros-consensus.cardano.intersectmbo.org/assets/files/report-b72e7d765cfee85b26dc035c52c6de84.pdf)
* [Ouroboros Network Specification](https://ouroboros-network.cardano.intersectmbo.org/pdfs/network-spec/network-spec.pdf)
