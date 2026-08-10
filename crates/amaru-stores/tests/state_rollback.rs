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

//! Integration tests for ledger `State` rollback against the RocksDB store backend.
//!
//! Lives here (not in `amaru-ledger`) so that `amaru-ledger` need not depend on `amaru-stores`,
//! even as a dev-dependency.

use std::{
    borrow::BorrowMut,
    collections::VecDeque,
    fmt::{Debug, Display},
    sync::{Arc, Mutex},
};

use amaru_kernel::{
    Block, BlockHeight, CertificatePointer, Epoch, EraHistory, GlobalParameters, Hash, Header, NetworkName,
    PREPROD_DEFAULT_PROTOCOL_PARAMETERS, PREPROD_ERA_HISTORY, PREPROD_GLOBAL_PARAMETERS, Point, Pots, ProposalsRoots,
    ProtocolParameters, Slot, Tip, cardano::network_block::make_block, cbor, make_header, to_cbor,
};
use amaru_ledger::{
    epoch_transition::GovernanceActivity,
    state::{BackwardError, ForkSwitchOutcome, State, StateError, volatile::VolatileFragment},
    store::{
        Columns, EpochTransitionProgress, HistoricalStores, ReadStore, Store, TransactionalContext,
        columns::{accounts, cc_members, dreps, pools, pots, proposals, utxo, votes},
    },
};
use amaru_plutus::arena_pool::ArenaPool;
use amaru_stores::rocksdb::{RocksDB, RocksDBHistoricalStores, RocksDbConfig};
use anyhow::anyhow;

#[test]
fn rollback_to_a_volatile_common_ancestor_succeeds() {
    let mut state = make_state();
    let earlier = point(100, 1);
    let later = point(200, 2);

    assert_eq!(*state.tip(), Point::Origin);

    forward_to(&mut state, earlier, 1);
    forward_to(&mut state, later, 2);
    assert_eq!(*state.tip(), later);

    rollback_to(&mut state, &later).unwrap();
    assert_eq!(*state.tip(), later);

    rollback_to(&mut state, &earlier).unwrap();
    assert_eq!(*state.tip(), earlier);

    rollback_to(&mut state, &Point::Origin).unwrap();
    assert_eq!(*state.tip(), Point::Origin);
}

#[test]
fn rollback_before_volatile_front_is_rejected() {
    let mut state = make_state();
    forward_to(&mut state, point(100, 1), 1);
    forward_to(&mut state, point(200, 2), 2);

    let to = point(50, 9);

    assert_no_rollback_to(&mut state, &to, |err| {
        assert!(matches!(err, BackwardError::UnknownRollbackPoint { .. } if err.rollback_point() == to))
    });

    assert_eq!(*state.tip(), point(200, 2), "tip is unchanged after a rejected rollback");
}

#[test]
fn rollback_within_volatile_but_unknown_hash_is_rejected() {
    let mut state = make_state();
    forward_to(&mut state, point(100, 1), 1);
    forward_to(&mut state, point(200, 2), 2);

    let to = point(100, 2);

    assert_no_rollback_to(&mut state, &to, |err| {
        assert!(matches!(err, BackwardError::UnknownRollbackPoint { .. } if err.rollback_point() == to))
    });

    assert_eq!(*state.tip(), point(200, 2), "tip is unchanged after a rejected rollback");
}

#[test]
fn rollback_within_volatile_but_unknown_slot_is_rejected() {
    let mut state = make_state();
    forward_to(&mut state, point(100, 1), 1);
    forward_to(&mut state, point(200, 2), 2);

    let to = point(150, 1);

    assert_no_rollback_to(&mut state, &to, |err| {
        assert!(matches!(err, BackwardError::UnknownRollbackPoint { .. } if err.rollback_point() == to))
    });

    assert_eq!(*state.tip(), point(200, 2), "tip is unchanged after a rejected rollback");
}

#[test]
fn rollback_after_volatile_front_is_rejected() {
    let mut state = make_state();
    forward_to(&mut state, point(100, 1), 1);

    let to = point(101, 2);

    assert_no_rollback_to(&mut state, &to, |err| {
        assert!(matches!(err, BackwardError::RollbackPointInFuture { .. } if err.rollback_point() == to))
    });

    assert_eq!(*state.tip(), point(100, 1), "tip is unchanged after a rejected rollback");
}

