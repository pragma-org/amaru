use super::{
    kernel::{
        block_point, epoch_slot, Hash, Hasher, MintedBlock, Point, PoolSigma, TransactionInput,
        TransactionOutput,
    },
    store::{self, Store},
};
use crate::ledger::kernel::{PoolId, CONSENSUS_SECURITY_PARAM};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
};

// State
// ----------------------------------------------------------------------------

/// The state of the ledger split into two sub-components:
///
/// - A _stable_ and persistent storage, which contains the part of the state which known to be
///   final. Fundamentally, this contains the aggregated state of the ledger that is at least 'k'
///   blocks old; where 'k' is the security parameter of the protocol.
///
/// - A _volatile_ state, which is maintained as a sequence of diff operations to be applied on
///   top of the _stable_ store. It contains at most 'CONSENSUS_SECURITY_PARAM' entries; old entries
///   get persisted in the stable storage when they are popped out of the volatile state.
pub struct State<'a, E> {
    tip: Point,
    stable: StableDB<'a, E>,
    volatile: VolatileDB,
}

type StableDB<'a, E> = Arc<dyn Store<Error = E> + Send + Sync + 'a>;

impl<'a, E: std::fmt::Debug> State<'a, E> {
    pub fn new(stable: StableDB<'a, E>) -> Self {
        Self {
            tip: stable
                .get_tip()
                .unwrap_or_else(|e| panic!("unable to initialize ledger-state's tip: {e:?}")),
            stable,
            // NOTE: At this point, we always restart from an empty volatile state; which means
            // that there needs to be some form of synchronization between the consensus and the
            // ledger here. Few assumptions also stems from this:
            //
            // (1) The consensus must be storing its own state, and in particular, where it has
            //     left the synchronization.
            //
            // (2) Re-applying 2160 (already synchronized) blocks is _fast-enough_ that it can be
            //     done on restart easily. To be measured; if this turns out to be too slow, we
            //     will consider storing views of the volatile DB on-disk to be able to restore
            //     them quickly.
            volatile: VecDeque::new(),
        }
    }

    /// Roll the ledger forward with the given block by applying transactions one by one, in
    /// sequence. The update stops at the first invalid transaction, if any. Otherwise, it updates
    /// the internal state of the ledger.
    pub fn forward(&mut self, block: MintedBlock<'_>) -> Result<(), ForwardErr<E>> {
        let point = block_point(&block);

        let st = apply_block(block);

        if self.volatile.len() >= CONSENSUS_SECURITY_PARAM {
            let now_stable = self.volatile.pop_front().unwrap();
            self.stable
                .save(
                    &point,
                    store::Add {
                        utxo: Box::new(now_stable.utxo.produced.into_iter()),
                        ..Default::default()
                    },
                    store::Remove {
                        utxo: Box::new(now_stable.utxo.consumed.into_iter()),
                        ..Default::default()
                    },
                )
                .map_err(ForwardErr::StorageErr)?;
        }

        self.volatile.push_back(st.anchor(point));

        Ok(())
    }

    pub fn backward<'b>(&mut self, to: &'b Point) -> Result<(), BackwardErr<'b>> {
        if let Some(ix) = self.volatile.iter().position(|diff| &diff.point == to) {
            self.volatile.resize_with(ix + 1, || {
                unreachable!("ix is necessarly strictly smaller than the length")
            });
            Ok(())
        } else {
            Err(BackwardErr::UnknownRollbackPoint(to))
        }
    }
}

