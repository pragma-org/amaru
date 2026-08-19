// Copyright 2026 PRAGMA
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::{
    borrow::BorrowMut,
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt::{Debug, Display},
    str::FromStr,
    sync::{Arc, Mutex},
};

use amaru_kernel::{
    Anchor, Block, BlockHeight, CertificatePointer, Constitution, ConstitutionalCommitteeStatus, Epoch, EraHistory,
    GlobalParameters, Hash, Header, HeaderHash, MaxString128, NetworkName, PREPROD_DEFAULT_PROTOCOL_PARAMETERS,
    PREPROD_ERA_HISTORY, PREPROD_GLOBAL_PARAMETERS, Point, Pots, ProposalId, ProposalsRoots, ProtocolParameters, Slot,
    cardano::network_block::make_block, cbor, make_header, to_cbor,
};
use amaru_ledger::{
    epoch_transition::GovernanceActivity,
    rules::block::BlockValidation,
    state::{ForkSwitchOutcome, State, volatile::VolatileFragment},
    store::{
        Columns, EpochTransitionProgress, HistoricalStores, ReadStore, Store, StoreError, TransactionalContext,
        columns::{accounts, cc_members, dreps, pools, pots, proposals, recently_unregistered_accounts, utxo, votes},
    },
};
use amaru_plutus::arena_pool::ArenaPool;
use amaru_stores::rocksdb::{RocksDB, RocksDBHistoricalStores, RocksDbConfig};
use anyhow::anyhow;

#[expect(clippy::wildcard_enum_match_arm)]
#[expect(clippy::panic)]
pub fn roll_forward(state: &mut State<MockStore, RocksDBHistoricalStores>, block: &Block) {
    match state.roll_forward(block, &ArenaPool::new(1024, 0)) {
        BlockValidation::Valid(_) => (),
        other => panic!("block was not applied: {other:?}"),
    }
}

pub fn rollback_to<S, HS>(state: &mut State<S, HS>, point: &Point) -> Result<(), anyhow::Error>
where
    S: Store,
    HS: HistoricalStores + Send + Sync + 'static,
{
    match state.switch_to_fork(point, std::iter::once(empty_block_at(0)), &ArenaPool::new(1024, 0))? {
        ForkSwitchOutcome::Completed { .. } => Ok(()),
        outcome @ ForkSwitchOutcome::Partial { .. } | outcome @ ForkSwitchOutcome::Failed { .. } => {
            Err(anyhow!("unexpected fork switch outcome: {outcome:?}"))
        }
    }
}

#[allow(clippy::panic)]
pub fn assert_no_rollback_to<S, HS, E>(state: &mut State<S, HS>, point: &Point, assert: impl FnOnce(&E))
where
    S: Store,
    HS: HistoricalStores + Send + Sync + 'static,
    E: Display + Debug + Send + Sync + 'static,
{
    assert_invalid_switch_to_fork_from(state, point, std::iter::once(invalid_block_at(0)), assert)
}

#[allow(clippy::panic)]
pub fn assert_invalid_switch_to_fork_from<I, S, HS, E>(
    state: &mut State<S, HS>,
    fork_point: &Point,
    blocks: I,
    assert: impl FnOnce(&E),
) where
    S: Store,
    HS: HistoricalStores + Send + Sync + 'static,
    E: Display + Debug + Send + Sync + 'static,
    I: IntoIterator<Item = Block>,
    I::IntoIter: ExactSizeIterator,
{
    let err = match state.switch_to_fork(fork_point, blocks, &ArenaPool::new(1024, 0)) {
        Ok(outcome) => panic!("expected switching to fork at {fork_point:?} to fail but got: {outcome:?}"),
        Err(err) => err,
    };

    assert(
        err.downcast_ref::<E>()
            .unwrap_or_else(|| panic!("switch failed but returned a different error than expected: {err:#}")),
    );
}

/// Create an initial ledger state
pub fn make_state() -> State<MockStore, RocksDBHistoricalStores> {
    make_state_in_epoch(Epoch::default()).0
}

