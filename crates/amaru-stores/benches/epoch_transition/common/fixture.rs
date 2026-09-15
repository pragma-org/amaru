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
    Block, BlockHeight, CertificatePointer, ConstitutionalCommitteeStatus, Epoch, EraHistory, GlobalParameters, Hash,
    NetworkName, PREPROD_DEFAULT_PROTOCOL_PARAMETERS, PREPROD_ERA_HISTORY, PREPROD_GLOBAL_PARAMETERS, Point,
    ProtocolParameters, Slot, TransactionInput, any_credential, any_modern_output, any_pool_params,
    cardano::network_block::make_block,
    cbor, make_header, to_cbor,
    utils::tests::{random_bytes_with_rng, run_strategy_with_rng},
};
use amaru_ledger::{
    epoch_transition::GovernanceActivity,
    state::{State, volatile::VolatileFragment},
    store::{Columns, Store, TransactionalContext},
};
use amaru_stores::rocksdb::RocksDBHistoricalStores;
use rand::{SeedableRng, rngs::SmallRng};

use super::{mock_store::MockStore, scale::EpochBenchScale};

/// Seed a RocksDB with pools, UTxOs, and accounts at the given scale, create the required epoch
/// snapshots, and build a `State` driven to the slot just before the epoch boundary.
///
/// Returns the State and the boundary slot. The caller times one `roll_forward` at the boundary
/// slot to measure the full epoch transition.
#[allow(clippy::expect_used)]
pub fn seed_and_build_state(scale: &EpochBenchScale) -> (State<MockStore, RocksDBHistoricalStores>, u64) {
    let era_history: EraHistory = PREPROD_ERA_HISTORY.clone();
    let global_parameters: GlobalParameters = PREPROD_GLOBAL_PARAMETERS.clone();
    let protocol_parameters: ProtocolParameters = PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone();

    let first_slot: u64 = 69_200_000; // inside preprod epoch 163
    let epoch = epoch_of(&era_history, first_slot);
    let boundary_slot = find_boundary_slot(&era_history, first_slot, epoch);

    let mock_store = MockStore::new();

    // Pools are pre-generated so their IDs can be reused when seeding accounts.
    let mut pool_rng = SmallRng::seed_from_u64(42);
    let pool_params_vec: Vec<amaru_kernel::PoolParams> =
        (0..scale.pools).map(|_| run_strategy_with_rng(&mut pool_rng, any_pool_params())).collect();
    let pool_ids: Vec<amaru_kernel::PoolId> = pool_params_vec.iter().map(|p| p.id).collect();

    // Seed protocol parameters and constitutional committee into the live DB before snapshotting.
    mock_store
        .db
        .with_transaction(|tx| {
            tx.set_protocol_parameters(&PREPROD_DEFAULT_PROTOCOL_PARAMETERS)?;
            tx.update_constitutional_committee(
                &ConstitutionalCommitteeStatus::NoConfidence,
                &std::collections::BTreeMap::new(),
                &std::collections::BTreeSet::new(),
            )
        })
        .expect("seeding protocol params succeeds");

    // Seed pools and UTxOs in a single transaction.
    let mut utxo_rng = SmallRng::seed_from_u64(43);
    let seeding_point = Point::Specific(Slot::from(first_slot), Hash::new([0u8; 32]), BlockHeight::from(1));
    let pool_params_for_seed = pool_params_vec.clone();
    mock_store
        .db
        .with_transaction(|tx| {
            tx.save(
                &era_history,
                &protocol_parameters,
                None,
                &seeding_point,
                None,
                Columns {
                    utxo: (0..scale.utxos as u64).map(move |i| {
                        let key = TransactionInput {
                            transaction_id: Hash::from(random_bytes_with_rng(&mut utxo_rng, 32).as_slice()),
                            index: i,
                        };
                        let value = run_strategy_with_rng(&mut utxo_rng, any_modern_output());
                        (key, value)
                    }),
                    pools: pool_params_for_seed
                        .into_iter()
                        .map(|params| (params, CertificatePointer::default(), 2_000_000u64)),
                    accounts: std::iter::empty(),
                    dreps: std::iter::empty(),
                    cc_members: std::iter::empty(),
                    proposals: std::iter::empty(),
                    votes: std::iter::empty(),
                },
                Columns {
                    utxo: std::iter::empty::<amaru_kernel::TransactionInput>(),
                    pools: std::iter::empty::<(amaru_kernel::PoolId, Epoch)>(),
                    accounts: std::iter::empty::<amaru_kernel::Credential>(),
                    dreps: std::iter::empty::<(amaru_kernel::Credential, CertificatePointer)>(),
                    cc_members: std::iter::empty::<amaru_kernel::Credential>(),
                    proposals: std::iter::empty::<()>(),
                    votes: std::iter::empty::<()>(),
                },
                std::iter::empty(),
            )
        })
        .expect("seeding pools and UTxOs succeeds");

    // Seed accounts delegated to the seeded pools.
    let mut account_rng = SmallRng::seed_from_u64(44);
    mock_store
        .db
        .save_bootstrap_accounts((0..scale.accounts).map(|i| {
            let credential = run_strategy_with_rng(&mut account_rng, any_credential());
            let row = amaru_ledger::store::columns::accounts::Row {
                pool: if pool_ids.is_empty() {
                    None
                } else {
                    Some((pool_ids[i % pool_ids.len()], CertificatePointer::default()))
                },
                deposit: 2_000_000,
                drep: None,
                rewards: 0,
            };
            (credential, row)
        }))
        .expect("seeding accounts succeeds");

    // Create the three epoch snapshots. Each snapshot captures the full seeded DB state.
    for snap_epoch in [epoch - 3, epoch - 2, epoch - 1] {
        mock_store.db.next_snapshot(snap_epoch).expect("snapshot creation succeeds");
    }

    let snapshots = RocksDBHistoricalStores::new(&mock_store.cfg, 0);

    let mut state = State::new_with(
        mock_store,
        snapshots,
        epoch,
        NetworkName::Preprod,
        era_history,
        global_parameters,
        protocol_parameters,
        GovernanceActivity::default(),
        None,
        VecDeque::new(),
    );

    // Drive the State to just before the stability window. The next roll_forward will spawn the
    // background rewards thread; the one after that (at the boundary) will join it and run the
    // full epoch transition.
    forward_to(&mut state, point(boundary_slot - 3));
    state.roll_forward(&empty_block_at(boundary_slot - 2), &amaru_plutus::arena_pool::ArenaPool::new(1024, 0));

    (state, boundary_slot)
}

