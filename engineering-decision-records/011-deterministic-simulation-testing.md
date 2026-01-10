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

The state-of-the-art to deal with the testing problem is to generate test
cases which includes fault injection and "fuzz" a whole network of nodes.

While the debugging problem can be tamed by making the system under test and
the fuzzer completely deterministic, that way if we find a failure we can
deterministically reproduce it from some seed.

## Decision

To tackle the problem of testing and debugging we've introduced two changes:

  1. Introduced a library, called pure-stage, for building parallel processing
     networks that can be run in a completely deterministic fashion. The idea
     being that the whole node-internal processing network will be ported from
     using gasket (which wasn't designed with determinism in mind) to
     pure-stage;

  2. A discrete-event simulator which spawns a network of nodes and simulates
     the network connections between them. The simulator decides when, or if,
     messages get delivered to the nodes and can step through the execution
     deterministically using the pure-stage API.

## Consequences

Apart from the goal of determinism, the design of the pure-stage library and
the simulator were mainly driven by the following considerations.

### Pipelined architecture

One complication to both making the node deterministic and simulation testing
it is the fact that the processing of incoming messages happens in a parallel
pipeline. So the trick of implementing the system as a state machine of type:

  (input, state) -> (output, state)

doesn't quite work. One rather needs to implement it as several state machines
(that can run in parallel) connected with queues. Further complications arise
due to the fact that the different stages in the pipeline share external state (e.g.
the contents of a persistent storage for blocks, headers, tips). Here, the design
goal for the concrete stages should be that updates to some entity be made by
only one stage and messages to other stages carry the knowledge about what
has been written and can thus be read. This is aided by the append-only nature
of the content-addressed storage items used within a blockchain node.

### Simulating time

Discrete event simulators are typically implemented using a heap of events
ordered by when the events occur. Before processing an event the clock is first
advanced to the arrival time of the event, if there are any timers (such as
timeouts or sleeps) that have happened meanwhile we need to handle them first
before processing the event, and any new events that result from processing the
event are assigned random arrival times and put back into the heap, the
simulation then continues until either some end time has been reached or the
simulator runs out of events.

This is what creates the effect of being able to "speed up time". For example
if one event is "there's a partition between node A and B", followed by events
trying to deliver messages between A and B, then those messages will timeout
immediately (unlike if we do Jepsen-style system tests where we have to wait
for the timeouts to happen in real-time).

Since the pure-stage library allows for modelling several connected stages that
might share memory, one cannot simply assume that the processing of one message
is a discrete event that produces output messages instantly. So the simulator
needs to not only orchestrate the network, but also the thread scheduler so to
say.

Here's a sketch of the algorithm for the simulator:

  1. The simulation heap starts out with external input messages, each paired
     with an arrival time and a target node;
  2. Each node is polled for the next time at which something would occur
     internally; the node is also inserted into the simulation heap with that
     time;
  3. The simulator pops the task with the smallest time from the heap (message
     delivery or internal node action) and performs it; this may enqueue
     messages for other nodes that cause them to be woken up sooner than dictated
     by their current position in the heap — if so, remove and re-insert;
  4. That node is then queried for the next interesting time and inserted back
     into the heap with that time;
  5. The simulator starts over from 3 (unless some stop condition is
     satisfied).

So the API should be something like:

```
  handle : ArrivalTime -> Message -> NextArrivalTime -> Vec<Message>
  nextInteresting : Time
  advanceTime : Time -> Vec<Message>
```

Where `NextArrivalTime` is when the next message (after the one we just popped)
arrives, the idea being that the node we are currently stepping is free to run
all its yield points until that time (since nothing interesting is happening in
the world until then anyway).

### Maximising the overlap of what's being tested vs deployed

We want the simulator to be as true to the "real world" as possible. Ideally
any failure that could happen in a real world deployment should be reproducible
within the simulator (and any failure to do so should be considered a bug in
the simulator), but of course one has to draw a line somewhere.

To achieve this the pure-stage library has a runtime interface that can be
toggled between being deterministic or to use tokio. This is described in more
detail under the section ["relation to production
mode"](https://github.com/pragma-org/amaru/blob/stevan/main/engineering-decision-records/011-deterministic-simulation-testing.md#relation-to-production-mode).

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

* Time skews (the clock of nodes drifts apart). The simulator controls the
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

As a first approximation one could introduce data corruption by having the
simulator mutate messages being sent between nodes or mutate data in
the stable storage of a node to ensure that hashes are checked.

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
and refer to the previous message's hash (i.e. hash-chained), it's difficult
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

## Discussion points

### Relation to production Mode

The above description focuses on a special mode of execution for the pure-stage
infrastructure that allows us to deterministically run a whole network while
precisely following what each node is doing. This mode is single-threaded by
definition due to the ordering guarantees it needs to make. When running an
Amaru node in production, the very same pure-stage components are used with a
non-deterministic execution engine that allows for parallelism, namely the
Tokio runtime. The idea here is that simulation testing should eventually
exhaust all possible interleavings of messages and other effects, thus covering
the range of possible behaviours of the Tokio implementation.

We still want to be able to diagnose, reproduce, and correct any runtime
failures observed during production mode. This is why all relevant effects
(which include message reception but also the results of external effects like
reading the clock or accessing disk storage) are recorded in a trace that can
then be taken to an offline system and be replayed. This requires that the
offline system has been primed with a suitable starting state (disk storage as
well as stage state) because the trace will usually be truncated (e.g. by
recording to an in-memory ring buffer that overwrites old entries). Care has
been taken to ensure that all relevant state can be snapshotted and stored
periodically, making it possible to recreate the exact runtime state of an
Amaru node from the youngest snapshot and the trace written since.

Due to the true parallelism available in production mode it is impossible to
recreate the exact temporal evolution of a node’s state when replaying a trace.
When focusing on a single stage the result is exact, though, because a stage
only processes inputs sequentially due to its state machine signature. We can
thus envision a time travelling debugger for a single stage, with a bit of fuzz
regarding the execution order between different stages.
