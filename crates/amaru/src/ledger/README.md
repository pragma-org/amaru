# Amaru's Ledger

## Role

The Amaru's ledger is responsible for tracking the ledger state necessary to (a) the consensus layer to validate block bodies (and headers indirectly), and (b) clients applications interested in the ongoing state of the chain.

Fundamentally, the ledger is a finite-state transducer driven by transactions. Said differently, it holds on a state that is updated into a new state by applying transactions. That state can be roughly represented as:

```
Ledger State
 ├─ UTxO (TxIn |-> TxOut)
 ├─ Stake distribution (StakeCredential |-> Lovelace)
 ├─ Certificates
 │   ├─ DReps (DRepID |-> Epoch, Anchor, Lovelace, Set StakeCredential)
 │   ├─ Committee (CCID |-> Committee State)
 │   ├─ SPOs
 │   │   ├─ Current Parameters (PoolID |-> PoolParams)
 │   │   ├─ Future Parameters (PoolID |-> PoolParams)
 │   │   ├─ Retirements (PoolID |-> Epoch)
 │   │   └─ Deposits (PoolID |-> Lovelace)
 │   └─ Ada holders
 │       ├─ Deposits (StakeCredential |-> (Lovelace, Lovelace))
 │       ├─ Pool Delegations (StakeCredential |-> Option PoolID)
 │       └─ Gov Delegations (StakeCredential |-> Option DRepID)
 ├─ Governance State
 │   ├─ Proposals
 │   ├─ Committee
 │   ├─ DReps
 │   ├─ Constitution
 │   └─ Protocol Parameters (current, previous & future)
 ├─ Deposited (Lovelace)
 ├─ Fees (Lovelace)
 └─ Donation (Lovelace)
 ```

> [!NOTE]
> At this point of Amaru's life, we do not yet look at the governance state but will eventually. We're still focused on the overall stake distribution.

So, the ledger state is fundamentally a very large key:value store. Some of those key:value pairs represent a relationship between two entities (e.g. pool delegations and gov delegations). It's important to note that the ledger doesn't need to hold onto the block/transaction history. Transactions can be discarded as soon as they've been applied. Storing blocks is therefore not a responsibility of the ledger.

Now, tracking the ledger state comes with a few challenges:

1. The state transitions are only eventually final; they're only final after `k` blocks. In practice, a single node will need to track multiple chains (at most, one per peer it is connected to) and make a choice for the one it considers as the most plausible candidate. It must do so while maintaining the state across all the candidate chains, and with the ability to switch between them in no-time since it mustn't disrupt the block production.

2. Many operations in the ledger are not immediate, but some are. For example, updating the parameters associated to a stake pool only takes effect on the following epoch boundary. Similarly, retiring a stake pool is only done at the epoch specified in the retirement certificate. So, many transitions of the ledger state must be stashed and applied later at epoch boundary (potentially creating a significant load on the node at each epoch boundary).

3. The calculation of the rewards and the leader schedule are done on snapshots of the stake distribution -- not on the current stake distribution. Thus, the ledger needs the ability to look back up to 2 epochs in the past.

## Design

In the Haskell implementation, those challenges are tackled by maintaining a single unified ledger state as a tree-like structure in plain RAM. By leveraging the persistent nature of various Haskell functional data-structure, it is possible to keep the `k=2160` versions of the ledger state in memory without requiring 2160 times the resources (since two versions are roughly similar and only contain a few changes). Stake distributions snapshots are also kept in memory for as long as they're needed and naturally shift over at every epoch boundary. While practical, this solution results in a rather large memory requirement (>16GB) for the node which we intend to solve for Amaru.

```
VOLATILE DB                        STABLE DB
*-------------*-----*------*       *--------*------------*
| Δ(t - 2159) | ... | Δ(t) |   +   | TxIn   | TxOut      |
*-------------*-----*------*       *--------*------------*
                                   | PoolId | PoolParams |
                                   *--------*------------*
                                   | ...    | ....       |
```

In Amaru, the ledger state is stored in an hybrid fashion. The _stable_ part of the ledger is stored in a key:value store (currently using RocksDB as a backend), while we keep in memory the last `k` delta to apply to that state in plain RAM in a _volatile_ FIFO data-structure. As we process block, we compute the next delta corresponding to the block and push it onto the volatile FIFO data-structure, and we apply the latest delta that is now considered stable.

When looking up information at the current tip, we perform the following:

- Compute the resulting delta from executing all delta in sequence (note that this can be cached and computed incrementally);
- Check whether the information is available in the delta only (e.g. if looking for a transaction output, it may be sufficient to look it up from the delta)
- Otherwise, reach for the stable database.

Some delta changes have immediate effects (e.g. consuming a UTxO entry, or registering a new stake pool), whereas some will be stored as _future states_ marked with an activation epoch (e.g. updating stake pool parameters). When fetching data from the stable database, it may thus be necessary to also select the state corresponding to the right epoch. At each epoch boundary, the future states are analyzed and made active if applicable.

On top of that, we perform a snapshot of the stable db on each epoch boundary (i.e. when we apply to the stable db the last block of a given epoch). The snapshot is done by copying the on-disk state.

### A remark about linearity

Note that, like the Haskell implementation, we make the choice to keep our view of the ledger mostly linear. It is possible to move backward in time (up to `k` blocks), but a single _ledger state_ is a linear view of **a** chain candidate. This means that in order to track multiple candidates, Amaru will have to instantiate multiple ledger components. However, the design split is such that all ledger instances can share a single stable database and only need to maintain their own volatile view.

Consequently, the stable database must be concurrent-safe by design, as stable blocks may be applied out of order and multiple times by the different ledger components.

## Open Questions / TODOs

- [ ] The approach we take is very much optimized towards the nominal mode of the node, when it is already synchronized and is following the tip. This is quite different from the Haskell node which is optimized for syncing. Amaru removes the syncing problem entirely by bootstrapping from snapshots. At this point, snapshots can be produced by the Haskell node, and in the long-run, they will be able to be produced by Amaru itself. Yet, the time to sync from a snapshot should remain "acceptable" (to be measured).

- [ ] We need to properly measure a few things to assert whether the approach is valid overall. In particular:
    - [ ] The overall validation time of a block from end to end
    - [ ] The memory footprint of the volatile DB.
    - [ ] The time needed at the epoch boundary to advance the stable DB (i.e. process all retirements & epoch events)
    - [ ] The time and space needed to store snapshots of the database at the epoch boundary

- [ ] If some of the time above ends up being too significant to be done in a synchronous fashion, we might resort to doing them in a separate thread. It's rather certain already that rewards calculation will need to happen in a separate thread. This might complicate the internal structure a bit.

- [ ] Validations of the ledger state is relatively cumbersome, but a first low-hanging fruit is to compare it with snapshots produced from the Haskell node. So, we've been compiling [data](../../data) from the PreProd network to be compared with.