/// Create an initial ledger state anchored to a given epoch
pub fn make_state_in_epoch(epoch: Epoch) -> (State<MockStore, RocksDBHistoricalStores>, Arc<Mutex<Vec<Point>>>) {
    make_state_in_epoch_with_snapshots(epoch, &[epoch])
}

/// Create an initial ledger state anchored to a given epoch + snapshots for the given epochs.
pub fn make_state_in_epoch_with_snapshots(
    epoch: Epoch,
    snapshot_epochs: &[Epoch],
) -> (State<MockStore, RocksDBHistoricalStores>, Arc<Mutex<Vec<Point>>>) {
    make_state_in_epoch_with_snapshots_and_store(epoch, snapshot_epochs, MockStore::new())
}

/// Create an initial ledger state anchored to a given epoch, and a given store
pub fn make_state_in_epoch_with_store(
    epoch: Epoch,
    mock_store: MockStore,
) -> (State<MockStore, RocksDBHistoricalStores>, Arc<Mutex<Vec<Point>>>) {
    make_state_in_epoch_with_snapshots_and_store(epoch, &[epoch], mock_store)
}

#[expect(clippy::expect_used)]
pub fn make_state_in_epoch_with_snapshots_and_store(
    epoch: Epoch,
    snapshot_epochs: &[Epoch],
    mock_store: MockStore,
) -> (State<MockStore, RocksDBHistoricalStores>, Arc<Mutex<Vec<Point>>>) {
    let network = NetworkName::Preprod;
    let era_history: EraHistory = PREPROD_ERA_HISTORY.clone();
    let global_parameters: GlobalParameters = PREPROD_GLOBAL_PARAMETERS.clone();
    let protocol_parameters: ProtocolParameters = PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone();
    let stable = mock_store.stable.clone();

    // Seed the rows that snapshot readers bail on when absent: the stake summary reads the
    // protocol parameters and the ratification context reads the constitutional committee.
    mock_store
        .db
        .with_transaction(|tx| {
            tx.set_protocol_parameters(&PREPROD_DEFAULT_PROTOCOL_PARAMETERS)?;
            tx.update_constitutional_committee(
                &ConstitutionalCommitteeStatus::NoConfidence,
                &BTreeMap::new(),
                &BTreeSet::new(),
            )
        })
        .expect("seeding the store succeeds");
    // Initialize the snapshots
    for snapshot_epoch in snapshot_epochs {
        mock_store.db.next_snapshot(*snapshot_epoch).expect("snapshot creation succeeds");
    }
    let snapshots = RocksDBHistoricalStores::new(&mock_store.cfg, 0);
    assert_eq!(
        snapshots.snapshots().expect("listing snapshots succeeds"),
        snapshot_epochs.to_vec(),
        "the seeded snapshots must be visible to the historical store"
    );

    let state = State::new_with(
        mock_store,
        snapshots,
        epoch,
        network,
        era_history,
        global_parameters,
        protocol_parameters,
        GovernanceActivity::default(),
        None,
        VecDeque::new(),
    );
    (state, stable)
}

/// Create a ledger state whose tip has just crossed an epoch boundary
pub fn make_state_across_epoch_boundary() -> StateAcrossEpochBoundary {
    let first_slot = 69_200_000; // inside epoch 163
    let epoch = epoch_of(first_slot);
    let mut boundary_slot = first_slot + 1;
    while epoch_of(boundary_slot) == epoch {
        boundary_slot += 1;
    }

    // The transition from e to e + 1 needs two ledger snapshots:
    //  - The rewards stake distribution comes from e−3
    //  - The block issuers and pots from e−1
    //
    // (note that epoch - 2 is not strictly necessary for the transition. It's included here for
    // completeness).
    let (mut state, stable) = make_state_in_epoch_with_snapshots(epoch, &[epoch - 3, epoch - 2, epoch - 1]);

    // Cross the boundary with roll forwards. The first applied block starts the background rewards
    // computation, the block at the boundary joins it and runs the epoch transition, making the
    // rewards effective:
    //
    //             epoch e            ╎         epoch e+1
    //     -------------------------- ╎ --------------------------
    //     fragment ── block1         ╎ block2
    forward_to(&mut state, point(boundary_slot - 3));
    let block1 = empty_block_at(boundary_slot - 2);
    let block2 = empty_block_at(boundary_slot);
    roll_forward(&mut state, &block1);
    roll_forward(&mut state, &block2);
    assert_eq!(*state.tip(), block2.point());

    StateAcrossEpochBoundary { state, stable, boundary_slot, block_before: block1, block_after: block2 }
}

