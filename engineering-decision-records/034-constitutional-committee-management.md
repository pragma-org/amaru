---
type: architecture
status: accepted
---

# Constitutional Committee Management

## Context

<!-- Provide context, explain where the decision came from and why it's necessary to make one. -->

The constitutional committee is modelled in a particularly interesting fashion
in the Haskell Cardano node and has given us a lot of headaches to make it fit
properly within our various abstraction layers.

In Haskell, the committee is captured by:

- An optional `Committee` type which comprises:
  - a `Map` from cold credential to its validity period
  - a voting threshold as a rational
  - See the upstream definition of
    [`Committee`](https://github.com/intersectmbo/cardano-ledger/blob/d00f244577cefc3abf7f6bb29b8e66d9bbb6eb49/eras/conway/impl/src/Cardano/Ledger/Conway/Governance/Procedures.hs#L544-L595).

- A `CommitteeState` which translates as a `Map` from cold credential to
  either:
  - an _authorization_ (i.e. delegation to a hot credential)
  - a _resignation_ (with an optional anchor).
  - See the upstream definitions of
    [`CommitteeAuthorization`](https://github.com/intersectmbo/cardano-ledger/blob/d00f244577cefc3abf7f6bb29b8e66d9bbb6eb49/libs/cardano-ledger-core/src/Cardano/Ledger/State/CertState.hs#L270-L294)
    and
    [`CommitteeState`](https://github.com/intersectmbo/cardano-ledger/blob/d00f244577cefc3abf7f6bb29b8e66d9bbb6eb49/libs/cardano-ledger-core/src/Cardano/Ledger/State/CertState.hs#L296-L316).

The two maps are managed somewhat independently and "synchronized" every
epoch. Among the notable behaviors:

1. A _candidate_ committee member (i.e. added in an active but not yet ratified
  proposal) is allowed to register a hot credential in the `CommitteeState`,
  but is not part of the `Committee`.

2. Similarly, a _candidate_ committee member can also resign, prior to being
  elected (yes, yes, ...).

3. In any case, a member that has previously resigned cannot authorize a new hot
  credential, and cannot resign again (might it be with a different rationale)
  in the same epoch.
  - This is enforced in the Conway governance certificate rule:
    [`ConwayCommitteeHasPreviouslyResigned`](https://github.com/intersectmbo/cardano-ledger/blob/d00f244577cefc3abf7f6bb29b8e66d9bbb6eb49/eras/conway/impl/src/Cardano/Ledger/Conway/Rules/GovCert.hs#L186-L204).
  - There are subtle scenarios associated with this rule, which are detailed
    below.

4. Voting conditions are handled separately and are entirely bound to the hot
  credential. To be a valid _voter_, the hot credential must come from an
  authorization (i.e. entry in `CommitteeState`) of a member from the
  `Committee`
  - This means that an expired member can technically vote beyond its
    mandate, but a resigned member cannot (there is no stake credential left).
  - In protocol version 10, not-yet-elected members with a registered hot
    credential could also cast votes.

5. Whether a vote is valid is then deferred to the epoch boundary ratification.
  Votes from members that are expired, not elected or that have resigned are
  discarded.
  - See
    [`committeeAccepted` / `committeeAcceptedRatio`](https://github.com/intersectmbo/cardano-ledger/blob/d00f244577cefc3abf7f6bb29b8e66d9bbb6eb49/eras/conway/impl/src/Cardano/Ledger/Conway/Rules/Ratify.hs#L123-L160).

6. The same hot credential can be associated with more than one cold credential.

7. The members of the `Committee` and the value of their validity period can
  only change at an epoch boundary, as a result of a corresponding governance
  action being enacted.

8. A motion of no confidence sets the `Committee` to `None`, effectively removing
   the entire map of elected committee members.
   - See the `NoConfidence` case in the enactment rule:
     [`ensCommitteeL .~ SNothing`](https://github.com/intersectmbo/cardano-ledger/blob/d00f244577cefc3abf7f6bb29b8e66d9bbb6eb49/eras/conway/impl/src/Cardano/Ledger/Conway/Rules/Enact.hs#L104-L111).

9. At the beginning of every epoch, after ratification/enactment, the
   `CommitteeState` is restricted to only cold credentials present in the
   `Committee`. This effectively prunes any dangling hot credential delegation
   or resignations for non-elected members.
   - This happens through
     [`updateCommitteeState`](https://github.com/intersectmbo/cardano-ledger/blob/d00f244577cefc3abf7f6bb29b8e66d9bbb6eb49/eras/conway/impl/src/Cardano/Ledger/Conway/Rules/Epoch.hs#L350-L350)
     and its definition as a `Map.intersection` with the elected committee:
     [`updateCommitteeState`](https://github.com/intersectmbo/cardano-ledger/blob/d00f244577cefc3abf7f6bb29b8e66d9bbb6eb49/eras/conway/impl/src/Cardano/Ledger/Conway/Rules/Epoch.hs#L426-L430).
   - Consequently, a motion of no confidence will also empty every existing hot
     credential or resignation mapping as part of the same epoch transition.
   - A candidate member that registers a hot credential and that isn't elected
     at the next epoch boundary must eventually re-register the hot credential
     since it will be removed at the epoch boundary.
   - Similarly, a candidate member that resigns in an epoch can actually
     re-register a hot credential in the next epoch (or resign again) if not
     elected. The resignation is not preserved.
   - An expired member and its corresponding hot credential or resignation
     bindings remain in the `CommitteeState` as long as the member is not
     explicitly removed (via a governance action) from the `Committee`.

## Decision

<!-- Explain the decision with sufficient details to be self-explanatory. -->

### Observations

From these various points, we can derive a few interesting observations which
have motivated our design:

1. CC Members are only added or removed at an epoch boundary, via governance
   actions (update committee or motion of no confidence).

2. Blocks can only ever update the `CommitteeState`: the hot credential and/or
   the resignation.

3. Blocks can never remove or add members, nor change their validity period.

### Our State

We therefore represent the two Haskell maps as:

- A single key/value map where keys are cold credentials and values are:
  - an optional epoch number which serves as a validity period
  - an optional `CommitteeMemberStatus` which can be either `Resigned` or `Delegated StakeCredential`.

We store the committee threshold entirely separately.

In practice, this means that:

- A freshly elected member has a non-null epoch mandate; that epoch may or may not be in the future.
- A member may have a resignation or a delegation, irrespective of whether they have a mandate.
  - This means that a member present in a proposal who would register a hot
    credential (or resignation) would correspond to a DB entry with no epoch.
- At epoch boundary, we can remove all entries that have no associated epoch (this is our "intersection").

### `Bind<L = CommitteeMemberStatus, R = Epoch, V = Empty>`

This fits particularly well with our notion of `Bind` where:

- a `left`-bind is associated with a hot credential or resignation.
- a `right`-bind is associated with an epoch.
- a bind's `value` is always empty (blocks never 'register' a cc-member).

The `left` binds can only be produced by blocks, as the result of processing
certificates.

Conversely, `right` binds are only produced by epoch transitions, which live
in the overlay.

The semantics of `Bind.then` apply nicely:

```rs
pub fn then(&mut self, newer: Self) {
    if newer.value.is_some() {
        *self = newer;
    } else {
        if !matches!(newer.left, Resettable::Unchanged) {
            self.left = newer.left;
        }
        if !matches!(newer.right, Resettable::Unchanged) {
            self.right = newer.right;
        }
    }
}
```

- Neither `self` nor `newer` can have a value, so only the `else` branch applies
- A more recent left bind other than `Unchanged` replaces an older left bind
- A more recent right bind other than `Unchanged` replaces an older right bind

### `Existence<Bind<CommitteeMemberStatus, Epoch, Empty>>`

Similarly, an `Existence` of such a bind becomes handy to track updates across
the volatile. Each layer (current, overlay, draining) in the volatile can
resolve a member's `Existence`:

- The current and draining series can only ever yield `Exists` with non-empty
  left binds or `Unknown`.
- The overlay can yield either `Exists` with right binds, `Gone` (removed
  members or no-confidence) or `Unknown` (no related governance action).

And they compose nicely from the most recent to least recent using `Existence::fold`:

- A `Gone` is terminal, once encountered, it supersedes any older state.

- An `Unknown` state defers to the next source; so a missing member in the
  current series will fall back to the overlay, which will in turn fall back to
  the draining sequence and then to the stable store.

- A recent `Exists` will always apply on top of previous `Exists` states, so
  that newer left or right binds overwrite older ones. The resulting bind contains the most
  recent left and right binds in the sequence, unless superseded by a `Gone`
  state.



## Consequences

<!-- Describe the result/consequences of applying that decision; both positive and negative outcomes -->

### Edge cases

- We require constitutional committee information in two cases:
  - when a related CC member certificate is found in the transaction; in which
    case the member can be resolved by cold credential
  - when a CC voter is found in a voting procedure

  However, we do not have an index of hot credential to member(s). We must
  therefore scan the entire CC member column to determine which ones match the
  hot credential. This was deemed a reasonable approach due to the tiny size of
  the CC. That tiny size is not guaranteed by the protocol, but rather by
  social consensus, and reaching a size at which this becomes a practical
  problem would require multiple governance actions, leaving plenty of time to
  react.
  - The same asymmetry exists upstream: hot credentials are derived by scanning
    committee state rather than via a unique hot-to-cold index; see
    [`authorizedHotCommitteeCredentials`](https://github.com/intersectmbo/cardano-ledger/blob/d00f244577cefc3abf7f6bb29b8e66d9bbb6eb49/libs/cardano-ledger-core/src/Cardano/Ledger/State/CertState.hs#L303-L311)
    and
    [`queryCommitteeMembersState`](https://github.com/intersectmbo/cardano-ledger/blob/d00f244577cefc3abf7f6bb29b8e66d9bbb6eb49/libs/cardano-ledger-api/src/Cardano/Ledger/Api/State/Query.hs#L277-L320).

- An elected member can exist solely in the volatile state and not be in the
  stable store. This is true for every newly elected member, which is only present in
  the overlay until the end of the stabilization window. Hence, the state of
  _current_ CC members cannot be reconstructed from the stable store alone.

- It is possible for a member to be removed via a governance action, and yet
  still be a valid cold credential for delegation; this happens in the case
  where two constitutional committee actions are (correctly) chained so that
  the first removes a member that the next one re-adds. The overlay must not
  recklessly mark CC members as "Gone" if they appear in another proposal.

## Discussion points

<!-- Summarizes, a posteriori, the major discussion points around that decision -->
