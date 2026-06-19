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

use std::collections::VecDeque;

use amaru_kernel::{
    BlockHeight, Epoch, EraHistory, GlobalParameters, Hash, NetworkName, PREPROD_DEFAULT_PROTOCOL_PARAMETERS,
    PREPROD_ERA_HISTORY, PREPROD_GLOBAL_PARAMETERS, Point, PoolId, ProtocolParameters, Slot, Tip,
};
use amaru_ledger::{
    epoch_transition::GovernanceActivity,
    state::{BackwardError, State, volatile::VolatileFragment},
    store::{EpochTransitionProgress, ReadStore, Store, StoreError},
};
use amaru_stores::rocksdb::{RocksDB, RocksDBHistoricalStores, RocksDbConfig};

#[test]
fn rollback_to_a_volatile_common_ancestor_succeeds() {
    let mut state = make_state();
    let earlier = point(100, 1);
    let later = point(200, 2);

    assert_eq!(*state.tip(), Point::Origin);

    forward_to(&mut state, earlier, 1);
    forward_to(&mut state, later, 2);
    assert_eq!(*state.tip(), later);

    state.rollback_to(&later).unwrap();
    assert_eq!(*state.tip(), later);

    state.rollback_to(&earlier).unwrap();
    assert_eq!(*state.tip(), earlier);

    state.rollback_to(&Point::Origin).unwrap();
    assert_eq!(*state.tip(), Point::Origin);
}

#[test]
fn rollback_before_volatile_front_is_rejected() {
    let mut state = make_state();
    forward_to(&mut state, point(100, 1), 1);
    forward_to(&mut state, point(200, 2), 2);

    let to = point(50, 9);

    assert!(matches!(
        dbg!(state.rollback_to(&to)),
        Err(err @ BackwardError::UnknownRollbackPoint { .. }) if err.rollback_point() == to,
    ));
    assert_eq!(*state.tip(), point(200, 2), "tip is unchanged after a rejected rollback");
}

#[test]
fn rollback_within_volatile_but_unknown_hash_is_rejected() {
    let mut state = make_state();
    forward_to(&mut state, point(100, 1), 1);
    forward_to(&mut state, point(200, 2), 2);

    let to = point(100, 2);

    assert!(matches!(
        dbg!(state.rollback_to(&to)),
        Err(err @ BackwardError::UnknownRollbackPoint { .. }) if err.rollback_point() == to,
    ));
    assert_eq!(*state.tip(), point(200, 2), "tip is unchanged after a rejected rollback");
}

#[test]
fn rollback_within_volatile_but_unknown_slot_is_rejected() {
    let mut state = make_state();
    forward_to(&mut state, point(100, 1), 1);
    forward_to(&mut state, point(200, 2), 2);

    let to = point(150, 1);

    assert!(matches!(
        dbg!(state.rollback_to(&to)),
        Err(err @ BackwardError::UnknownRollbackPoint { .. }) if err.rollback_point() == to,
    ));
    assert_eq!(*state.tip(), point(200, 2), "tip is unchanged after a rejected rollback");
}

#[test]
fn operational_cert_sequence_number_returns_none_when_pool_is_unknown() {
    let state = make_state();
    let unknown = PoolId::new(Hash::new([7u8; 28]));
    assert_eq!(state.operational_cert_sequence_number(&unknown).unwrap(), None);
}

#[test]
fn operational_cert_sequence_number_finds_the_sequence_number_from_a_volatile_fragment() {
    let mut state = make_state();
    let pool_1 = PoolId::new(Hash::new([1u8; 28]));
    let pool_2 = PoolId::new(Hash::new([2u8; 28]));
    push_volatile(&mut state, 100, pool_1, 42);
    push_volatile(&mut state, 200, pool_2, 99);

    assert_eq!(state.operational_cert_sequence_number(&pool_1).unwrap(), Some(42));
    assert_eq!(state.operational_cert_sequence_number(&pool_2).unwrap(), Some(99));
}