/// Process a given block into a series of ledger-state diff (a.k.a events) to apply.
fn apply_block(block: MintedBlock<'_>) -> VolatileState<()> {
    let failed_transactions = FailedTransactions::from_block(&block);

    let mut st = VolatileState::default();

    for (ix, transaction_body) in block.transaction_bodies.to_vec().into_iter().enumerate() {
        let transaction_id = Hasher::<256>::hash(transaction_body.raw_cbor());

        let transaction_body = transaction_body.unwrap();

        let (inputs, outputs) = match failed_transactions.has(ix as u32) {
            // NOTE: Successful transaction: standard inputs are consumed, and outputs produced.
            false => {
                let inputs = transaction_body.inputs.to_vec().into_iter();
                let outputs = transaction_body.outputs.into_iter().map(|x| x.into());
                (
                    Box::new(inputs) as Box<dyn Iterator<Item = TransactionInput>>,
                    Box::new(outputs) as Box<dyn Iterator<Item = TransactionOutput>>,
                )
            }

            // NOTE: Failed transaction: collateral inputs are consumed, and collateral outputs produced (if any)
            true => {
                let inputs = transaction_body
                    .collateral
                    .map(|x| x.to_vec())
                    .unwrap_or_default()
                    .into_iter();

                let outputs = match transaction_body.collateral_return {
                    Some(output) => Box::new([output.into()].into_iter())
                        as Box<dyn Iterator<Item = TransactionOutput>>,
                    None => {
                        Box::new(std::iter::empty()) as Box<dyn Iterator<Item = TransactionOutput>>
                    }
                };

                (
                    Box::new(inputs) as Box<dyn Iterator<Item = TransactionInput>>,
                    outputs,
                )
            }
        };

        st.merge(apply_transaction(&transaction_id, inputs, outputs));
    }

    st
}

fn apply_transaction(
    transaction_id: &Hash<32>,
    inputs: impl Iterator<Item = TransactionInput>,
    outputs: impl Iterator<Item = TransactionOutput>,
) -> VolatileState<()> {
    let mut consumed = BTreeSet::new();
    consumed.extend(inputs);

    let mut produced = BTreeMap::new();
    for (ix, output) in outputs.enumerate() {
        let input = TransactionInput {
            transaction_id: *transaction_id,
            index: ix as u64,
        };
        produced.insert(input, output);
    }

    VolatileState {
        utxo: DiffSet { consumed, produced },
        ..VolatileState::default()
    }
}

impl<'a, E: std::fmt::Debug> ouroboros::ledger::LedgerState for State<'a, E> {
    fn pool_id_to_sigma(&self, _pool_id: &PoolId) -> Result<PoolSigma, ouroboros::ledger::Error> {
        // FIXME: Obtain from ledger's stake distribution
        Err(ouroboros::ledger::Error::PoolIdNotFound)
    }

    fn vrf_vkey_hash(&self, pool_id: &PoolId) -> Result<Hash<32>, ouroboros::ledger::Error> {
        // FIXME: Look in the volatile part first
        self.stable
            .get_pool(pool_id, epoch_slot(self.tip.slot_or_default()))
            .unwrap_or_else(|e| panic!("unable to fetch pool from database: {e:?}"))
            .map(|params| params.vrf)
            .ok_or(ouroboros::ledger::Error::PoolIdNotFound)
    }

    fn slot_to_kes_period(&self, slot: u64) -> u64 {
        // FIXME: Extract from genesis configuration.
        let slots_per_kes_period: u64 = 129600;
        slot / slots_per_kes_period
    }

    fn max_kes_evolutions(&self) -> u64 {
        // FIXME: Extract from genesis configuration.
        62
    }

    fn latest_opcert_sequence_number(&self, _issuer_vkey: &[u8]) -> Option<u64> {
        // FIXME: Obtain from protocol's state
        None
    }
}

// FailedTransactions
// ----------------------------------------------------------------------------

struct FailedTransactions {
    inner: BTreeSet<u32>,
}

impl FailedTransactions {
    pub fn from_block(block: &MintedBlock<'_>) -> Self {
        FailedTransactions {
            inner: block
                .invalid_transactions
                .as_deref()
                .map(|indices| {
                    let mut tree = BTreeSet::new();
                    tree.extend(indices.to_vec().as_slice());
                    tree
                })
                .unwrap_or_default(),
        }
    }

