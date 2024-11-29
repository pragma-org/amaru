pub mod diff_epoch_reg;
pub mod diff_set;

use diff_epoch_reg::DiffEpochReg;
use diff_set::DiffSet;

use super::{
    kernel::{
        block_point, epoch_from_slot, relative_slot, Certificate, Epoch, Hash, Hasher, MintedBlock,
        Point, PoolId, PoolParams, PoolSigma, TransactionInput, TransactionOutput,
        CONSENSUS_SECURITY_PARAM,
    },
    store::{self, columns::pools, Store},
};

use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex},
};
use tracing::{info, info_span};

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
pub struct State<S, E>
where
    S: Store<Error = E>,
{
    /// A handle to the stable store, shared across all ledger instances.
    stable: Arc<Mutex<S>>,

    /// Our own in-memory vector of volatile deltas to apply onto the stable store in due time.
    volatile: VolatileDB,
}

impl<S: Store<Error = E>, E: std::fmt::Debug> State<S, E> {
    pub fn new(stable: Arc<Mutex<S>>) -> Self {
        Self {
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

    /// Inspect the tip of this ledger state. This corresponds to the point of the latest block
    /// applied to the ledger.
    pub fn tip(&'_ self) -> Cow<'_, Point> {
        if let Some(st) = self.volatile.back() {
            return Cow::Borrowed(&st.point);
        }

        Cow::Owned(
            self.stable
                .lock()
                .unwrap()
                .tip()
                .unwrap_or_else(|e| panic!("no tip found in stable db: {e:?}")),
        )
    }

    /// Roll the ledger forward with the given block by applying transactions one by one, in
    /// sequence. The update stops at the first invalid transaction, if any. Otherwise, it updates
    /// the internal state of the ledger.
    pub fn forward(&mut self, block: MintedBlock<'_>) -> Result<(), ForwardErr<E>> {
        let point = block_point(&block);

        let state = apply_block(block);

        if self.volatile.len() >= CONSENSUS_SECURITY_PARAM {
            let mut db = self.stable.lock().unwrap();

            let now_stable = self.volatile.pop_front().unwrap();
            let current_epoch = epoch_from_slot(now_stable.point.slot_or_default());

            if current_epoch > db.most_recent_snapshot().unwrap_or_default() {
                let span_snapshot = info_span!("snapshot", epoch = current_epoch).entered();
                db.with_pools(|iterator| {
                    for pool in iterator {
                        tick_pool(pool, current_epoch)
                    }
                })
                .map_err(ForwardErr::StorageErr)?;
                db.next_snapshot(current_epoch)
                    .map_err(ForwardErr::StorageErr)?;
                span_snapshot.exit();
            }

            let (add, remove) = now_stable.into_store_update();
            db.save(&point, add, remove)
                .map_err(ForwardErr::StorageErr)?;
        } else {
            info!(num_deltas = self.volatile.len(), "warming up volatile db",);
        }

        info!(
            target: "amaru::ledger::tip",
            epoch = epoch_from_slot(point.slot_or_default()),
            relative_slot = relative_slot(point.slot_or_default()),
        );

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
        let current_epoch = epoch_from_slot(self.tip().slot_or_default());

        // NOTE: When we are near an epoch boundary, we need to consider the not-yet-stable changes
        // that belong to the previous epoch. Any re-registration or retirement that would now be
        // valid must be acknowledged.
        let volatile = self
            .volatile
            .iter()
            .fold(DiffEpochReg::default(), |mut state, step| {
                if epoch_from_slot(step.point.slot_or_default()) < current_epoch {
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

        let db = self.stable.lock().unwrap();

        if let Some(state) = db.pool(pool).map_err(QueryErr::StorageErr)? {
            // NOTE: Similarly, the stable DB is at most k blocks in the past. So, if a certificate
            // is submitted near the end (i.e. within k blocks) of the last epoch, then we could be
            // in a situation where we haven't yet processed the registrations (since they're
            // processed with a delay of k blocks) but have already moved into the next epoch.
            if let Some(params) = state
                .future_params
                .iter()
                .filter(|(_, epoch)| epoch <= &current_epoch)
                .last()
            {
                return Ok(params.0.clone());
            }

            return Ok(Some(state.current_params));
        }

        Ok(None)
    }
}

/// Alter a Pool object by applying updates recorded across the epoch. A pool can have two types of
/// updates:
///
/// 1. Re-registration (effectively adjusting its underlying parameters), which always take effect
///    on the next epoch boundary.
///
/// 2. Retirements, which specifies an epoch where the retirement becomes effective.
///
/// While we collect all updates as they arrive from blocks, a few rules apply:
///
/// a. Any re-registration that comes after a retirement cancels that retirement.
/// b. Any retirement that come after a retirement cancels that initial retirement.
fn tick_pool<'a>(
    mut row: Box<dyn std::borrow::BorrowMut<Option<pools::Row>> + 'a>,
    current_epoch: Epoch,
) {
    let (update, retirement, needs_update) = match row.borrow().as_ref() {
        None => (None, None, false),
        Some(pool) => pool.future_params.iter().fold(
            (None, None, false),
            |(update, retirement, needs_update), (params, epoch)| match params {
                Some(params) if epoch <= &current_epoch => (Some(params), None, true),
                None => (
                    update,
                    Some(*epoch),
                    needs_update || epoch <= &current_epoch,
                ),
                Some(..) => (update, retirement, needs_update),
            },
        ),
    };

    if needs_update {
        // This drops the immutable borrow. We avoid cloning inside the fold because we only ever need
        // to clone the last update. Yet we can't hold onto a reference because we must acquire a
        // mutable borrow below.
        let update: Option<PoolParams> = update.cloned();

        let pool: &mut Option<pools::Row> = row.borrow_mut();

        // If the most recent retirement is effective as per the current epoch, we simply drop the
        // entry. Note that, any re-registration happening after that retirement would cancel it,
        // which is taken care of in the fold above (returning 'None').
        if let Some(epoch) = retirement {
            if epoch <= current_epoch {
                *pool = None;
                return;
            }
        }

        // Unwrap is safe here because we know the entry exists. Otherwise we wouldn't have got an
        // update to begin with!
        let pool = pool.as_mut().unwrap();

        if let Some(new_params) = update {
            pool.current_params = new_params;
        }

        // Regardless, always prune future params from those that are now-obsolete.
        pool.future_params
            .retain(|(_, epoch)| epoch > &current_epoch);
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

// LedgerState
// ----------------------------------------------------------------------------

// The 'LedgerState' trait materializes the interface required of the consensus layer in order to
// validate block headers. It allows to keep the ledger implementation rather abstract to the
// consensus in order to decouple both components.
impl<S: Store<Error = E> + Sync + Send, E: std::fmt::Debug> ouroboros::ledger::LedgerState
    for State<S, E>
{
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
    pub fn into_store_update(
        self,
    ) -> (
        store::Columns<
            impl Iterator<Item = (TransactionInput, TransactionOutput)>,
            impl Iterator<Item = (PoolParams, Epoch)>,
        >,
        store::Columns<
            impl Iterator<Item = TransactionInput>,
            impl Iterator<Item = (PoolId, Epoch)>,
        >,
    ) {
        let epoch = epoch_from_slot(self.point.slot_or_default());

        info!(
            utxo_produced = self.utxo.produced.len(),
            utxo_consumed = self.utxo.produced.len(),
            pools_registered = self.pools.registered.len(),
            pools_retired = self.pools.unregistered.len(),
            "updating stable db",
        );

        (
            store::Columns {
                utxo: self.utxo.produced.into_iter(),
                pools: self
                    .pools
                    .registered
                    .into_iter()
                    .flat_map(move |(_, registrations)| {
                        registrations
                            .into_iter()
                            .map(|r| (r, epoch))
                            .collect::<Vec<_>>()
                    }),
            },
            store::Columns {
                utxo: self.utxo.consumed.into_iter(),
                pools: self.pools.unregistered.into_iter(),
            },
        )
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