#[allow(clippy::expect_used)]
fn epoch_of(era_history: &EraHistory, slot: u64) -> Epoch {
    era_history.slot_to_epoch_unchecked_horizon(Slot::from(slot)).expect("slot is within era history")
}

fn find_boundary_slot(era_history: &EraHistory, start: u64, epoch: Epoch) -> u64 {
    let mut slot = start + 1;
    while epoch_of(era_history, slot) == epoch {
        slot += 1;
    }
    slot
}

pub fn point(slot: u64) -> Point {
    Point::Specific(Slot::from(slot), Hash::new([slot as u8; 32]), BlockHeight::from(slot))
}

#[allow(clippy::expect_used)]
pub fn empty_block_at(slot: u64) -> Block {
    let header = make_header(slot, slot, None);
    let mut block = make_block();
    block.header = header;
    block.transaction_bodies.clear();
    block.transaction_witnesses.clear();
    block.auxiliary_data.clear();
    let mut block: Block = cbor::decode(to_cbor(&block).as_slice()).expect("block round-trips");
    block.header.body_mut().block_body_size = block.body_len();
    block.header.body_mut().block_body_hash = block.body_hash();
    cbor::decode(to_cbor(&block).as_slice()).expect("block round-trips")
}

fn forward_to(state: &mut State<MockStore, RocksDBHistoricalStores>, p: Point) {
    let issuer = Hash::new([0u8; 28]);
    #[allow(clippy::expect_used)]
    state.push_fragment(VolatileFragment::default().anchor(p, issuer)).expect("forward");
}