    pub fn has(&self, ix: u32) -> bool {
        self.inner.contains(&ix)
    }
}

// VolatileDB
// ----------------------------------------------------------------------------

// NOTE: Once we implement ledger validation, we might want to maintain aggregated version(s) of
// this sequence of _DiffSet_ at different points, such that one can easily lookup the volatile database
// before reaching for the stable storage.
//
// Otherwise, we need to traverse the entire sequence for any query on the volatile state.
type VolatileDB = VecDeque<VolatileState<Point>>;

struct VolatileState<T> {
    pub point: T,
    pub utxo: DiffSet<TransactionInput, TransactionOutput>,
}

impl Default for VolatileState<()> {
    fn default() -> Self {
        Self {
            point: (),
            utxo: Default::default(),
        }
    }
}

impl VolatileState<()> {
    pub fn anchor(self, point: Point) -> VolatileState<Point> {
        VolatileState {
            point,
            utxo: self.utxo,
        }
    }
}

impl<T> VolatileState<T> {
    pub fn merge(&mut self, other: VolatileState<T>) {
        self.utxo.merge(other.utxo);
    }
}

// DiffSet
// ----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffSet<K: Ord, V> {
    pub consumed: BTreeSet<K>,
    pub produced: BTreeMap<K, V>,
}

impl<K: Ord, V> Default for DiffSet<K, V> {
    fn default() -> Self {
        Self {
            consumed: Default::default(),
            produced: Default::default(),
        }
    }
}

impl<K: Ord, V> DiffSet<K, V> {
    pub fn merge(&mut self, other: DiffSet<K, V>) {
        self.produced.retain(|k, _| !other.consumed.contains(k));
        self.consumed.retain(|k| !other.produced.contains_key(k));
        self.consumed.extend(other.consumed);
        self.produced.extend(other.produced);
    }
}

#[cfg(test)]
mod diff_test {
    use super::*;
    use proptest::prelude::*;
    use std::collections::{BTreeMap, BTreeSet};

    prop_compose! {
        fn any_diff()(
            consumed in
                any::<BTreeSet<u8>>(),
            mut produced in
                any::<BTreeMap<u8, u8>>()
        ) -> DiffSet<u8, u8> {
            produced.retain(|k, _| !consumed.contains(k));
            DiffSet {
                produced,
                consumed,
            }
        }
    }

    proptest! {
        #[test]
        fn prop_merge_itself(mut st in any_diff()) {
            let original = st.clone();
            st.merge(st.clone());
            prop_assert_eq!(st, original);
        }
    }

    proptest! {
        #[test]
        fn prop_merge_no_overlap(mut st in any_diff(), diff in any_diff()) {
            st.merge(diff.clone());

            for (k, v) in diff.produced.iter() {
                prop_assert_eq!(
                    st.produced.get(k),
                    Some(v),
                    "everything newly produced is produced"
                );
            }

            for k in diff.consumed.iter() {
                prop_assert!(
                    st.consumed.contains(k),
                    "everything newly consumed is consumed",
                );
            }

            for (k, _) in st.produced.iter() {
                prop_assert!(
                    !st.consumed.contains(k),
                    "nothing produced is also consumed",
                )
            }

            for k in st.consumed.iter() {
                prop_assert!(
                    !st.produced.contains_key(k),
                    "nothing consumed is also produced",
                )
            }
        }
    }
}

// Errors
// ----------------------------------------------------------------------------

#[derive(Debug)]
pub enum ForwardErr<E> {
    StorageErr(E),
}

#[derive(Debug)]
pub enum BackwardErr<'a> {
    /// The ledger has been instructed to rollback to an unknown point. This should be impossible
    /// if chain-sync messages (roll-forward and roll-backward) are all passed to the ledger.
    UnknownRollbackPoint(&'a Point),
}
