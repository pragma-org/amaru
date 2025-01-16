pub mod diff_bind;
pub mod diff_epoch_reg;
pub mod diff_set;

use crate::ledger::{
    kernel::{
        self, block_point, epoch_from_slot, output_lovelace, Certificate, Epoch, Hash, Hasher,
        Lovelace, MintedBlock, Point, PoolId, PoolParams, PoolSigma, StakeCredential,
        TransactionInput, TransactionOutput, CONSENSUS_SECURITY_PARAM, STABILITY_WINDOW,
        STAKE_CREDENTIAL_DEPOSIT,
    },
    rewards::RewardsSummary,
    store::{self, columns::*, Store},
};
use diff_bind::DiffBind;
use diff_epoch_reg::DiffEpochReg;
use diff_set::DiffSet;
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, VecDeque},
    iter,
    sync::{Arc, Mutex},
};
use tracing::{debug, info, info_span, Span};

const EVENT_TARGET: &str = "amaru::ledger::state";

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

    /// The computed rewards summary to be applied on the next epoch boundary. This is computed
    /// once in the epoch, and held until the end where it is reset.
    rewards_summary: Option<RewardsSummary>,
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
            //     views of the volatile DB on-disk to be able to restore them quickly.
            volatile: VolatileDB::new(),

            rewards_summary: None,
        }
    }

    /// Inspect the tip of this ledger state. This corresponds to the point of the latest block
    /// applied to the ledger.
    pub fn tip(&'_ self) -> Cow<'_, Point> {
        if let Some(st) = self.volatile.view_back() {
            return Cow::Borrowed(&st.anchor.0);
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
    pub fn forward(&mut self, span: &Span, block: MintedBlock<'_>) -> Result<(), ForwardErr<E>> {
        let point = block_point(&block);
        let issuer = Hasher::<224>::hash(&block.header.header_body.issuer_vkey[..]);
        let relative_slot = kernel::relative_slot(point.slot_or_default());

        let span_apply_block = info_span!(
            target: EVENT_TARGET,
            parent: span,
            "block.body.validate",
            block.transactions.total = tracing::field::Empty,
            block.transactions.failed = tracing::field::Empty,
            block.transactions.success = tracing::field::Empty
        )
        .entered();
        let state = self.apply_block(&span_apply_block, block)?;
        span_apply_block.exit();

        if self.volatile.len() >= CONSENSUS_SECURITY_PARAM {
            let mut db = self.stable.lock().unwrap();

            let now_stable = self.volatile.pop_front().unwrap_or_else(|| {
                unreachable!("pre-condition: self.volatile.len() >= CONSENSUS_SECURITY_PARAM")
            });

            let current_epoch = epoch_from_slot(now_stable.anchor.0.slot_or_default());
            span.record("stable.epoch", current_epoch);

            // Note: the volatile sequence may contain points belonging to two epochs. We diligently
            // make snapshots at the end of each epoch. Thus, as soon as the next stable block is
            // exactly MORE than one epoch apart, it means that we've already pushed to the stable
            // db all the blocks from the previous epoch and it's time to make a snapshot before we
            // apply this new stable diff.
            //
            // However, 'current_epoch' here refers to the _ongoing_ epoch in the volatile db. So
            // we must snapshot the one _just before_.
            if current_epoch > db.most_recent_snapshot() + 1 {
                // FIXME: All operations below should technically happen in the same database
                // transaction. If we interrupt the application between any of those, we might end
                // up with a corrupted state.
                info_span!(target: EVENT_TARGET, parent: span, "snapshot", epoch = current_epoch - 1).in_scope(|| {
                    db.next_snapshot(current_epoch - 1, self.rewards_summary.take())
                        .map_err(ForwardErr::StorageErr)
                })?;

                info_span!(target: EVENT_TARGET, parent: span, "tick.pool").in_scope(|| {
                    // Then we, can tick pools to compute their new state at the epoch boundary. Notice
                    // how we tick with the _current epoch_ however, but we take the snapshot before
                    // the tick since the actions are only effective once the epoch is crossed.
                    db.with_pools(|iterator| {
                        for (_, pool) in iterator {
                            pools::Row::tick(pool, current_epoch)
                        }
                    })
                    .map_err(ForwardErr::StorageErr)
                })?;
            }

            let (stable_point, stable_issuer, fees, add, remove) = now_stable.into_store_update();

            info_span!(target: EVENT_TARGET, parent: span, "save").in_scope(|| {
                db.save(&stable_point, Some(&stable_issuer), add, remove)
                    .and_then(|()| {
                        db.with_pots(|mut row| {
                            row.borrow_mut().fees += fees;
                        })
                    })
                    .map_err(ForwardErr::StorageErr)
            })?;

            // Once we reach the stability window,
            if self.rewards_summary.is_none() && relative_slot >= STABILITY_WINDOW as u64 {
                self.rewards_summary = Some(
                    db.rewards_summary(current_epoch - 1)
                        .map_err(ForwardErr::StorageErr)?,
                );
            }
        } else {
            info!(target: EVENT_TARGET, parent: span, size = self.volatile.len(), "volatile.warming_up",);
        }

        span.record("tip.epoch", epoch_from_slot(point.slot_or_default()));
        span.record("tip.relative_slot", relative_slot);

        self.volatile.push_back(state.anchor(point, issuer));

        Ok(())
    }

    pub fn backward<'b>(&mut self, to: &'b Point) -> Result<(), BackwardErr<'b>> {
        self.volatile
            .rollback_to(to, BackwardErr::UnknownRollbackPoint(to))
    }

    /// Fetch stake pool details from the current live view of the ledger.
    pub fn get_pool(&self, pool: &PoolId) -> Result<Option<PoolParams>, QueryErr<E>> {
        let current_epoch = epoch_from_slot(self.tip().slot_or_default());

        let volatile_view = self.volatile.iter_pools();

        match diff_epoch_reg::Fold::for_epoch(current_epoch, pool, volatile_view) {
            diff_epoch_reg::Fold::Registered(pool) => Ok(Some(pool.clone())),
            diff_epoch_reg::Fold::Unregistered => Ok(None),
            diff_epoch_reg::Fold::Undetermined => {
                let db = self.stable.lock().unwrap();
                let mut row = db.pool(pool).map_err(QueryErr::StorageErr)?;
                pools::Row::tick(Box::new(&mut row), current_epoch);
                Ok(row.map(|row| row.current_params))
            }
        }
    }

    fn resolve_inputs<'a>(
        &'a self,
        ongoing_state: &'a VolatileState<()>,
        inputs: impl Iterator<Item = &'a TransactionInput>,
    ) -> Result<Vec<(&'a TransactionInput, Cow<'a, TransactionOutput>)>, E> {
        let mut result = Vec::new();

        for input in inputs {
            let output = ongoing_state
                .resolve_input(input)
                .or_else(|| self.volatile.resolve_input(input))
                .map(|o| Ok(Cow::Borrowed(o)))
                .unwrap_or_else(|| {
                    let db = self.stable.lock().unwrap();
                    db.resolve_input(input).map(|opt| {
                        Cow::Owned(opt.unwrap_or_else(|| {
                            panic!("unknown UTxO expected to be known: {input:?}!")
                        }))
                    })
                })?;

            result.push((input, output));
        }

        Ok(result)
    }

    /// Process a given block into a series of ledger-state diff (a.k.a events) to apply.
    fn apply_block(
        &self,
        span: &Span,
        block: MintedBlock<'_>,
    ) -> Result<VolatileState<()>, ForwardErr<E>> {
        let failed_transactions = FailedTransactions::from_block(&block);

        let mut state = VolatileState::default();

        let (mut count_total, mut count_failed, mut count_success) = (0, 0, 0);
        for (ix, transaction_body) in block.transaction_bodies.to_vec().into_iter().enumerate() {
            count_total += 1;
            let transaction_id = Hasher::<256>::hash(transaction_body.raw_cbor());

            let transaction_body = transaction_body.unwrap();

            let (inputs, outputs, fees) = match failed_transactions.has(ix as u32) {
                // == Successful transaction
                // - inputs are consumed;
                // - outputs are produced.
                false => {
                    count_success += 1;
                    let inputs = transaction_body.inputs.to_vec().into_iter();
                    let outputs = transaction_body.outputs.into_iter().map(|x| x.into());
                    (
                        Box::new(inputs) as Box<dyn Iterator<Item = TransactionInput>>,
                        Box::new(outputs) as Box<dyn Iterator<Item = TransactionOutput>>,
                        transaction_body.fee,
                    )
                }

                // == Failed transaction
                // - collateral inputs are consumed;
                // - collateral outputs produced (if any).
                true => {
                    count_failed += 1;
                    let inputs = transaction_body
                        .collateral
                        .map(|x| x.to_vec())
                        .unwrap_or_default();

                    let resolved_inputs = self
                        .resolve_inputs(&state, inputs.iter())
                        .map_err(ForwardErr::StorageErr)?;

                    let (outputs, collateral_return) = match transaction_body.collateral_return {
                        Some(output) => {
                            let output = output.into();
                            let collateral_return = output_lovelace(&output);
                            (
                                Box::new([output].into_iter())
                                    as Box<dyn Iterator<Item = TransactionOutput>>,
                                collateral_return,
                            )
                        }
                        None => (
                            Box::new(iter::empty()) as Box<dyn Iterator<Item = TransactionOutput>>,
                            0,
                        ),
                    };

                    let fees = resolved_inputs
                        .iter()
                        .fold(0, |total, (_, output)| total + output_lovelace(output))
                        - collateral_return;

                    (
                        Box::new(inputs.into_iter()) as Box<dyn Iterator<Item = TransactionInput>>,
                        outputs,
                        fees,
                    )
                }
            };

            let span_apply_transaction = info_span!(
                target: EVENT_TARGET,
                parent: span,
                "apply.transaction",
                transaction.id = %transaction_id,
                transaction.inputs = tracing::field::Empty,
                transaction.outputs = tracing::field::Empty,
                transaction.certificates = tracing::field::Empty,
            )
            .entered();
            apply_transaction(
                &mut state,
                &span_apply_transaction,
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
                fees,
            );
            span_apply_transaction.exit();
        }

        span.record("block.transactions.total", count_total);
        span.record("block.transactions.failed", count_failed);
        span.record("block.transactions.success", count_success);

        Ok(state)
    }
}