pub struct StateAcrossEpochBoundary {
    // The ledger state
    pub state: State<MockStore, RocksDBHistoricalStores>,
    // A model of the stable writes that were persisted after flushing the volatile window
    pub stable: Arc<Mutex<Vec<Point>>>,
    // The slot at which the epoch boundary was crossed
    pub boundary_slot: u64,
    // The block right below the boundary
    pub block_before: Block,
    // The block at the boundary
    pub block_after: Block,
}

/// An empty block carrying the given header. It has no transactions but is a valid block for the
/// ledger
#[expect(clippy::expect_used)]
pub fn empty_block(header: Header) -> Block {
    let mut block = make_block();
    block.header = header;
    block.transaction_bodies.clear();
    block.transaction_witnesses.clear();
    block.auxiliary_data.clear();

    // NOTE: `invalid_transactions` is deliberately left as the fixture has it, an empty set. The
    // derived encoder sizes the block array from the highest field that is not `None`, so setting it
    // to `None` emits four fields and `Block`'s decoder rejects the array.

    // The size and hash caches are `#[cbor(skip)]` and only populated on decode, so round-trip once
    // to recompute them over the stripped body, announce them in the header, then round-trip again
    // so the header hash follows.
    let mut block: Block = cbor::decode(to_cbor(&block).as_slice()).expect("stripped block should round-trip");
    block.header.body_mut().block_body_size = block.body_len();
    block.header.body_mut().block_body_hash = block.body_hash();
    cbor::decode(to_cbor(&block).as_slice()).expect("stripped block should round-trip")
}

pub fn empty_block_at(slot: u64) -> Block {
    empty_block(make_header(slot, slot, None))
}

/// Forward the ledger to a given point
#[expect(clippy::expect_used)]
pub fn forward_to(state: &mut State<MockStore, RocksDBHistoricalStores>, point: Point) {
    let issuer = Hash::new([0u8; 28]);
    state.push_fragment(VolatileFragment::default().anchor(point, issuer)).expect("forward");
}

pub fn point(slot: u64) -> Point {
    point_with_hash(slot, Hash::new([slot as u8; 32]))
}

pub fn point_with_hash(slot: u64, hash: HeaderHash) -> Point {
    Point::Specific(Slot::from(slot), hash, BlockHeight::from(slot))
}

#[expect(clippy::unwrap_used)]
pub fn epoch_of(slot: u64) -> Epoch {
    let era_history: EraHistory = PREPROD_ERA_HISTORY.clone();
    era_history.slot_to_epoch_unchecked_horizon(Slot::from(slot)).unwrap()
}

/// A block whose announced body hash does not match its body, so it fails validation.
#[expect(clippy::expect_used)]
pub fn invalid_block_at(slot: u64) -> Block {
    let mut block = empty_block_at(slot);
    block.header.body_mut().block_body_hash = Hash::new([0xFF; 32]);
    let block: Block = cbor::decode(to_cbor(&block).as_slice()).expect("tampered block should round-trip");
    block
}

pub struct MockStore {
    pub cfg: RocksDbConfig,

    pub db: RocksDB,
    /// The blocks that were persisted after flushing the volatile window.
    pub stable: Arc<Mutex<Vec<Point>>>,
    /// The persisted epoch-transition progress marker, shared with the transactions.
    progress: Arc<Mutex<Option<EpochTransitionProgress>>>,

    fail_transition_progress: bool,
}

impl Default for MockStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MockStore {
    #[expect(clippy::expect_used)]
    pub fn new() -> Self {
        let ledger_dir = tempfile::tempdir().expect("tempdir creation succeeds").keep();
        let cfg = RocksDbConfig::new(ledger_dir);
        let db = RocksDB::empty(&cfg).expect("RocksDB::empty succeeds");
        let stable = Arc::new(Mutex::new(Vec::new()));
        Self { cfg, db, stable, progress: Arc::new(Mutex::new(None)), fail_transition_progress: false }
    }