#[test]
fn recover_restores_a_whole_volatile_db_rollback() {
    let era_history: EraHistory = PREPROD_ERA_HISTORY.clone();
    let first_slot = 43_200;
    let epoch = era_history.slot_to_epoch(Slot::from(first_slot), Slot::from(first_slot)).unwrap();
    let (mut state, _) = make_state_in_epoch(epoch);

    let point1 = point(first_slot + 100, 1);
    let point2 = point(first_slot + 200, 2);
    forward_to(&mut state, point1, 1);
    forward_to(&mut state, point2, 2);
    assert_eq!(*state.tip(), point2);

    // Rolling back to the immutable tip clears the whole volatile DB; failing on the first
    // replayed block must restore it wholesale.
    let invalid_block = invalid_block_at(first_slot + 300);
    let invalid_block_tip = invalid_block.tip();
    match state.switch_to_fork(&Point::Origin, std::iter::once(invalid_block), &ArenaPool::new(1024, 0)) {
        Ok(ForkSwitchOutcome::Failed { failure }) => assert_eq!(failure.tip, invalid_block_tip),
        other => panic!("expected a rolled back switch, got: {other:?}"),
    }

    assert_eq!(*state.tip(), point2, "tip is restored after recovering a whole volatileDB rollback");
}

#[test]
fn a_failed_switch_beating_the_replaced_chain_keeps_the_valid_prefix() {
    let era_history: EraHistory = PREPROD_ERA_HISTORY.clone();
    let k = PREPROD_GLOBAL_PARAMETERS.consensus_security_param;

    let first_slot = 43_200;
    let epoch = era_history.slot_to_epoch(Slot::from(first_slot), Slot::from(first_slot)).unwrap();
    let (mut state, flushed) = make_state_in_epoch(epoch);

    for slot in first_slot..(first_slot + k) {
        forward_to(&mut state, point(slot, 1), slot);
    }
    let tip_slot = first_slot + k - 1;

    // Roll back one block and replay three: two valid blocks and an invalid one. The valid blocks
    // make the fork one block longer than the chain it replaces, so the failure keeps the valid
    // prefix instead of undoing the switch.
    let fork_point = point(tip_slot - 1, 1);
    let block1 = empty_block_at(tip_slot + 1);
    let block2 = empty_block_at(tip_slot + 2);
    let block3 = invalid_block_at(tip_slot + 3);
    let block2_tip = block2.tip();
    let block3_tip = block3.tip();

    let blocks = vec![block1, block2, block3];
    match state.switch_to_fork(&fork_point, blocks, &ArenaPool::new(1024, 0)) {
        Ok(ForkSwitchOutcome::Partial { applied_tip: tip, metrics, failure }) => {
            assert_eq!(tip, block2_tip);
            assert_eq!(metrics.slot, tip_slot + 2);
            assert_eq!(failure.tip, block3_tip);
        }
        other => panic!("expected a partial switch, got: {other:?}"),
    }

    assert_eq!(*state.tip(), block2_tip.point(), "the ledger stays on the last applied block");
    assert_eq!(
        *flushed.lock().unwrap(),
        vec![point(first_slot, 1)],
        "the replay evicted the oldest block to the stable store"
    );
}

