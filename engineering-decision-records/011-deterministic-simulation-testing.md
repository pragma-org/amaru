---
type: architecture
status: accepted
---

# Deterministic simulation testing

## Context

Testing and debugging distributed systems is difficult. 

One of the main contributing factors to the problem of testing is that there
are so many failure modes, which results in a combinatorial explosion of states
and test cases that need to be written to ensure proper coverage.

Even when one finds a failing test case, the problem of debugging is
complicated by the parallel nature of a network of nodes. The same test cases
might not fail deterministically, because some bugs are only triggered in some
interleaving of threads that only happen rarely.

## Motivation

The state-of-the-art to deal with the testing problem is to generate test
cases which includes fault injection and "fuzz" a whole network of nodes.

While the debugging problem can be tamed by making the system under test and
the fuzzer completely deterministic, that way if we find a failure we can
deterministically reproduce it from some seed.

## Decision

In order to tackle the problem of testing and debugging we've introduced two
changes:

  1. Introduced a library, called pure-stage, for building parallel processing
     networks that can be run in a completely deterministic fashion. The idea
     being that the consensus pipeline will be ported from using gasket (which
     wasn't designed with determinism in mind) to pure-stage;

  2. A discrete-event simulator which spawns a network of nodes and simulates
     the network connections between them. The simulator decides when, or if,
     messages get delivered to the nodes and can step through the execution
     deterministically using the pure-stage API.

## Discussion points

Apart from the goal of determinism, the design of the pure-stage library and
the simulator were mainly driven by the following considerations.

### Pipelined architecture

One complication to both making the node deterministic and simulation testing
it is the fact that the processing of incoming messages happens in a parallel
pipeline. So the trick of implementing the system as a state machine of type:

  (input, state) -> (output, state)

doesn't quite work. One rather needs to implement it as several state machines
(that can run in parallel) connected with queues. Further complications arise
due to the fact that the different stages in the pipeline share state (or one
stage needs to be able to send messages to another to update the other copy of
the state).

### Maximising the overlap of what's being tested vs deployed

We want the simulator to be as true to the "real world" as possible. Ideally
any failure that could happen in a real world deployment should be reproducible
within the simulator (and any failure to do so should be considered a bug in
the simulator), but of course one has to draw a line somewhere.

To achieve this the pure-stage library has a runtime interface that can be
toggled between being deterministic or to use tokio.

On the simulator side it's important to be able to simulate a realistic
networking environment as well as realistic faults. We'll describe these two
aspects below.

### Simulating a realistic network

Messages between nodes can be subject to the following:

* Loss (due to network partition)
* Duplication
* Latency
* Reordering (consequence of latency)

It's possible to simulate all these conditions in the simulator, because it has
access to all the messages that are supposed to be delivered to every node.

### Fault injection

Apart from network faults, which are always present in distributed systems due
to the nature of nodes being connected via the network, there are many other
things that can go wrong. Following the literature on distributed systems,
we'll split these faults up in two categories:

  1. Crash faults, where things just go wrong;
  2. Byzantine faults, where potentially compromised nodes or other
     actors on the network maliciously tries break things.

#### Crash faults

Crash faults have not been implemented yet, but here's a list of them and a
sketch of how to implement them:

* Node crashes and restarts (loss of volatile memory). The simulator spawns
  nodes, so implementing crashes and restarts can be done by dropping and
  respawning a node;

* I/O and GC pauses. Rust doesn't have GC, but disk I/O can sometimes be slower
  than normal due to other stuff happening on the computer that the node is
  running on. I/O pauses can be implemented at the pure-stage level by delaying
  when a future becomes available;

* Time skews (the clock of nodes drifts a part). The simulator controls the
  clock of the nodes, via pure-stage's API, so skews can be implemented by
  advancing some node's clocks more than others;

* Disk failures (since all of our stable storage is delegated to RocksDB we
  inherit its storage failure model). To test this the simulator can inject the
  following disk faults on nodes:
    - Delete the whole database;
    - Flip bits in the database;
    - Truncate the write-ahead-log of the database (to simulate a crash where
      the last entry wasn't fully fsynced to disk). For more details, see
      [Protocol-Aware Recovery for Consensus-Based Distributed
      Storage](https://dl.acm.org/doi/10.1145/3241062).

#### Byzantine faults

The space of possibilities here is infinite. Any combination of changing any
message or any piece of state on a faulty node is allowed. Although the
protocol, with its use of cryptographically secure hashes, should render all of
these attacks useless, nevertheless this needs to be tested to some extent.

As a first approximation one could have the simulator mutate messages between
or the stable storage of a node to ensure that hashes are checked.

Coordinated attacks that explore the extremes of protocol parameters and timings
of messages is out of scope of this decision. Let us just note there's the
possibility to add Byzantine (malicious) nodes to the simulation test and
otherwise run the tests as before, and ensure that the non-faulty nodes still
work as intended.

It should also be noted that this space contains admin mistakes, such as
misconfigurations of node, computer or network settings. We've not thought
about how to simulate these type of faults yet.

### Membership changes and upgrades

Testing how the system behaves as nodes leave and join, potentially with a
different version of the software, is also important as there could be
backward- and forward-compatibility problems between the version.

While we've not put much thought into this yet, the simulator has been designed
in such a way that it can communicate with the nodes via stdin/stdout a la
Jepsen's Maelstrom, which means we could technically run different versions of
the node binaries (this would be slower than the default in-process tests).

### Test case generation

Due to the consensus protocol's requirement that all messages need to be hashed
and refer to the previous message's hash (i.e. hash-chained), it's not possible
to generate "random" traffic as one normally would with property-based testing.

Instead a pre-generated "block tree" is used and the simulation tests merely
choose a path through this tree. Currently the generator always backtracks and
chooses the longest path it can find. Concurrent upstream peers are simulated
by having overlapping requests being made from the same tree.

The pre-generation is currently done using Haskell libraries that are shared
with the Cardano node. In the future this step should be moved to Rust and
happen during test-time, rather than be pre-generated.

### Properties to test for

The main property of the consensus protocol can be found
[here](https://ouroboros-consensus.cardano.intersectmbo.org/docs/for-developers/CardanoPraosBasics/).

Currently we only check that "honest nodes promptly selects the best valid
chain that has propagated to it" without checking any time-bounds.

The history of the test execution that the simulator produces includes the
times of each message sent into and out of the system under test, which would
allow us to make assertions based on time as well. For example using a variant
for LTL logic.

### Replaying

When the simulator finds a test case for which the above property doesn't hold,
or if we run into an error in a production deployment, it's important to be
able to debug the problem.

Special care has been taken in the design of the simulator and pure-stage to
record all steps necessary to be able to deterministically reproduce exactly
what happened leading up to the failure or error.

Once the problem has been debugged and fix produced, one should be able to
replay the recorded steps and ensure that indeed the problem has been fixed.

### Debugger

The recorded steps for replaying could also be visualised, e.g. showing a
sequence diagram of the messages being sent between the nodes and coloured
diffs of how the state of the nodes changes upon receiving messages.

### Antithesis

One might wonder: why bother with all this when we are using Antithesis paid
SaaS to do simulation testing already?

Some differences that may or may not motivate the double effort:

1. The effort to introduce deterministic simulation tests was started before
   Antithesis was approached, and we knew it would be a good tool or a budget
   for it;

2. The Antithesis tests take on the order of hours to run, whereas the simulation
   tests described above take seconds to run. The immediate feedback makes it
   possible to use the tests during development as a sanity check;

3. Antithesis doesn't support for example disk faults currently;

4. Good system architecture and being easy to test locally (i.e. with `cargo
   test`) goes hand in hand. The efforts described above leads to for example
   being able to write a unit test for a regression test involving a whole network
   of nodes and faults, which is something that would not been possible if the
   system had not been made deterministic (and solely relied on Antithesis for
   determinism).
