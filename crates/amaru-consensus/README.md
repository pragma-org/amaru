# Consensus pipeline

The following graph represents a simplified sequence of _stages_ headers (either forwarded or rolled-back) undergo in
the consensus module. Each square node is a processing step, and arrows are labelled with the type of messages flowing
between the various steps.

```mermaid
graph TD
    disc[disconnect peer]
    net@{ shape: delay, label: "Upstream peers" }
    upstream([upstream]) -.-> net
    upstream -- chain sync --> pull
    pull -- ChainSyncEvent --> rcv[receive header]
    rcv -- malformed header --> disc
    val_hdr[validate header]
    rcv -- DecodedChainSyncEvent --> sto_hdr
    sto_hdr -.-> store@{ shape: cyl, label: "Chain store" }
    sto_hdr -- storage failure --> crash?
    sto_hdr -- DecodedChainSyncEvent --> val_hdr[validate header]
    val_hdr -- invalid header --> disc
    val_hdr -- DecodedChainSyncEvent --> select[select chain]
    select -- invalid chain --> disc
    select -- ValidateHeaderEvent --> fetch[fetch block]
    fetch -.-> net
    fetch -- fetch error --> disc
    fetch -- ValidateBlockEvent --> sto_block[store block]
    sto_block -.-> store
    sto_block -- storage failure --> crash?
    sto_block -- ValidateBlockEvent --> val_block[validate block]
    val_block -.-> store
    val_block -- invalid block --> disc
    val_block -- BlockValidationResult --> fwd[forward chain]
    fwd --> down([downstream])
    down -.-> net
```

Stages:

* [pull](../amaru/src/stages/pull.rs): connects to upstream peers, running chain sync and block fetch protocols.
* [receive header](src/consensus/receive_header.rs): this stage is responsible for basic sanity check of _chain sync_
  messages, deserialising raw headers, and potentially checking whether or not they should be further processed (eg. if
  a header is already known to be invalid, or known to be valid because it's part of our best chain, let's not waste
  time processing it!)
* [validate header](src/consensus/validate_header.rs): protocol validation of the header, checks the correctness of the
  VRF leader election w.r.t relevant stake distribution, and epoch nonce
* [store header](src/consensus/store_header.rs): store valid (and invalid?) headers indexed by hash
* [select chain](src/consensus/select_chain.rs): proceed to chain (candidate) selection, possibly changing the current
  best chain,
* [fetch block](../amaru/src/stages/consensus/fetch_block.rs): fetch block body corresponding to the new header, if any.
* [store block](src/consensus/store_block.rs): store valid (and invalid) block bodies, indexed by header hash. The
  blocks are stored _before_ validation in order to support [
  _pipelining_](https://iohk.io/en/blog/posts/2022/02/01/introducing-pipelining-cardanos-consensus-layer-scaling-solution/)
* [validate block](../amaru/src/stages/ledger.rs): validate the block body against its parent ledger state
* [forward chain](../amaru/src/stages/consensus/forward_chain/): forward newly selected chain to downstream peers (chain
  followers)

## Chain Selection

The main job of the _consensus_ component is to participate in the _Ouroboros consensus_ process, contributing to the
validation (and as a block producer to the extension) of _the_ chain with _peers_ in an strongly connected network.
Being a _decentralized_ and _distributed_ system, there's no global information a peer can rely on, and therefore each
has to decide on their own which chain to follow based on information provided by other peers. This is the _Chain
Selection_ process:

* A node connects to a set of _upstream peers_ and starts running the `ChainSync` protocol client, pulling new headers
  starting from some well known intersection point and _tracking_ upstream peers' chain
* It validates the headers and corresponding bodies, discarding invalid ones and possibly disconnecting from incorrect
  peers,
* Given a set of valid _candidate chains_ the node tracks from its upstream peers, it selects its current _best chain_,
* As the best chain evolves, the node forwards changes to connected _downstream peers_ thus acting as an _upstream peer_
  for them.

We specify here how this chain selection process is done, leaving aside network-related details, ie. how information
flows from and to upstream and downstream peers, using some informal notation.

`chain_selection` can be thought of as a function taking some `Upstream` message, current `State` and returning an
updated
`State` and a `Result`

```.haskell
chain_selection: Upstream -> State -> (State, Result)
```

`Upstream` messages are simple:

```.idris
data Upstream where
```

Either we receive a `RollForward` from some (upstream) `Peer`, telling us this peer has extended their current chain
with the given `Header`.
This `Header` has already been validated by some earlier stage so it's internally consistent (eg. signature and VRF
proof have been verified).