#[test]
fn a_failed_switch_not_beating_the_replaced_chain_is_rolled_back() {
    let era_history: EraHistory = PREPROD_ERA_HISTORY.clone();
    let k = PREPROD_GLOBAL_PARAMETERS.consensus_security_param;

    let first_slot = 43_200;
    let epoch = era_history.slot_to_epoch(Slot::from(first_slot), Slot::from(first_slot)).unwrap();
    let (mut state, flushed) = make_state_in_epoch(epoch);

    for slot in first_slot..(first_slot + k) {
        forward_to(&mut state, point(slot, 1), slot);
    }
    let tip_slot = first_slot + k - 1;
    let original_tip = tip(tip_slot, 1);

    // Roll back two blocks and fail on the second replayed block: the fork never gets longer than
    // the chain it replaces and nothing reached the stable store, so the switch is fully undone.
    let fork_point = point(tip_slot - 2, 1);
    let block1 = empty_block_at(tip_slot + 1);
    let block2 = invalid_block_at(tip_slot + 2);

    let block2_tip = block2.tip();
    let blocks = vec![block1, block2];
    match state.switch_to_fork(&fork_point, blocks, &ArenaPool::new(1024, 0)) {
        Ok(ForkSwitchOutcome::Failed { failure }) => assert_eq!(failure.tip, block2_tip),
        other => panic!("expected a rolled back switch, got: {other:?}"),
    }

    assert_eq!(*state.tip(), original_tip.point(), "the pre-switch tip is restored");
    assert!(flushed.lock().unwrap().is_empty(), "nothing reached the stable store");
}

#[test]
fn a_switch_replaying_across_an_epoch_boundary_attempts_the_epoch_transition() {
    let era_history: EraHistory = PREPROD_ERA_HISTORY.clone();
    let k = PREPROD_GLOBAL_PARAMETERS.consensus_security_param;

    // Find the first epoch boundary after an arbitrary anchor slot, then lay the volatile window
    // over the `k` slots leading up to it.
    let epoch_of = |slot: u64| era_history.slot_to_epoch(Slot::from(slot), Slot::from(slot)).unwrap();
    let anchor = 43_200;
    let epoch = epoch_of(anchor);
    let mut boundary = anchor + 1;
    while epoch_of(boundary) == epoch {
        boundary += 1;
    }
    let first_slot = boundary - k;
    assert_eq!(epoch_of(first_slot), epoch, "the whole window must sit inside a single epoch");

    let (mut state, flushed) = make_state_in_epoch(epoch);
    for slot in first_slot..boundary {
        forward_to(&mut state, point(slot, 1), slot);
    }
    let initial_tip = tip(boundary - 1, 1);

    // Replay one block on the other side of the boundary. Applying it must first transition the
    // ledger into the new epoch, which fails here because no rewards were ever computed: the
    // transition is attempted eagerly during the replay rather than silently skipped.
    let fork_point = point(boundary - 2, 1);
    let replayed = empty_block_at(boundary);

    let err = match state.switch_to_fork(&fork_point, std::iter::once(replayed), &ArenaPool::new(1024, 0)) {
        Ok(outcome) => panic!("expected the epoch transition to fail, got: {outcome:?}"),
        Err(err) => err,
    };
    assert!(
        matches!(err.downcast_ref::<StateError>(), Some(StateError::RewardsSummaryNotReady)),
        "unexpected error: {err:#}"
    );

    assert_eq!(*state.tip(), initial_tip.point(), "the pre-switch tip is restored after a failed transition");
    assert!(flushed.lock().unwrap().is_empty(), "nothing reached the stable store");
}

