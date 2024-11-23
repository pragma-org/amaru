use super::{
    kernel::{
        block_point, epoch_slot, Certificate, Epoch, Hash, Hasher, MintedBlock, Point, PoolId,
        PoolParams, PoolSigma, TransactionInput, TransactionOutput, CONSENSUS_SECURITY_PARAM,
    },
    store::{self, Store},
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
};
use tracing::info;
use vec1::{vec1, Vec1};

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

        let state = apply_block(block);

        if self.volatile.len() >= CONSENSUS_SECURITY_PARAM {
            let (add, remove) = self.volatile.pop_front().unwrap().into_store_update();
            self.stable
                .save(&point, add, remove)
                .map_err(ForwardErr::StorageErr)?;
        } else {
            info!(
                ?point,
                "warming up; {} volatile states",
                self.volatile.len()
            );
        }

        self.volatile.push_back(state.anchor(point));

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

    /// Fetch stake pool details from the current live view of the ledger.
    pub fn get_pool(&self, pool: &PoolId) -> Result<Option<PoolParams>, QueryErr<E>> {
        let current_epoch = epoch_slot(self.tip.slot_or_default());

        // NOTE: When we are near an epoch boundary, we need to consider the not-yet-stable changes
        // that belong to the previous epoch. Any re-registration or retirement that would now be
        // valid must be acknowledged.
        let volatile = self
            .volatile
            .iter()
            .fold(DiffEpochReg::default(), |mut state, step| {
                if epoch_slot(step.point.slot_or_default()) < current_epoch {
                    if let Some(registrations) = step.pools.registered.get(pool) {
                        state.register(pool, registrations.last());
                    }

                    if let Some(retirement) = step.pools.unregistered.get(pool) {
                        // NOTE: in principle, there's a minimum epoch retirement delay which will
                        // likely always stay larger than one epoch. Thus, the case where we have a
                        // retirement certificate in the previous epoch that is enacted in the
                        // current epoch will never occur. Yet, since it is technically bounded by
                        // a protocol parameter, we better get the logic right.
                        if retirement <= &current_epoch {
                            state.unregister(pool, *retirement);
                        }
                    }
                }

                state
            });

        if let Some(retirement) = volatile.unregistered.get(pool) {
            if retirement <= &current_epoch {
                return Ok(None);
            }
        }

        if let Some(registrations) = volatile.registered.get(pool) {
            return Ok(Some((*registrations.last()).clone()));
        }

        if let Some(state) = self.stable.get_pool(pool).map_err(QueryErr::StorageErr)? {
            if let Some(params) = state
                .future_params
                .iter()
                .filter(|(_, epoch)| epoch <= &current_epoch)
                .last()
            {
                return Ok(Some(params.0.clone()));
            }

            return Ok(Some(state.current_params));
        }

        Ok(None)
    }
}

