---
type: architecture
status: accepted
---

# Ledger validation context

## Context

The Cardano ledger is a Mealy machine whose transitions are formally specified. In Hindley-Milner notation, each transition roughly resembles:

```
st -> cmd -> (st, events)
```

Where:

- `st` designates the entire ledger state or a subset.
- `cmd` designates a block, a transaction or part of a transaction.

Cardano transactions are, in fact, akin to a collection of primitive operations that affect the state and a certain number of pre- and post-conditions. They are structured in fields and have a rather precise validation sequence order.

The entire validation sequence can be [mapped as a flow chart](https://app.excalidraw.com/l/7Ao24g28S3o/872UZdalVsy), which reads top-down and left-to-right.

## Motivation

### 1. Transition inter-dependencies and evolving context

The first challenge with implementing such a state machine resides in sub-transitions' inter-dependencies.

For example, a sequence of two certificates that (1) registers a stake pool and (2) delegates some credentials, can be valid in one order but invalid in another. The execution of the registration certificate does change the ongoing validation context for the second certificate. The transition doesn't consider the initial state but the virtual intermediary state being built along the way.

### 2. Interleaving of validations and I/O state changes

The second challenge stems from using an on-disk solution to manage this state effectively. Given the tight time constraints that the validation demands, performing an I/O operation for every state change isn't realistic. Plus, the interleaving of validation and I/O operations with side effects complicates the error handling and makes the overall code hard to test and instrument.

### 3. Additional context and extra information lookups

Besides, the transition rules cannot solely run from the local context of a block or transaction. In most cases, additional information is required. For example, the output address and values corresponding to a spent input aren't part of the transaction but are present in the UTxO state.

Furthermore, it isn't possible (or desirable) to load the entire ledger state in memory and operate from it. This defies the very design goals of the on-disk ledger state. In practice, it is also unnecessary. Even though transactions require an extra context, that context is negligible in front of the entire ledger state.

### 4. Validations-as-a-library

There's an explicit desire to keep the block/transaction validation rules as lean as possible and free of any storage or state management concerns. The use case here resides in providing better libraries for client applications, which could leverage battle-tested rules from full-node implementations to pre-validate transactions and provide a better user experience without re-implementing such validations.

From an audit standpoint, keeping the validation rules lean enables a better understanding of the overall ledger's behaviour, which can otherwise be rapidly obfuscated in implementation details and complex data structure updates.

## Decision

### Abstract away state management from validations

To decouple validations from state-management, we need to _at least_ abstract one or the other. After a few debates, our choice leaned towards abstracting the state management from the validations and keeping validation rules as the primary logic driver. We, therefore, provide an abstract interface as an eDSL available to the validation rules. The concrete implementation of those state operations is specified independently and chosen at the top of the stack by the caller.

For example, we can capture the operation around UTxO management as follows:

```rust
pub trait UtxoStateManagement {
    fn consume(&mut self, input: TransactionInput);
    fn produce(&mut self, input: TransactionInput, output: TransactionOutput);
}
```

> [!NOTE]
> We chose to provide an API around mutable references instead of yielding a new state because we are writing Rust. While libraries exist to create and manage immutable data structures, they come with performance overheads and feel like shoehorning Haskell into Rust.

This allows the implementation and instantiation of different validation contexts for different needs (e.g. for testing). So far, we have identified the following operations across all validations, although they might evolve following our discoveries and needs. So far, the split corresponds to a split per entity, which roughly maps to the internal structure of our store. This may, however, not be the ideal split, so we might want to revisit it later. We should split traits based on their usage in the validation rules to simplify implementing a subset of the rules. The fact that they currently reflect our store structure is mostly just _a design accident_.

```rust
pub trait UtxoStateManagement {
    fn consume(&mut self, input: TransactionInput);
    fn produce(&mut self, input: TransactionInput, output: TransactionOutput);
}

pub trait PoolsStateManagement {
    fn register(&mut self, params: PoolParams);
    fn retire(&mut self, pool: PoolId, epoch: Epoch);
}

pub trait AccountsStateManagement {
    fn register(
        &mut self,
        credential: StakeCredential,
        state: AccountState,
    ) -> Result<(), RegisterError<AccountState, StakeCredential>>;

    fn delegate_pool(
        &mut self,
        credential: StakeCredential,
        pool: PoolId,
    ) -> Result<(), DelegateError<StakeCredential, PoolId>>;

    fn delegate_vote(
        &mut self,
        credential: StakeCredential,
        drep: DRep,
        pointer: CertificatePointer,
    ) -> Result<(), DelegateError<StakeCredential, DRep>>;

    fn unregister(&mut self, credential: StakeCredential);

    fn withdraw_from(&mut self, credential: StakeCredential);
}

pub trait DRepsManagement {
    fn register(
        &mut self,
        drep: StakeCredential,
        state: DRepState,
    ) -> Result<(), RegisterError<DRepState, StakeCredential>>;

    fn update(
        &mut self,
        drep: StakeCredential,
        anchor: Option<Anchor>,
    ) -> Result<(), UpdateError<StakeCredential>>;

    fn unregister(&mut self, drep: StakeCredential, refund: Lovelace);

    fn vote(&mut self, drep: StakeCredential);
}

pub trait ProposalsStateManagement {
    fn acknowledge(&mut self, pointer: ProposalPointer, proposal: Proposal);
}

pub trait CommitteeStateManagement {
    fn delegate_cold_key(
        &mut self,
        cc_member: StakeCredential,
        delegate: StakeCredential,
    ) -> Result<(), DelegateError<StakeCredential, StakeCredential>>;

    fn resign(
        &mut self,
        cc_member: StakeCredential,
        anchor: Option<Anchor>,
    ) -> Result<(), UnregisterError<CCMember, StakeCredential>>;
}
```

This approach mostly satifies our constraints (2) and (4) listed above, and (1) partially (explications further down). It allows the interleaving of ledger-state changes with validations while effectively keeping the implementation details out of the validation logic. It also gives the option to defer the concrete side-effects and I/O operations on the store.

```rust
fn validate_certificate<C>(
    context: &mut C,
    pointer: CertificatePointer,
    certificate: Certificate,
) -> Result<(), InvalidCertificates>
where
    C: AccountsStateManagement + PoolsStateManagement,
{
    match certificate {
        Certificate::StakeDeregistration(credential) | Certificate::UnReg(credential, _) => {
            // ...
            // some validations
            // ...

            context.unregister(credential);

            Ok(())
        },

        Certificate::PoolRetirement(id, epoch) => {
            // ...
            // some validations
            // ...

            context.retire(id, epoch);

            Ok(())
        }

        _ => ...
    }
}
```

### Lookup context & preparation

In addition to the state-management interface above, we sometimes require read-only lookups of specific entities. So, for all traits above, we define an extra `lookup` method and call the overall a _Slice_. For example, the `UtxoSlice` resembles:

```rust
pub trait UtxoSlice {
    fn lookup(&self, input: &TransactionInput) -> Option<&TransactionOutput>;
    fn consume(&mut self, input: TransactionInput);
    fn produce(&mut self, input: TransactionInput, output: TransactionOutput);
}
```

To satisfy the design constraint (3) above, we assume the slices have prefetched data and aren't fetching data from an on-disk store on the fly. This presents a challenge: how can we ensure the data is available when the `lookup` occurs?

To overcome this, we introduce an extra step which we call _preparation_, and whose role is to traverse the block once to identify which entities will be needed by the validation rules, thus ensuring that the data needed down the line is identified and fetched from the storage in a single batch. Note that the validation context is bounded because blocks are bounded in size. So, the amount of data that needs to be prefetched is limited and acceptable to live transiently in memory during validations. We thus define the following additional traits:

```rust
pub trait PrepareUtxoSlice<'a> {
    fn require_input(&'_ mut self, input: &'a TransactionInput);
}

pub trait PreparePoolsSlice<'a> {
    fn require_pool(&'_ mut self, pool: &'a PoolId);
}

pub trait PrepareAccountsSlice<'a> {
    fn require_account(&'_ mut self, credential: &'a StakeCredential);
}

pub trait PrepareDRepsSlice<'a> {
    fn require_drep(&'_ mut self, credential: &'a StakeCredential);
}
```

With this, we can perform a single preparation step, which then leads to the creation of a validation slice that reflects a subset (a.k.a a slice) of the ledger state relevant to the transaction. For example, one can identify necessary inputs to lookup as such:

```rust
pub fn prepare_block<'block>(
    context: &mut impl PreparationContext<'block>,
    block: &'block MintedBlock<'_>,
) {
    block.transaction_bodies.iter().for_each(|transaction| {
        let inputs = transaction.inputs.iter();

        let collaterals = transaction
            .collateral
            .as_deref()
            .map(|xs| xs.as_slice())
            .unwrap_or(&[])
            .iter();

        let reference_inputs = transaction
            .reference_inputs
            .as_deref()
            .map(|xs| xs.as_slice())
            .unwrap_or(&[])
            .iter();

        inputs
            .chain(reference_inputs)
            .chain(collaterals)
            .for_each(|input| context.require_input(input));
    });
}
```

> [!NOTE]
> Preparation steps do not own their data and come with a lifetime bound to the block itself. This reduces the amount of data we need to copy, thus making the preparation step extremely efficient. The data is then either fetched from the database or found in the block.

### Limiting traversals of sub-fields

The slices partially address our concern (1) since they provide ways of pushing updates to the state during validation. Internally, the data structure implementing the validation context can then appropriately handle any modification to the context. For example, one could imagine a UTxO slice implemented using a hash map. Consuming a UTxO would effectively remove an item from the map so that subsequent calls to `.lookup` fail. Hence, if a transaction consumes an input, that input is effectively unavailable to another transaction down the line.

This works well for updates that outlive the validations and that have an effect on the ledger state. But it doesn't help with local validation constraints. For example, when validating a stake credential delegation certificate, one expects a signature (or, more generally, a witness) that authorizes the use of the credential. To keep the validation readable, somewhat structured and performant, we do not necessarily want to validate the witness immediately -- since this would demand identifying and finding the right witness, traversing the witness as many times as we have requirements on them, effectively leading to an `O(n * log n)` asymptotic cost. Similarly, to identify which witnesses are necessary, we must look back at specific fields of the transaction and collect those requiring a witness.

We aim to fuse those steps by allowing the slices to collect _local validation constraints_ while traversing the transaction. Hence, when validating a certificate, one can require witness validation later without processing the witnesses immediately. This minimizes the number of traversals we must perform on the transaction fields and lowers the cost to an `O(n)` in the case of witnesses. For now, we introduce a `WitnessSlice` to capture this behaviour.

```rust
pub trait WitnessSlice {
    /// Indicate that a witness is required to be present (and valid) for the corresponding
    /// set of credentials.
    fn require_witness(&mut self, credential: StakeCredential);

    /// Indicate that a bootstrap witness is required to be present (and valid) for the corresponding
    /// root.
    fn require_bootstrap_witness(&mut self, root: Hash<28>);

    /// Obtain the full list of required signers collected while traversing the transaction.
    fn required_signers(&mut self) -> BTreeSet<Hash<28>>;

    /// Obtain the full list of required bootstrap witnesses collected while traversing the
    /// transaction.
    fn required_bootstrap_signers(&mut self) -> BTreeSet<Hash<28>>;
}
```

### Further considerations

The current design will effectively process blocks in three passes:

1. Parsing
2. Preparation
3. Validation

It sounds _reasonable_ and more performant to combine the parsing & preparation steps and only requires two passes. Since the block is already being traversed for parsing, we could maintain an additional parsing context that takes care of marking elements to be fetched for validation. Besides, this approach seems well-geared towards the libraries that we already use for decoding CBOR (a.k.a [minicbor](https://docs.rs/minicbor/latest/minicbor/)) which provides [a mechanism](https://docs.rs/minicbor/latest/minicbor/fn.decode_with.html) for writing decoders that take an additional mutable context as parameter.

This change would, however, require rewriting many of the decoders in Pallas to support decoding from such a custom context. Since the context wouldn't change the underlying serialization, we could reuse the existing decoders based on the unit (i.e., `()`) context.

## Discussion points

### Ongoing Challenge of Ledger vs. State Duplication

We noticed that we're doing parallel work on the "ledger rules" side (validations during block processing) and the "state" side (tracking governance proposals, votes, UTxOs, accounts, etc.). Each side has very similar code, which leads to duplication and potentially double work: we ended up implementing the same checks twice, with data processing also happening in two separate places.

### Why This Duplication Happens

We have validations that need to access additional context beyond just what's in a transaction (e.g., credentials, governance parameters, or DRep state). Right now, some of that context lives in our volatile DB and some in stable storage. We initially wanted to keep ledger rules purely about validation, separated from specific storage details, but that separation complicates how we update the state alongside validation.

### Idea to Merge "Rules" and "State"

Our goal was to unify the code so that after each validation, we could directly derive and apply the state changes ("diff"). We identified that it would be possible by having the rules operating over a set of traits (interfaces) that both provide the necessary lookup (to fetch items like UTxOs, pool parameters, or account info) and handle updates (to consume an input, produce an output, retire a pool, etc.).
  - These traits can be backed by our existing volatile state so that when validation finishes, our updated state is already computed.
  - By using traits, we also allow other consumers (e.g., Dolos) to ignore or discard updates if they only need validation.

### Context Prefetch & Incremental State Updates

We discussed how some validations within a *single* transaction can depend on newly mutated state (e.g., a transaction might first register a credential, then immediately use that credential in a delegation certificate).

We acknowledged that we need to carefully handle ordering to ensure that each certificate or input is seen in the correct context.

We also discussed prefetching the data ahead of time (by traversing the block or transactions) so that when we perform the actual validation, we avoid multiple repetitive database lookups.

### Performance Considerations

We recognized that we don't want to traverse the same fields or sub-fields multiple times if there are many validations and checks. We want to organize the validation logic to handle each field (e.g., all transaction inputs) in one pass, minimizing overhead.

### Implementation Details and Potential Enhancements

We've started sketching traits like `UtxoSlice`, `PoolsSlice`, `AccountsSlice`, etc., which define both lookup and mutation methods (e.g., `consume`, `produce`, `register`, `retire`).
  We considered a more sophisticated structure for prefetching data (to keep the logic near where the validation happens). Still, we concluded that we could achieve acceptable performance by building the slices once for each block.
  We debated how best to organize files and rules, such as whether to organize by transaction fields or by topic (UTxO, staking, governance, etc.). We're leaning toward grouping logic by field to reduce repeated traversals.

Ultimately, we decided to move forward with trait-based state interfaces that will replace the current duplication between rules and state, allowing us to integrate validation and state updates in a single pass. We can keep them separate enough to remain flexible and pluggable but close enough to avoid double work when processing each block.