    pub fn failing_transition_progress() -> Self {
        let mut mock_store = MockStore::new();
        mock_store.fail_transition_progress = true;
        mock_store
    }
}

impl ReadStore for MockStore {
    /// The tip of the stable store: the last point flushed out of the volatile window.
    #[expect(clippy::unwrap_used)]
    fn tip(&self) -> amaru_ledger::store::Result<Point> {
        Ok(self.stable.lock().unwrap().last().copied().unwrap_or(Point::Origin))
    }

    fn proposals_roots(&self) -> amaru_ledger::store::Result<ProposalsRoots> {
        Ok(ProposalsRoots::default())
    }

    fn pots(&self) -> amaru_ledger::store::Result<Pots> {
        Ok(Pots::default())
    }

    #[expect(clippy::unwrap_used)]
    fn epoch_transition_progress(&self) -> amaru_ledger::store::Result<Option<EpochTransitionProgress>> {
        if self.fail_transition_progress {
            Err(StoreError::Internal(anyhow!("failed transition progress!").into()))
        } else {
            Ok(*self.progress.lock().unwrap())
        }
    }

    fn iter_pools(&self) -> amaru_ledger::store::Result<impl Iterator<Item = (pools::Key, pools::Row)>> {
        Ok(std::iter::empty())
    }

    fn iter_recently_unregistered_accounts(
        &self,
    ) -> amaru_ledger::store::Result<impl Iterator<Item = recently_unregistered_accounts::Key>> {
        Ok(std::iter::empty())
    }

    fn iter_proposals(&self) -> amaru_ledger::store::Result<impl Iterator<Item = (proposals::Key, proposals::Row)>> {
        Ok(std::iter::empty())
    }

    fn governance_activity(&self) -> amaru_ledger::store::Result<GovernanceActivity> {
        Ok(GovernanceActivity::default())
    }
}

impl Store for MockStore {
    type Transaction<'a> = MockTransaction<'a>;

    fn next_snapshot(&self, epoch: Epoch) -> amaru_ledger::store::Result<()> {
        self.db.next_snapshot(epoch)
    }

    fn create_transaction(&self) -> MockTransaction<'_> {
        MockTransaction { flushed: &self.stable, progress: &self.progress }
    }
}

/// Records the points written to the stable store instead of writing them, which keeps the test on
/// the ledger's flush behaviour rather than RocksDB's (covered by its own tests).
pub struct MockTransaction<'a> {
    flushed: &'a Mutex<Vec<Point>>,
    progress: &'a Mutex<Option<EpochTransitionProgress>>,
}

impl ReadStore for MockTransaction<'_> {
    fn governance_activity(&self) -> amaru_ledger::store::Result<GovernanceActivity> {
        Ok(GovernanceActivity::default())
    }

    /// A bootstrapped ledger always has a constitution; these tests never propose one, so any
    /// anchor will do and there is no guardrails script to enforce.
    #[expect(clippy::expect_used)]
    fn constitution(&self) -> amaru_ledger::store::Result<Constitution> {
        Ok(Constitution {
            anchor: Anchor {
                url: MaxString128::from_str("https://example.com").expect("valid anchor URL"),
                content_hash: [0; 32].into(),
            },
            guardrail_script: None,
        })
    }
}