/// Process a given block into a series of ledger-state diff (a.k.a events) to apply.
fn apply_block(block: MintedBlock<'_>) -> VolatileState<()> {
    let failed_transactions = FailedTransactions::from_block(&block);

    let mut state = VolatileState::default();

    for (ix, transaction_body) in block.transaction_bodies.to_vec().into_iter().enumerate() {
        let transaction_id = Hasher::<256>::hash(transaction_body.raw_cbor());

        let transaction_body = transaction_body.unwrap();

        let (inputs, outputs) = match failed_transactions.has(ix as u32) {
            // == Successful transaction
            // - inputs are consumed;
            // - outputs are produced.
            false => {
                let inputs = transaction_body.inputs.to_vec().into_iter();
                let outputs = transaction_body.outputs.into_iter().map(|x| x.into());
                (
                    Box::new(inputs) as Box<dyn Iterator<Item = TransactionInput>>,
                    Box::new(outputs) as Box<dyn Iterator<Item = TransactionOutput>>,
                )
            }

            // == Failed transaction
            // - collateral inputs are consumed;
            // - collateral outputs produced (if any).
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

        apply_transaction(
            &mut state,
            &transaction_id,
            inputs,
            outputs,
            // TODO: There should really be an Iterator instance in Pallas
            // on those certificates...
            transaction_body
                .certificates
                .map(|xs| xs.to_vec())
                .unwrap_or_default()
                .into_iter(),
        );
    }

    state
}

fn apply_transaction<T>(
    state: &mut VolatileState<T>,
    transaction_id: &Hash<32>,
    inputs: impl Iterator<Item = TransactionInput>,
    outputs: impl Iterator<Item = TransactionOutput>,
    certificates: impl Iterator<Item = Certificate>,
) {
    {
        // Inputs/Outputs
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

        state.utxo.merge(DiffSet { consumed, produced });
    }

    {
        // Certificates
        for certificate in certificates {
            match certificate {
                Certificate::PoolRetirement(id, epoch) => state.pools.unregister(id, epoch),
                Certificate::PoolRegistration {
                    operator: id,
                    vrf_keyhash: vrf,
                    pledge,
                    cost,
                    margin,
                    reward_account,
                    pool_owners: owners,
                    relays,
                    pool_metadata: metadata,
                } => state.pools.register(
                    id,
                    PoolParams {
                        id,
                        vrf,
                        pledge,
                        cost,
                        margin,
                        reward_account,
                        owners,
                        relays,
                        metadata,
                    },
                ),
                // FIXME: Process other types of certificates
                _ => {}
            }
        }
    }
}

impl<'a, E: std::fmt::Debug> ouroboros::ledger::LedgerState for State<'a, E> {
    fn pool_id_to_sigma(&self, _pool_id: &PoolId) -> Result<PoolSigma, ouroboros::ledger::Error> {
        // FIXME: Obtain from ledger's stake distribution
        Err(ouroboros::ledger::Error::PoolIdNotFound)
    }

    // FIXME: This method most probably needs pool from the mark or set snapshots (so one or two
    // epochs in the past), and not from the live view.
    fn vrf_vkey_hash(&self, pool_id: &PoolId) -> Result<Hash<32>, ouroboros::ledger::Error> {
        self.get_pool(pool_id)
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

/// Failed transactions aren'y immediately available in blocks. Only indices of those transactions
/// are stored. This internal structure provides a clean(er) interface to accessing those indices.
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

pub struct VolatileState<T> {
    pub point: T,
    pub utxo: DiffSet<TransactionInput, TransactionOutput>,
    pub pools: DiffEpochReg<PoolId, PoolParams>,
}

impl Default for VolatileState<()> {
    fn default() -> Self {
        Self {
            point: (),
            utxo: Default::default(),
            pools: Default::default(),
        }
    }
}

impl VolatileState<()> {
    pub fn anchor(self, point: Point) -> VolatileState<Point> {
        VolatileState {
            point,
            utxo: self.utxo,
            pools: self.pools,
        }
    }
}

impl VolatileState<Point> {
    pub fn into_store_update<'a>(self) -> (store::Add<'a>, store::Remove<'a>) {
        let epoch = epoch_slot(self.point.slot_or_default());

        info!(?self.point, "adding {} UTxO entries", self.utxo.produced.len());
        info!(?self.point, "removing {} UTxO entries", self.utxo.consumed.len());
        info!(?self.point, "registering/updating {} stake pools", self.pools.registered.len());
        info!(?self.point, "retiring {} stake pools", self.pools.unregistered.len());

        (
            store::Add {
                utxo: Box::new(self.utxo.produced.into_iter()),
                pools: Box::new(self.pools.registered.into_iter().flat_map(
                    move |(_, registrations)| {
                        registrations
                            .into_iter()
                            .map(|r| (r, epoch))
                            .collect::<Vec<_>>()
                    },
                )),
            },
            store::Remove {
                utxo: Box::new(self.utxo.consumed.into_iter()),
                pools: Box::new(self.pools.unregistered.into_iter()),
            },
        )
    }
}

// DiffEpochReg
// ----------------------------------------------------------------------------

/// A compact data-structure tracking deferred registration & unregistration changes in a key:value
/// store. By deferred, we reflect on the fact that unregistering a value isn't immediate, but
/// occurs only after a certain epoch (specified when unregistering). Similarly, re-registering is
/// treated as an update, but always deferred to some specified epoch as well.
///
/// The data-structure can be reduced through a composition relation that ensures two
/// `DiffEpochReg` collapses into one that is equivalent to applying both `DiffEpochReg` in
/// sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffEpochReg<K, V> {
    pub registered: BTreeMap<K, Vec1<V>>,
    pub unregistered: BTreeMap<K, Epoch>,
}

impl<K, V> Default for DiffEpochReg<K, V> {
    fn default() -> Self {
        Self {
            registered: Default::default(),
            unregistered: Default::default(),
        }
    }
}

impl<K: Ord, V> DiffEpochReg<K, V> {
    /// We reduce registration and de-registration according to the following rules:
    ///
    /// 1. A single `DiffEpochReg` spans over *a block*. Thus, there is no epoch change whatsoever
    ///    happening within a single block.
    ///
    /// 2. Beyond the first registration, any new registration takes precedence. Said differently,
    ///    there's always _at most_ two registrations.
    ///
    ///    In practice, the first registation could also *sometimes* be collapsed, if there's
    ///    already a registration in the stable storage. But we don't have acccess to the storage
    ///    here, so by default, we'll always keep the first registration untouched.
    ///
    /// 3. Registration immediately cancels out any unregistration.
    ///
    /// 4. There can be at most 1 unregistration per entity. Any new unregistration is preferred
    ///    and replaces previous registrations.
    pub fn register(&mut self, k: K, v: V) {
        self.unregistered.remove(&k);
        match self.registered.get_mut(&k) {
            None => {
                self.registered.insert(k, vec1![v]);
            }
            Some(vs) => {
                if vs.len() > 1 {
                    vs[1] = v;
                } else {
                    vs.push(v);
                }
            }
        }
    }

    // See 'register' for details.
    pub fn unregister(&mut self, k: K, epoch: Epoch) {
        self.unregistered.insert(k, epoch);
    }
}

#[cfg(test)]
mod diff_epoch_reg_test {
    use super::*;
    use proptest::prelude::*;
    use std::collections::BTreeMap;

    prop_compose! {
        fn any_diff()(
            registered in
                any::<BTreeMap<u8, Vec<u8>>>(),
            unregistered in
                any::<BTreeMap<u8, Epoch>>()
        ) -> DiffEpochReg<u8, u8> {
            DiffEpochReg {
                registered: registered
                    .into_iter()
                    .filter_map(|(k, mut v)| {
                        v.truncate(2);
                        let v = Vec1::try_from(v).ok()?;
                        Some((k, v))
                    })
                    .collect::<BTreeMap<_, _>>(),
                unregistered,
            }
        }
    }

    proptest! {
        // NOTE: We could avoid this test altogether by modelling the type in a different way.
        // Having a sum One(V) | Two(V, V) instead of a Vec1 would give us this guarantee _by
        // construction_.
        #[test]
        fn prop_register(mut st in any_diff(), (k, v) in any::<(u8, u8)>()) {
            st.register(k, v);
            let vs = st.registered.get(&k).expect("we just registered an element");
            assert!(vs.len() <= 2, "registered[{k}] = {:?} has more than 2 elements", vs);
            if vs.len() == 1 {
                assert_eq!(vs, &vec1![v], "only element is different");
            } else {
                assert_eq!(*vs.last(), v, "last element is different");
            }
        }
    }

    proptest! {
        #[test]
        fn prop_register_cancels_unregister(mut st in any_diff(), (k, v) in any::<(u8, u8)>()) {
            st.register(k, v);
            assert!(!st.unregistered.contains_key(&k))
        }
    }

    proptest! {
        #[test]
        fn prop_unregister_right_biaised(mut st in any_diff(), (k, e) in any::<(u8, Epoch)>()) {
            st.unregister(k, e);
            let e_retained = st.unregistered.get(&k);
            assert_eq!(e_retained, Some(&e))
        }
    }
}

// DiffSet
// ----------------------------------------------------------------------------

/// A compact data-structure tracking changes in a DAG. A composition relation exists, allowing to reduce
/// two `DiffSet` into one that is equivalent to applying both `DiffSet` in sequence.
///
/// Concretely, we use this to track changes to apply to the UTxO set across a block, coming from
/// the processing of each transaction in sequence.
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
    pub fn merge(&mut self, other: Self) {
        self.produced.retain(|k, _| !other.consumed.contains(k));
        self.consumed.retain(|k| !other.produced.contains_key(k));
        self.consumed.extend(other.consumed);
        self.produced.extend(other.produced);
    }
}