fn apply_transaction<T>(
    state: &mut VolatileState<T>,
    span: &Span,
    transaction_id: &Hash<32>,
    inputs: impl Iterator<Item = TransactionInput>,
    outputs: impl Iterator<Item = TransactionOutput>,
    certificates: impl Iterator<Item = Certificate>,
    fees: Lovelace,
) {
    const EVENT_TARGET: &str = "amaru::ledger::state::apply::transaction";

    // Inputs/Outputs
    {
        let consumed = BTreeSet::from_iter(inputs);
        let produced = outputs.enumerate().map(|(index, output)| (TransactionInput {
            transaction_id: *transaction_id,
            index: index as u64
        }, output)).collect::<BTreeMap<_, _>>();

        span.record("transaction.inputs", consumed.len());
        span.record("transaction.outputs", produced.len());

        state.utxo.merge(DiffSet { consumed, produced });
    }

    // Fees
    {
        state.fees += fees;
    }

    // Certificates
    {
        let mut count = 0;
        for certificate in certificates {
            count += 1;
            match certificate {
                Certificate::StakeRegistration(credential) | Certificate::Reg(credential, ..) | Certificate::VoteRegDeleg(credential, ..) => {
                    debug!(name: "certificate.stake.registration", target: EVENT_TARGET, parent: span, credential = ?credential);
                    state
                        .accounts
                        .register(credential, STAKE_CREDENTIAL_DEPOSIT as Lovelace, None);
                }
                Certificate::StakeDelegation(credential, pool)
                // FIXME: register DRep delegation
                | Certificate::StakeVoteDeleg(credential, pool, ..) => {
                    debug!(name: "certificate.stake.delegation", target: EVENT_TARGET, parent: span, credential = ?credential, pool = %pool);
                    state.accounts.bind(credential, Some(pool));
                }
                Certificate::StakeRegDeleg(credential, pool, ..)
                // FIXME: register DRep delegation
                | Certificate::StakeVoteRegDeleg(credential, pool, ..) => {
                    debug!(name: "certificate.stake.registration", target: EVENT_TARGET, parent: span, credential = ?credential);
                    debug!(name: "certificate.stake.delegation", target: EVENT_TARGET, parent: span, credential = ?credential, pool = %pool);
                    state.accounts.register(
                        credential,
                        STAKE_CREDENTIAL_DEPOSIT as Lovelace,
                        Some(pool),
                    );
                }
                Certificate::StakeDeregistration(credential)
                | Certificate::UnReg(credential, ..) => {
                    debug!(name: "certificate.stake.deregistration", target: EVENT_TARGET, parent: span, credential = ?credential);
                    state.accounts.unregister(credential);
                }
                Certificate::PoolRetirement(id, epoch) => {
                    debug!(name: "certificate.pool.retirement", target: EVENT_TARGET, parent: span, pool = %id, epoch = %epoch);
                    state.pools.unregister(id, epoch)
                }
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
                } => {
                    let params = PoolParams {
                        id,
                        vrf,
                        pledge,
                        cost,
                        margin,
                        reward_account,
                        owners,
                        relays,
                        metadata,
                    };
                    debug!(
                        name: "certificate.pool.registration",
                        target: EVENT_TARGET,
                        parent: span,
                        pool = %id,
                        params = ?params,
                    );

                    state.pools.register(id, params)
                }
                // FIXME: Process other types of certificates
                _ => {}
            }
        }
        span.record("transaction.certificates", count);
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

    fn latest_opcert_sequence_number(&self, _issuer_vkey: &Hash<28>) -> Option<u64> {
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

// FIXME: Currently, the cache owns data that are also available in the sequence. We could
// potentially avoid cloning and re-allocation altogether by sharing an allocator and having them
// both reference from within that allocator (e.g. an arena allocator like bumpalo)
//
// Ideally, we would just have the struct be self-referenced, but that isn't possible in Rust and
// we cannot introduce a lifetime to the VolatileDB (which would bubble up to the State).
//
// Another option is to have the cache not own data, but indices onto the sequence. This may
// require to switch the sequence back to a Vec to allow fast random lookups.
struct VolatileDB {
    cache: VolatileCache,
    sequence: VecDeque<VolatileState<(Point, PoolId)>>,
}

impl VolatileDB {
    pub fn new() -> Self {
        VolatileDB {
            cache: VolatileCache::default(),
            sequence: VecDeque::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.sequence.len()
    }

    pub fn view_back(&self) -> Option<&VolatileState<(Point, PoolId)>> {
        self.sequence.back()
    }

    pub fn resolve_input(&self, input: &TransactionInput) -> Option<&TransactionOutput> {
        self.cache.utxo.produced.get(input)
    }

    pub fn iter_pools(&self) -> impl Iterator<Item = (Epoch, &DiffEpochReg<PoolId, PoolParams>)> {
        self.sequence
            .iter()
            .map(|st| (epoch_from_slot(st.anchor.0.slot_or_default()), &st.pools))
    }

    pub fn pop_front(&mut self) -> Option<VolatileState<(Point, PoolId)>> {
        self.sequence.pop_front().inspect(|state| {
            // NOTE: It is imperative to remove consumed and produced UTxOs from the cache as we
            // remove them from the sequence to prevent the cache from growing out of proportion.
            for k in state.utxo.consumed.iter() {
                self.cache.utxo.consumed.remove(k);
            }

            for (k, _) in state.utxo.produced.iter() {
                self.cache.utxo.produced.remove(k);
            }
        })
    }

    pub fn push_back(&mut self, state: VolatileState<(Point, PoolId)>) {
        // TODO: See NOTE on VolatileDB regarding the .clone()
        self.cache.merge::<Point>(state.utxo.clone());
        self.sequence.push_back(state);
    }

    pub fn rollback_to<E>(&mut self, point: &Point, on_unknown_point: E) -> Result<(), E> {
        self.cache = VolatileCache::default();

        let mut ix = 0;
        for diff in self.sequence.iter() {
            if diff.anchor.0.slot_or_default() <= point.slot_or_default() {
                // TODO: See NOTE on VolatileDB regarding the .clone()
                self.cache.merge::<Point>(diff.utxo.clone());
                ix += 1;
            }
        }

        if ix >= self.sequence.len() {
            Err(on_unknown_point)
        } else {
            self.sequence.resize_with(ix, || {
                unreachable!("ix is necessarly strictly smaller than the length")
            });
            Ok(())
        }
    }
}

// TODO: At this point, we only need to lookup UTxOs, so the aggregated cache is limited to those.
// It would be relatively easy to extend to accounts, but it is trickier for pools since
// DiffEpochReg aren't meant to be mergeable across epochs.
#[derive(Default)]
pub struct VolatileCache {
    pub utxo: DiffSet<TransactionInput, TransactionOutput>,
}

impl VolatileCache {
    pub fn merge<T>(&mut self, utxo: DiffSet<TransactionInput, TransactionOutput>) {
        self.utxo.merge(utxo);
    }
}

pub struct VolatileState<A> {
    pub anchor: A,
    pub utxo: DiffSet<TransactionInput, TransactionOutput>,
    pub pools: DiffEpochReg<PoolId, PoolParams>,
    pub accounts: DiffBind<StakeCredential, PoolId, Lovelace>,
    pub fees: Lovelace,
}

impl Default for VolatileState<()> {
    fn default() -> Self {
        Self {
            anchor: (),
            utxo: Default::default(),
            pools: Default::default(),
            accounts: Default::default(),
            fees: 0,
        }
    }
}

impl VolatileState<()> {
    pub fn anchor(self, point: Point, issuer: PoolId) -> VolatileState<(Point, PoolId)> {
        VolatileState {
            anchor: (point, issuer),
            utxo: self.utxo,
            pools: self.pools,
            accounts: self.accounts,
            fees: self.fees,
        }
    }

    pub fn resolve_input(&self, input: &TransactionInput) -> Option<&TransactionOutput> {
        self.utxo.produced.get(input)
    }
}

impl VolatileState<(Point, PoolId)> {
    pub fn into_store_update(
        self,
    ) -> (
        Point,
        PoolId,
        Lovelace,
        store::Columns<
            impl Iterator<Item = utxo::Add>,
            impl Iterator<Item = pools::Add>,
            impl Iterator<Item = accounts::Add>,
        >,
        store::Columns<
            impl Iterator<Item = utxo::Remove>,
            impl Iterator<Item = pools::Remove>,
            impl Iterator<Item = accounts::Remove>,
        >,
    ) {
        let epoch = epoch_from_slot(self.anchor.0.slot_or_default());

        (
            self.anchor.0,
            self.anchor.1,
            self.fees,
            store::Columns {
                utxo: self.utxo.produced.into_iter(),
                pools: self
                    .pools
                    .registered
                    .into_iter()
                    .flat_map(move |(_, registrations)| {
                        registrations
                            .into_iter()
                            // NOTE/TODO: Re-registrations (a.k.a pool params updates) are always
                            // happening on the following epoch. We do not explicitly store epochs
                            // for registrations in the DiffEpochReg (which may be an arguable
                            // choice?) so we have to artificially set it here. Note that for
                            // registrations (when there's no existing entry), the epoch is wrong
                            // but it is fully ignored. It's slightly ugly, but we cannot know if
                            // an entry exists without querying the stable store -- and frankly, we
                            // don't _have to_.
                            .map(|pool| (pool, epoch + 1))
                            .collect::<Vec<_>>()
                    }),
                accounts: self
                    .accounts
                    .registered
                    .into_iter()
                    .map(|(credential, (pool, deposit))| (credential, pool, deposit, 0)),
            },
            store::Columns {
                utxo: self.utxo.consumed.into_iter(),
                pools: self.pools.unregistered.into_iter(),
                accounts: self.accounts.unregistered.into_iter(),
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