#[test]
fn a_successful_switch_to_fork_trims_the_volatile_window() {
    let era_history: EraHistory = PREPROD_ERA_HISTORY.clone();
    let k = PREPROD_GLOBAL_PARAMETERS.consensus_security_param;

    // We make sure that the whole test stays inside a single epoch so that we don't have to worry
    // about epoch transitions and the stable store.
    let first_slot = 43_200;
    let last_slot = first_slot + k + 2;
    let epoch = era_history.slot_to_epoch(Slot::from(first_slot), Slot::from(last_slot)).unwrap();
    assert_eq!(
        epoch,
        era_history.slot_to_epoch(Slot::from(last_slot), Slot::from(last_slot)).unwrap(),
        "the whole test must stay inside a single epoch"
    );

    let (mut state, flushed) = make_state_in_epoch(epoch);

    // Fill the volatile window right up to `k`.
    for slot in first_slot..(first_slot + k) {
        forward_to(&mut state, point(slot, 1), slot);
    }

    //   stable store ┊ volatile window                                    len
    //   ─────────────┼────────────────────────────────────────────  ───────────
    //      (empty)   ┊ 43200  43201  ⋯  45358  45359                 2160 = k

    let tip_slot = first_slot + k - 1;
    assert_eq!(*state.tip(), point(tip_slot, 1));

    // Roll back one block and replay three. This is what chain selection hands the ledger once a
    // rejected block has left the ledger tip behind the selected chain. The new chain is two blocks
    // longer than the one it replaces, so the replay overflows the window.
    let fork_point = point(tip_slot - 1, 1);
    let replayed: Vec<Block> = ((tip_slot + 1)..=(tip_slot + 3)).map(empty_block_at).collect();
    let replayed_tip = replayed[2].tip();

    //   after the rollback:                                              len                                                                                                                                                                                                                                                                                                                             //   ─────────────┼────────────────────────────────────────────  ───────────                                                                                                                                                                                                                                                                                                                          //      (empty)   ┊ 43200  43201  ⋯  45358                        2159 = k-1
    //                                                                                                                                                                                                                                                                                                                                                                                                    //   mid-replay, the second and third blocks each evict the oldest volatile entry:
    //   43200  43201 ┊ 43202  ⋯  45358 │ 45360  45361  45362         2160 = k
    //                                   └ replayed

    match state.switch_to_fork(&fork_point, replayed, &ArenaPool::new(1024, 0)) {
        Ok(ForkSwitchOutcome::Completed { metrics }) => assert_eq!(metrics.slot, tip_slot + 3),
        other => panic!("expected a completed switch, got: {other:?}"),
    }

    assert_eq!(
        *flushed.lock().unwrap(),
        vec![point(first_slot, 1), point(first_slot + 1, 1)],
        "exactly the two blocks pushed out of the window were flushed, oldest first"
    );
    assert_eq!(*state.tip(), replayed_tip.point());

    // Those two are on disk now and can no longer be taken back, so they are no longer legal
    // rollback targets.
    let flushed = point(first_slot + 1, 1);
    assert_no_rollback_to(&mut state, &flushed, |err| {
        assert!(matches!(err, BackwardError::UnknownRollbackPoint { .. } if err.rollback_point() == flushed))
    });
    // The oldest block still inside the window can still be rolled back to.
    let oldest_volatile_tip = point(first_slot + 2, 1);
    rollback_to(&mut state, &oldest_volatile_tip).unwrap();
    assert_eq!(*state.tip(), oldest_volatile_tip);
}

// HELPERS

fn rollback_to<S, HS>(state: &mut State<S, HS>, point: &Point) -> Result<(), anyhow::Error>
where
    S: Store,
    HS: HistoricalStores + Send + Sync + 'static,
{
    match state.switch_to_fork(point, std::iter::empty(), &ArenaPool::new(1024, 0))? {
        ForkSwitchOutcome::Completed { .. } => Ok(()),
        outcome @ ForkSwitchOutcome::Partial { .. } | outcome @ ForkSwitchOutcome::Failed { .. } => {
            Err(anyhow!("unexpected fork switch outcome: {outcome:?}"))
        }
    }
}

#[allow(clippy::panic)]
fn assert_no_rollback_to<S, HS, E>(state: &mut State<S, HS>, point: &Point, assert: impl FnOnce(&E))
where
    S: Store,
    HS: HistoricalStores + Send + Sync + 'static,
    E: Display + Debug + Send + Sync + 'static,
{
    assert_invalid_switch_to_fork_from(state, point, std::iter::empty(), assert)
}