#[cfg(test)]
mod diff_set_test {
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

    proptest! {
        #[test]
        fn prop_composition(
            st0 in any_diff().prop_map(|st| st.produced),
            diffs in prop::collection::vec(any_diff(), 1..5),
        ) {
            // NOTE: The order in which we apply transformation here doesn't matter, because we
            // know that DiffSet consumed and produced do not overlap _by construction_ (cf the
            // prop_merge_no_overlap). So we could write the two statements below in any order.
            fn apply(mut st: BTreeMap<u8, u8>, diff: &DiffSet<u8, u8>) -> BTreeMap<u8, u8> {
                for k in diff.consumed.iter() {
                    st.remove(k);
                }

                for (k, v) in diff.produced.iter() {
                    st.insert(*k, *v);
                }

                st
            }

            // Apply each diff in sequence.
            let st_seq = diffs.iter().fold(st0.clone(), apply);

            // Apply a single reduced diff
            let st_compose = apply(
                st0,
                &diffs
                    .into_iter()
                    .fold(DiffSet::default(), |mut acc, diff| {
                        acc.merge(diff);
                        acc
                    })
            );

            assert_eq!(st_seq, st_compose);
        }
    }
}

// Errors
// ----------------------------------------------------------------------------

#[derive(Debug)]
pub enum QueryErr<E> {
    StorageErr(E),
}

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