```.idris
    RollForward Peer Header
```

... or we receive a `Rollback`, telling us this peer has found a better fork and is now following it.

```.idris
  | Rollback Peer Point
```

The `State` we need to maintain and update is more complex:

```.idris
data State =  State {
```

It's conceptually split in two parts, an _immutable_ part which is stored on disk and is a single `Chain`, eg. a "list"
of blocks where each block is valid and linked to its parent.

```haskell
    immutable : Chain,
```

and a "volatile" or _mutable_ part which we call [`HeadersTree`](src/consensus/headers_tree.rs). It contains only
headers as the blocks are always stored on disk and referenced by their header hash

```.idris
    headers_tree: HeadersTree
 }
```

This `HeadersTree` structure is:

* a `tree` of _headers_ where the root is equal to the _tip_ of the `immutable` chain, with the property that a `Header`
  as a unique `NodeId` associated to it (e.g no two headers with the same `Hash` can ever be 2 nodes in the tree),
* a map from `Peer`s to `NodeId` which tracks where (upstream) peers told us they are in the chain(s),
* a singled out `best_chain` which points at _the_ node which is our current best chain, along with the `best_peer`,
* a `max_length` parameter which is fixed at creation and is a bound on the maximum length of a branch of the `tree`.

```.idris
data HeadersTree = HeadersTree {
    tree :: Tree,
    peers: Map Peer NodeId,
    best_chain: NodeId,
    best_peer: Peer
 }
```

Here is a graphical representation of a `HeadersTree` with a couple of peers and a maximum length of 6:

![A live `HeadersTree` with some state](basic-headers-tree.jpg)

> [!IMPORTANT]
>
> Invariants for `HeadersTree` structure:
> * `best_chain`'s length is:
    >

* either exactly equal to `max_length`

> * or shorter than `max_length` and the root of the `tree` is `Genesis`
>

Finally, the `Result` of the chain selection algorithm defines what needs to be communicated to downstream `Peer`s:

```haskell
data Result =
```

* The simplest case is that our best chain did not change:

  ```haskell
  NoChange
  ```
* The chain could have been extended with a new header:

  ```haskell
  NewTip(Peer, Header)
  ```
* The chain has been rolled back to some point:
  ```haskell
  RollbackTo(Peer, Point)
  ```
* The chain has switched to a fork:
  ```haskell
  SwitchedToFork {
     peer: Peer,
     rollback_point : Point,
     fork: [Header]
  }
  ```