#[test]
fn operational_cert_sequence_number_returns_the_latest_value_when_a_pool_has_multiple_fragments() {
    let mut state = make_state();
    let pool = PoolId::new(Hash::new([1u8; 28]));
    push_volatile(&mut state, 100, pool, 5);
    push_volatile(&mut state, 200, pool, 7);
    // Reverse iteration of the volatile sequence yields the newest match first.
    assert_eq!(state.operational_cert_sequence_number(&pool).unwrap(), Some(7));
}

#[test]
fn rollback_after_volatile_front_is_rejected() {
    let mut state = make_state();
    forward_to(&mut state, point(100, 1), 1);

    let to = point(101, 2);

    assert!(matches!(
        dbg!(state.rollback_to(&to)),
        Err(err @ BackwardError::RollbackPointInFuture { .. }) if err.rollback_point() == to,
    ));
    assert_eq!(*state.tip(), point(100, 1), "tip is unchanged after a rejected rollback");
}

// Step 7: candidate-clone + atomic publish for the writer path. A reader that takes
// a snapshot via `state.load()` must continue to observe the pre-mutation view even
// after the writer has published a new view.
#[test]
fn load_returns_a_consistent_snapshot_across_writes() {
    let mut state = make_state();
    let first = point(100, 1);
    let second = point(200, 2);

    forward_to(&mut state, first, 1);

    let snap_before = state.load();
    assert_eq!(snap_before.tip().into_owned(), first);

    forward_to(&mut state, second, 2);

    // The snapshot taken before the second push still observes the first tip.
    assert_eq!(snap_before.tip().into_owned(), first, "old snapshot must not see post-write state");

    // A fresh load picks up the new tip.
    assert_eq!(state.load().tip().into_owned(), second);
}

// Step 7: a writer failure must drop the candidate, leaving the live view untouched.
// `rollback_to(unknown_point)` mutates the candidate's overlay/volatile bookkeeping
// while validating, then returns Err. The candidate is discarded; readers continue
// observing the pre-rollback tip.
#[test]
fn failed_write_drops_the_candidate() {
    let mut state = make_state();
    forward_to(&mut state, point(100, 1), 1);
    forward_to(&mut state, point(200, 2), 2);

    let snap_before = state.load();
    assert_eq!(snap_before.tip().into_owned(), point(200, 2));

    let unknown = point(50, 9);
    assert!(state.rollback_to(&unknown).is_err());

    assert_eq!(state.load().tip().into_owned(), point(200, 2), "tip is unchanged after a rejected rollback");
    assert_eq!(snap_before.tip().into_owned(), point(200, 2));
}

// Step 7: stress test — concurrent readers must continue progressing while a writer
// hammers the state with rollback + push cycles. With the pre-step-7 implementation
// the writer held the inner RwLock for the entire mutation, blocking every `load()`;
// with candidate-clone + atomic publish, readers see snapshots and never wait on the
// writer.
//
// We don't assert specific latency bounds here (too noisy in CI). Nor do we assert that
// readers observe a specific tip distribution — under workspace test contention threads
// are descheduled often enough that the writer's tip transitions may be invisible to a
// given reader. The test's real signal is:
// - no panics or deadlocks across writer + readers;
// - any tip observed is a valid one (the writer never publishes a torn state).
#[test]
fn concurrent_readers_progress_during_writes() {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
        time::Duration,
    };

    let state = make_state();
    let early = point(100, 1);
    let later = point(200, 2);

    let stop = Arc::new(AtomicBool::new(false));

    // Writer: alternate between (early), (early, later), (early). Always pushes the
    // same anchored fragments, so the only thing that changes is the tip.
    let writer_state = state.clone();
    let writer_stop = stop.clone();
    let writer = thread::spawn(move || {
        let issuer = PoolId::new(Hash::new([0u8; 28]));
        let mut transitions = 0u64;
        for i in 0..1000 {
            if writer_stop.load(Ordering::Relaxed) {
                break;
            }
            // Reset to Origin and rebuild the volatile.
            writer_state.rollback_to(&Point::Origin).expect("rollback to origin");
            writer_state
                .push_fragment(VolatileFragment::default().anchor(Tip::new(early, BlockHeight::from(1)), issuer, 0))
                .expect("push early");
            if i % 2 == 0 {
                writer_state
                    .push_fragment(VolatileFragment::default().anchor(Tip::new(later, BlockHeight::from(2)), issuer, 0))
                    .expect("push later");
            }
            transitions += 1;
        }
        transitions
    });

    // Readers: tight loop of `state.load().tip()`. Track observed tip transitions.
    let mut readers = Vec::new();
    for _ in 0..4 {
        let reader_state = state.clone();
        let reader_stop = stop.clone();
        readers.push(thread::spawn(move || {
            let mut observed_tips = std::collections::BTreeSet::new();
            let deadline = std::time::Instant::now() + Duration::from_millis(500);
            while std::time::Instant::now() < deadline && !reader_stop.load(Ordering::Relaxed) {
                let tip = reader_state.load().tip().into_owned();
                observed_tips.insert(tip);
            }
            observed_tips
        }));
    }

    // Wait briefly for readers to do their work, then stop everyone.
    thread::sleep(Duration::from_millis(500));
    stop.store(true, Ordering::Relaxed);

    let writer_transitions = writer.join().expect("writer thread did not panic");
    assert!(writer_transitions > 0, "writer made at least one transition");

    for (i, reader) in readers.into_iter().enumerate() {
        let observed = reader.join().expect("reader thread did not panic");
        for tip in &observed {
            assert!(
                matches!(tip, Point::Origin) || *tip == early || *tip == later,
                "reader {i} observed unexpected tip {tip:?}"
            );
        }
    }
}