#[allow(clippy::panic)]
fn assert_invalid_switch_to_fork_from<I, S, HS, E>(
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
fn make_state() -> State<MockStore, RocksDBHistoricalStores> {
    make_state_in_epoch(Epoch::default()).0
}

/// Create an initial ledger state anchored to a given epoch
#[expect(clippy::expect_used)]
fn make_state_in_epoch(epoch: Epoch) -> (State<MockStore, RocksDBHistoricalStores>, Arc<Mutex<Vec<Point>>>) {
    let network = NetworkName::Preprod;
    let era_history: EraHistory = PREPROD_ERA_HISTORY.clone();
    let global_parameters: GlobalParameters = PREPROD_GLOBAL_PARAMETERS.clone();
    let protocol_parameters: ProtocolParameters = PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone();

    let ledger_dir = tempfile::tempdir().expect("tempdir creation succeeds").keep();
    let cfg = RocksDbConfig::new(ledger_dir);
    let db = RocksDB::empty(&cfg).expect("RocksDB::empty succeeds");

    // Initialize at least one "most recent snapshot"
    db.next_snapshot(epoch).expect("snapshot creation succeeds");
    let snapshots = RocksDBHistoricalStores::new(&cfg, 0);
    assert_eq!(
        snapshots.snapshots().expect("listing snapshots succeeds"),
        vec![epoch],
        "the seeded snapshot must be visible to the historical store"
    );

    let flushed = Arc::new(Mutex::new(Vec::new()));
    let state = State::new_with(
        MockStore { db, flushed: flushed.clone() },
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
    (state, flushed)
}

/// An empty block carrying the given header. It has no transactions but is a valid block for the
/// ledger
#[expect(clippy::expect_used)]
fn empty_block(header: Header) -> Block {
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
    block.header.header_body.block_body_size = block.body_len();
    block.header.header_body.block_body_hash = block.body_hash();
    cbor::decode(to_cbor(&block).as_slice()).expect("stripped block should round-trip")
}

fn empty_block_at(slot: u64) -> Block {
    empty_block(make_header(slot, slot, None))
}

/// Forward the ledger to a given point
#[expect(clippy::expect_used)]
fn forward_to(state: &mut State<MockStore, RocksDBHistoricalStores>, point: Point, height: u64) {
    let issuer = Hash::new([0u8; 28]);
    let tip = Tip::new(point, BlockHeight::from(height));
    state.push_fragment(VolatileFragment::default().anchor(tip, issuer)).expect("forward");
}

fn tip(slot: u64, tag: u8) -> Tip {
    Tip::new(point(slot, tag), BlockHeight::from(slot))
}

fn point(slot: u64, tag: u8) -> Point {
    Point::Specific(Slot::from(slot), Hash::new([tag; 32]))
}

/// A block whose announced body hash does not match its body, so it fails validation.
#[expect(clippy::expect_used)]
fn invalid_block_at(slot: u64) -> Block {
    let mut block = empty_block_at(slot);
    block.header.header_body.block_body_hash = Hash::new([0xFF; 32]);
    let block: Block = cbor::decode(to_cbor(&block).as_slice()).expect("tampered block should round-trip");
    block
}

struct MockStore {
    db: RocksDB,
    /// The blocks that were persisted after flushing the volatile window.
    flushed: Arc<Mutex<Vec<Point>>>,
}

impl ReadStore for MockStore {
    fn tip(&self) -> amaru_ledger::store::Result<Point> {
        Ok(Point::Origin)
    }

    fn proposals_roots(&self) -> amaru_ledger::store::Result<ProposalsRoots> {
        Ok(ProposalsRoots::default())
    }

    fn pots(&self) -> amaru_ledger::store::Result<Pots> {
        Ok(Pots::default())
    }

    fn epoch_transition_progress(&self) -> amaru_ledger::store::Result<Option<EpochTransitionProgress>> {
        Ok(None)
    }

    fn iter_pools(&self) -> amaru_ledger::store::Result<impl Iterator<Item = (pools::Key, pools::Row)>> {
        Ok(std::iter::empty())
    }
}

impl Store for MockStore {
    type Transaction<'a> = MockTransaction<'a>;

    fn next_snapshot(&self, epoch: Epoch) -> amaru_ledger::store::Result<()> {
        self.db.next_snapshot(epoch)
    }

    fn create_transaction(&self) -> MockTransaction<'_> {
        MockTransaction { flushed: &self.flushed }
    }
}

/// Records the points written to the stable store instead of writing them, which keeps the test on
/// the ledger's flush behaviour rather than RocksDB's (covered by its own tests).
struct MockTransaction<'a> {
    flushed: &'a Mutex<Vec<Point>>,
}

impl ReadStore for MockTransaction<'_> {}

impl<'a> TransactionalContext<'a> for MockTransaction<'a> {
    fn commit(self) -> amaru_ledger::store::Result<()> {
        Ok(())
    }

    fn reset_epoch_transition_progress(&self) -> amaru_ledger::store::Result<()> {
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
        _governance_activity: GovernanceActivity,
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