There are also a number of error conditions which are not formally specified bu explained in
the [Handling errors](#handling-errors) section.

### Rolling forward

When a `RollForward peer header` message arrives, the peer tells us they are extending their chain by _one_ header. We
first check the given `header`'s parent: if it's not pointing at the header corresponding to what the `peers` map point
to, this is an error. If the header is well-formed, we need to identify several cases:

1. The message comes from our current `best_peer` and therefore extends our current `best_chain`:
    * We insert the `header` into the `tree`,
    * We update `best_chain` to point to the corresponding node,
    * We update `peers` map to ensur `best_peer` points to the new node,
    * We must ensure the length of our `best_chain` is always lower than equal to `max_length`, so in "steady state" we
      must to _prune_ the `tree`. The following diagram illustrates what can happen when pruning the tree.

   ![Pruning the tree when extending the chain](pruning-headers-tree.jpg)

   In particular as shown here, it's possible for a `Peer` to point to a chain which is smaller than our `best_chain`
   and anchored at the parent of our current _tip_ (the root of the tree). In this case, there's no way this peer's
   chain can ever become our best chain and we can as well drop this `Peer` altogether as it's obviously on a "dead"
   fork.
    * the result of the selection is a `NewTip(peer, header)`,
2. The message comes from another `Peer`, which has several interesting subcases:
    1. The given `header` extends the `peer`'s chain but it's still shorter or of equal length than our `best_chain`: We
       simply add the new `header` to the tree, update the `peers` map but do not change our `best_chain`. The result is
       `NoChange`,
    2. The given `header` extends the `peer`'s chain in such a way it becomes (strictly) _longer_ than our current
       `best_chain`. Here, we can again split in 2 cases:

        1. The new `header` extends `best_chain`. This is simple as it just entails, on top of updating the other
           fields, changing the `best_peer`. The result is `NewTip(header)`,
        2. The new `header` is on a different chain than `best_chain`. This is a _fork_ and
           it [requires special care](#handling-forks).

### Rolling Backward

When a `Rollback peer point` message arrives, the peer notifies us they are switching to a fork. A rollback is most
often followed by roll-forwards providing the headers for the fork. Note a `Rollback` only contains a `Point` and not a
`Header`: We necessarily have the corresponding header as it was on the chain from this peer we previously followed.

The first step is to check whether or not the given `point` exists in our `tree`:

* If it does not then it's an error because:
    * either the pointed-at header is in the `immutable` part of the chain, and this is illegal,
    * or it's completely made up,
* If it does, then we need to update the content of the `HeadersTree`:
    * the `peers` map should point to the node corresponding to the `point`ed-at header,
    * the tree should be pruned of nodes between the new peer's node id and the previous one. In so doing we should take
      care of _not pruning_ nodes which are still pointed at by other `peers`
    * In addition, if the `peer` is our current `best_peer`, this necessarily changes our `best_chain` with 2 different
      cases:
        * The `peer`'s chain is still the longest: We update our `best_chain` accordingly and keep our `best_peer` as
          is. The result of `chain_selection` should be `RollbackTo(peer, point)`
        * There's another peer pointing to our current `best_chain`: We update our `best_peer` but keep our `best_chain`
          as is. The result is `NoChange`,
        * finally, there's the case where another peer is pointing at a better (longer) chain: this is
          a [fork](#handling-forks).

### Handling forks

When we need to switch to another peer's chain that's not on our current `best_chain`, this is a fork. The following
picture represents a typical fork situation from a `Rollback "Alice" 4` message:

![Fork after a rollback](fork-after-rollback.jpg)

When a fork should happen:

1. We need to find the `rollback_point`, eg. the most recent intersection `Point` of the old and new `best_chain`s (node
   3 in the picture),
2. We update the `HeadersTree` so that:
    * `best_peer` points to the peer we forked to,
    * `best_chain` points to this peer's chain,
    * `peers` map for `peer` is updated to point to rollback `point` (4 in the example),
3. We emit a `SwitchToFork` `Result`:
   ```haskell
   SwitchToFork {
      rollback_point,
      peer,
      fork = [7, 8, 10]
   }
   ```

> [!WARNING]
>
> What if there are multiple peers pointing to the same new `best_chain`? Which one should become our `best_peer`?
> Shouldn't `best_peer` be a list, or a priority queue?

> [!WARNING]
>
> What if the new best chain is shorter than $k$, which is the case in the picture? This is very unlikely if the node is
> following a significant number of peers but not impossible in other cases. However, this means the peer supposedly
> knows another chain which is longer than our previous best chain, but they could be lying to us!
>
>
> Possible solution:
>   * We do not immediately switch to the (shorter) fork, which means we need to possibly track 2 nodes for each peer:
      the current best chain from this node, and the latest best chain before rollback if this chain was our best chain
      of length $k$
>   * The latter is updated once the node's current best chain catches up
>   * We use the the longest known chain from a peer to select out best chain

### Handling errors