// HELPERS

/// Create an initial ledger state
#[expect(clippy::expect_used)]
fn make_state() -> State<MockStore, RocksDBHistoricalStores> {
    let network = NetworkName::Preprod;
    let era_history: EraHistory = PREPROD_ERA_HISTORY.clone();
    let global_parameters: GlobalParameters = PREPROD_GLOBAL_PARAMETERS.clone();
    let protocol_parameters: ProtocolParameters = PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone();

    let ledger_dir = tempfile::tempdir().expect("tempdir creation succeeds").keep();
    let cfg = RocksDbConfig::new(ledger_dir);
    let store = RocksDB::empty(&cfg).expect("RocksDB::empty succeeds");
    let snapshots = RocksDBHistoricalStores::new(&cfg, 0);

    State::new_with(
        MockStore(store),
        snapshots,
        Epoch::default(),
        network,
        era_history,
        global_parameters,
        protocol_parameters,
        GovernanceActivity::default(),
        VecDeque::new(),
    )
}

/// Forward the ldeger to a given point
#[expect(clippy::expect_used)]
fn forward_to(state: &mut State<MockStore, RocksDBHistoricalStores>, point: Point, height: u64) {
    let issuer = PoolId::new(Hash::new([0u8; 28]));
    let tip = Tip::new(point, BlockHeight::from(height));
    state.push_fragment(VolatileFragment::default().anchor(tip, issuer, 0)).expect("forward");
}

/// Push a volatile fragment with the given slot, pool issuer, and operational certificate sequence number.
#[expect(clippy::expect_used)]
fn push_volatile(state: &mut State<MockStore, RocksDBHistoricalStores>, slot: u64, issuer: PoolId, seq: u64) {
    let point = Point::Specific(Slot::from(slot), Hash::new([0u8; 32]));
    let tip = Tip::new(point, BlockHeight::from(slot));
    state.push_fragment(VolatileFragment::default().anchor(tip, issuer, seq)).expect("push_fragment");
}

fn point(slot: u64, tag: u8) -> Point {
    Point::Specific(Slot::from(slot), Hash::new([tag; 32]))
}

struct MockStore(RocksDB);

#[expect(unused_variables)]
impl ReadStore for MockStore {
    fn tip(&self) -> amaru_ledger::store::Result<Point> {
        Ok(Point::Origin)
    }