impl<'a> TransactionalContext<'a> for MockTransaction<'a> {
    fn commit(self) -> amaru_ledger::store::Result<()> {
        Ok(())
    }

    #[expect(clippy::unwrap_used)]
    fn reset_epoch_transition_progress(&self) -> amaru_ledger::store::Result<()> {
        *self.progress.lock().unwrap() = None;
        Ok(())
    }

    #[expect(clippy::unwrap_used)]
    fn try_epoch_transition(
        &self,
        from: Option<EpochTransitionProgress>,
        to: Option<EpochTransitionProgress>,
    ) -> amaru_ledger::store::Result<bool> {
        let mut progress = self.progress.lock().unwrap();
        if *progress == from {
            *progress = to;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn set_recently_pruned_proposals<'iter>(
        &self,
        _proposals: impl IntoIterator<Item = (&'iter amaru_kernel::ProposalId, amaru_kernel::RatificationStatus)>,
    ) -> amaru_ledger::store::Result<()> {
        Ok(())
    }

    fn prune_recently_unregistered_accounts(&self, _epoch: Epoch) -> amaru_ledger::store::Result<()> {
        Ok(())
    }

    fn set_proposals_roots(&self, _roots: &ProposalsRoots) -> amaru_ledger::store::Result<()> {
        Ok(())
    }

    fn set_protocol_parameters(&self, _protocol_parameters: &ProtocolParameters) -> amaru_ledger::store::Result<()> {
        Ok(())
    }

    fn set_governance_activity(&self, _dormant_epochs: GovernanceActivity) -> amaru_ledger::store::Result<()> {
        Ok(())
    }

    fn with_accounts(&self, _with: impl FnMut(accounts::Iter<'_, '_>)) -> amaru_ledger::store::Result<()> {
        Ok(())
    }

    fn with_pools(&self, _with: impl FnMut(pools::Iter<'_, '_>)) -> amaru_ledger::store::Result<()> {
        Ok(())
    }

    fn with_utxo(&self, _with: impl FnMut(utxo::Iter<'_, '_>)) -> amaru_ledger::store::Result<()> {
        Ok(())
    }

    fn with_dreps(&self, _with: impl FnMut(dreps::Iter<'_, '_>)) -> amaru_ledger::store::Result<()> {
        Ok(())
    }

    fn with_cc_members(&self, _with: impl FnMut(cc_members::Iter<'_, '_>)) -> amaru_ledger::store::Result<()> {
        Ok(())
    }

    fn remove_proposals<T>(&self, _proposals: &BTreeMap<ProposalId, T>) -> amaru_ledger::store::Result<()> {
        Ok(())
    }

    fn set_constitution(&self, _constitution: &amaru_kernel::Constitution) -> amaru_ledger::store::Result<()> {
        Ok(())
    }

    fn update_constitutional_committee(
        &self,
        _status: &ConstitutionalCommitteeStatus,
        _added: &BTreeMap<amaru_kernel::StakeCredential, Epoch>,
        _removed: &BTreeSet<amaru_kernel::StakeCredential>,
    ) -> amaru_ledger::store::Result<()> {
        Ok(())
    }

    fn with_block_issuers(
        &self,
        _with: impl FnMut(amaru_ledger::store::columns::slots::Iter<'_, '_>),
    ) -> amaru_ledger::store::Result<()> {
        Ok(())
    }

    fn with_pots(&self, _with: impl FnMut(Box<dyn BorrowMut<pots::Row> + '_>)) -> amaru_ledger::store::Result<()> {
        Ok(())
    }

    #[expect(clippy::unwrap_used)]
    fn save(
        &self,
        _era_history: &EraHistory,
        _protocol_parameters: &ProtocolParameters,
        _governance_activity: Option<GovernanceActivity>,
        point: &Point,
        _issuer: Option<&pools::Key>,
        _add: Columns<
            impl Iterator<Item = (utxo::Key, utxo::Value)>,
            impl Iterator<Item = pools::Value>,
            impl Iterator<Item = (accounts::Key, accounts::Value)>,
            impl Iterator<Item = (dreps::Key, dreps::Value)>,
            impl Iterator<Item = (cc_members::Key, cc_members::Value)>,
            impl Iterator<Item = (proposals::Key, proposals::Value)>,
            impl Iterator<Item = (votes::Key, votes::Value)>,
        >,
        _remove: Columns<
            impl Iterator<Item = utxo::Key>,
            impl Iterator<Item = (pools::Key, Epoch)>,
            impl Iterator<Item = accounts::Key>,
            impl Iterator<Item = (dreps::Key, CertificatePointer)>,
            impl Iterator<Item = cc_members::Key>,
            impl Iterator<Item = ()>,
            impl Iterator<Item = ()>,
        >,
        _withdrawals: impl Iterator<Item = accounts::Key>,
    ) -> amaru_ledger::store::Result<()> {
        self.flushed.lock().unwrap().push(*point);
        Ok(())
    }
}
