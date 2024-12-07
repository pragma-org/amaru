pub mod diff_bind;
pub mod diff_epoch_reg;
pub mod diff_set;

use crate::ledger::{
    kernel::{
        block_point, epoch_from_slot, relative_slot, Certificate, Hash, Hasher, Lovelace,
        MintedBlock, Point, PoolId, PoolParams, PoolSigma, StakeCredential, TransactionInput,
        TransactionOutput, CONSENSUS_SECURITY_PARAM, STAKE_CREDENTIAL_DEPOSIT,
    },
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
use tracing::{debug, info, info_span};

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

            let now_stable = self.volatile.pop_front().unwrap_or_else(|| {
                unreachable!("pre-condition: self.volatile.len() >= CONSENSUS_SECURITY_PARAM")
            });
            let current_epoch = epoch_from_slot(now_stable.point.slot_or_default());

            // Note: the volatile sequence may contain points belonging to two epochs. We diligently
            // make snapshots at the end of each epoch. Thus, as soon as the next stable block is
            // exactly MORE than one epoch apart, it means that we've already pushed to the stable
            // db all the blocks from the previous epoch and it's time to make a snapshot before we
            // apply this new stable diff.
            //
            // However, 'current_epoch' here refers to the _ongoing_ epoch in the volatile db. So
            // we must snapshot the one _just before_.
            if current_epoch > db.most_recent_snapshot() + 1 {
                let span_snapshot = info_span!("snapshot", epoch = current_epoch - 1).entered();
                db.next_snapshot(current_epoch - 1)
                    .map_err(ForwardErr::StorageErr)?;
                span_snapshot.exit();

                let span_tick_pool = info_span!("tick_pool", epoch = current_epoch).entered();
                info!(epoch = current_epoch, "tick_pools");
                // Then we, can tick pools to compute their new state at the epoch boundary. Notice
                // how we tick with the _current epoch_ however, but we take the snapshot before
                // the tick since the actions are only effective once the epoch is crossed.
                db.with_pools(|iterator| {
                    for (_, pool) in iterator {
                        pools::Row::tick(pool, current_epoch)
                    }
                })
                .map_err(ForwardErr::StorageErr)?;
                span_tick_pool.exit();
            }

            let (add, remove) = now_stable.into_store_update();

            let span_save = info_span!("save").entered();
            db.save(&point, add, remove)
                .map_err(ForwardErr::StorageErr)?;
            span_save.exit();
        } else {
            info!(num_deltas = self.volatile.len(), "warming up volatile db",);
        }

        info!(
            epoch = epoch_from_slot(point.slot_or_default()),
            relative_slot = relative_slot(point.slot_or_default()),
            "tip"
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

        let volatile_view = self
            .volatile
            .iter()
            .map(|st| (epoch_from_slot(st.point.slot_or_default()), &st.pools));

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
                    None => Box::new(iter::empty()) as Box<dyn Iterator<Item = TransactionOutput>>,
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
                Certificate::StakeRegistration(credential) | Certificate::Reg(credential, ..) => {
                    debug!(?credential, "certificate.stake.registration");
                    state
                        .accounts
                        .register(credential, STAKE_CREDENTIAL_DEPOSIT as Lovelace, None);
                }
                Certificate::StakeDelegation(credential, pool)
                // FIXME: register DRep delegation
                | Certificate::StakeVoteDeleg(credential, pool, ..) => {
                    debug!(?credential, ?pool, "certificate.stake.delegation");
                    state.accounts.bind(credential, Some(pool));
                }
                Certificate::StakeRegDeleg(credential, pool, ..)
                // FIXME: register DRep delegation
                | Certificate::StakeVoteRegDeleg(credential, pool, ..) => {
                    debug!(?credential, "certificate.stake.registration");
                    debug!(?credential, ?pool, "certificate.stake.delegation");
                    state.accounts.register(
                        credential,
                        STAKE_CREDENTIAL_DEPOSIT as Lovelace,
                        Some(pool),
                    );
                }
                Certificate::StakeDeregistration(credential)
                | Certificate::UnReg(credential, ..) => {
                    debug!(?credential, "certificate.stake.deregistration");
                    state.accounts.unregister(credential);
                }
                Certificate::PoolRetirement(id, epoch) => {
                    debug!(pool = ?id, ?epoch, "certificate.pool.retirement");
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

                    info!(
                        pool = ?id,
                        ?params,
                        "certificate=registration"
                    );

                    state.pools.register(id, params)
                }
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
    pub accounts: DiffBind<StakeCredential, PoolId, Lovelace>,
}

impl Default for VolatileState<()> {
    fn default() -> Self {
        Self {
            point: (),
            utxo: Default::default(),
            pools: Default::default(),
            accounts: Default::default(),
        }
    }
}

impl VolatileState<()> {
    pub fn anchor(self, point: Point) -> VolatileState<Point> {
        VolatileState {
            point,
            utxo: self.utxo,
            pools: self.pools,
            accounts: self.accounts,
        }
    }
}

impl VolatileState<Point> {
    pub fn into_store_update(
        self,
    ) -> (
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
        let epoch = epoch_from_slot(self.point.slot_or_default());

        debug!(
            utxo.produced = self.utxo.produced.len(),
            utxo.consumed = self.utxo.produced.len(),
            pools.registered = self.pools.registered.len(),
            pools.retired = self.pools.unregistered.len(),
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