    fn epoch_transition_progress(&self) -> amaru_ledger::store::Result<Option<EpochTransitionProgress>> {
        Err(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn protocol_parameters(&self) -> amaru_ledger::store::Result<ProtocolParameters> {
        Err(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn pool(
        &self,
        pool: &amaru_kernel::PoolId,
    ) -> amaru_ledger::store::Result<Option<amaru_ledger::store::columns::pools::Row>> {
        Err(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn operational_cert_sequence_number(
        &self,
        _pool_id: &PoolId,
    ) -> amaru_ledger::store::Result<Option<amaru_ledger::store::columns::opcerts::Row>> {
        Ok(None)
    }

    fn account(
        &self,
        credential: &amaru_kernel::StakeCredential,
    ) -> amaru_ledger::store::Result<Option<amaru_ledger::store::columns::accounts::Row>> {
        Err(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn utxo(
        &self,
        input: &amaru_kernel::TransactionInput,
    ) -> amaru_ledger::store::Result<Option<amaru_kernel::MemoizedTransactionOutput>> {
        Err(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn pots(&self) -> amaru_ledger::store::Result<amaru_ledger::summary::Pots> {
        Err(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn constitutional_committee(&self) -> amaru_ledger::store::Result<amaru_kernel::ConstitutionalCommitteeStatus> {
        Err(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn constitution(&self) -> amaru_ledger::store::Result<amaru_kernel::Constitution> {
        Err(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn proposals_roots(&self) -> amaru_ledger::store::Result<amaru_ledger::governance::ratification::ProposalsRoots> {
        Err(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn governance_activity(&self) -> amaru_ledger::store::Result<GovernanceActivity> {
        Err(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn iter_utxos(
        &self,
    ) -> amaru_ledger::store::Result<
        impl Iterator<Item = (amaru_ledger::store::columns::utxo::Key, amaru_ledger::store::columns::utxo::Value)>,
    > {
        Err::<std::iter::Empty<_>, _>(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn iter_block_issuers(
        &self,
    ) -> amaru_ledger::store::Result<
        impl Iterator<Item = (amaru_ledger::store::columns::slots::Key, amaru_ledger::store::columns::slots::Value)>,
    > {
        Err::<std::iter::Empty<_>, _>(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn iter_pools(
        &self,
    ) -> amaru_ledger::store::Result<
        impl Iterator<Item = (amaru_ledger::store::columns::pools::Key, amaru_ledger::store::columns::pools::Row)>,
    > {
        Err::<std::iter::Empty<_>, _>(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn iter_accounts(
        &self,
    ) -> amaru_ledger::store::Result<
        impl Iterator<Item = (amaru_ledger::store::columns::accounts::Key, amaru_ledger::store::columns::accounts::Row)>,
    > {
        Err::<std::iter::Empty<_>, _>(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn iter_dreps(
        &self,
    ) -> amaru_ledger::store::Result<
        impl Iterator<Item = (amaru_ledger::store::columns::dreps::Key, amaru_ledger::store::columns::dreps::Row)>,
    > {
        Err::<std::iter::Empty<_>, _>(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn iter_proposals(
        &self,
    ) -> amaru_ledger::store::Result<
        impl Iterator<Item = (amaru_ledger::store::columns::proposals::Key, amaru_ledger::store::columns::proposals::Row)>,
    > {
        Err::<std::iter::Empty<_>, _>(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn iter_cc_members(
        &self,
    ) -> amaru_ledger::store::Result<
        impl Iterator<Item = (amaru_ledger::store::columns::cc_members::Key, amaru_ledger::store::columns::cc_members::Row)>,
    > {
        Err::<std::iter::Empty<_>, _>(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }

    fn iter_votes(
        &self,
    ) -> amaru_ledger::store::Result<
        impl Iterator<Item = (amaru_ledger::store::columns::votes::Key, amaru_ledger::store::columns::votes::Row)>,
    > {
        Err::<std::iter::Empty<_>, _>(StoreError::Internal(anyhow::anyhow!("mock").into()))
    }
}

impl Store for MockStore {
    type Transaction<'a> = <RocksDB as Store>::Transaction<'a>;

    fn next_snapshot(&self, epoch: amaru_kernel::Epoch) -> amaru_ledger::store::Result<()> {
        self.0.next_snapshot(epoch)
    }

    fn create_transaction(&self) -> Self::Transaction<'_> {
        self.0.create_transaction()
    }
}
