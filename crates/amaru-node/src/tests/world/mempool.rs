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

//! Adversarial mempool scenarios on the production node graph.
//!
//! These tests deliberately use a deterministic validator so that the world harness can focus
//! on the stage protocol: invalid transactions must never enter, duplicates must not consume
//! capacity, and revalidation must remove transactions that no longer fit the adopted tip.
//!
//! These are focused mempool-stage scenarios, not end-to-end submission or forging tests. The
//! broader forge and ledger race scenarios are catalogued in [`SCENARIOS.md`](SCENARIOS.md).

use std::sync::Arc;

use amaru_consensus::effects::ResourceTxValidation;
use amaru_kernel::{Point, Transaction, cbor::WithOriginalBytes, to_cbor};
use amaru_mempool::{InMemoryMempool, MempoolConfig};
use amaru_ouroboros::{MempoolMsg, MempoolSeqNo, ResourceMempool, TxInsertResult, TxOrigin};
use amaru_ouroboros_traits::TransactionValidationError;
use amaru_pure_stage::StageRef;
use tokio::runtime::Runtime;

use super::{WorldConnectionProvider, WorldLoop, build_world_node_with_mempool_stage};
use crate::tests::{configuration::NodeTestConfig, test_data::create_transaction};

struct MempoolWorld {
    world: WorldLoop,
    _runtime: Runtime,
    mempool_stage: StageRef<MempoolMsg>,
    mempool: ResourceMempool<Transaction>,
}

impl MempoolWorld {
    fn new() -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("runtime");
        let provider = Arc::new(WorldConnectionProvider::new(0x004D_454D_504F_4F4C));
        let config = NodeTestConfig::default()
            .with_no_upstream_peers()
            .with_listen_address("127.0.0.1:39101")
            .with_seed(0x004D_454D_504F_4F4C);
        let (running, mempool_stage) =
            build_world_node_with_mempool_stage(&config, provider.clone(), runtime.handle()).expect("world node");
        let mempool = running.resources().get::<ResourceMempool<Transaction>>().expect("mempool").clone();
        let world = WorldLoop::new(provider, vec![running]);
        Self { world, _runtime: runtime, mempool_stage, mempool }
    }

    fn validate_expected_validity(&self) {
        let validator: ResourceTxValidation = Arc::new(|tx: &Transaction| {
            if tx.is_expected_valid {
                Ok(())
            } else {
                Err(TransactionValidationError::from(anyhow::anyhow!("transaction rejected by test validator")))
            }
        });
        self.world.graph(0).resources().put(validator);
    }

    fn reject_everything(&self) {
        let validator: ResourceTxValidation = Arc::new(|_: &Transaction| {
            Err(TransactionValidationError::from(anyhow::anyhow!("transaction invalid after tip")))
        });
        self.world.graph(0).resources().put(validator);
    }

    fn insert_batch(&mut self, txs: Vec<WithOriginalBytes<Transaction>>) {
        self.world.enqueue_msg(
            0,
            &self.mempool_stage,
            MempoolMsg::InsertBatch { txs, origin: TxOrigin::Local, caller: StageRef::blackhole() },
        );
        self.world.run_until_horizon(1_000_000_000);
    }

    fn state(&self) -> amaru_ouroboros::MempoolState {
        self.mempool.state()
    }
}

#[test]
fn world_mempool_rejects_invalid_and_duplicate_transactions() {
    let mut test = MempoolWorld::new();
    test.validate_expected_validity();

    let valid = create_transaction(0);
    let mut invalid = create_transaction(1).into_inner();
    invalid.is_expected_valid = false;
    let invalid = invalid.into();

    test.insert_batch(vec![valid.clone(), invalid, valid]);

    assert_eq!(test.state().tx_count, 1);
    assert_eq!(test.mempool.last(), MempoolSeqNo(1));
    assert_eq!(
        test.mempool.read().iter().map(|tx| tx.tx_id()).collect::<Vec<_>>(),
        vec![create_transaction(0).tx_id()]
    );
}

#[test]
fn world_mempool_duplicate_storm_does_not_consume_capacity_or_sequence_numbers() {
    let mut test = MempoolWorld::new();
    test.validate_expected_validity();

    let tx = create_transaction(5);
    let max_bytes = to_cbor(&tx).len() as u64;
    let limited: ResourceMempool<Transaction> =
        Arc::new(InMemoryMempool::new(MempoolConfig::default().with_max_bytes(max_bytes)));
    limited.set_tip(test.mempool.tip());
    test.world.graph(0).resources().put(limited.clone());
    test.mempool = limited;

    test.insert_batch(vec![tx; 1024]);

    assert_eq!(test.state().tx_count, 1);
    assert_eq!(test.mempool.last(), MempoolSeqNo(1));
    assert_eq!(test.state().size_bytes, max_bytes);
}

#[test]
fn world_mempool_rejects_transactions_after_capacity_without_partial_overflow() {
    let mut test = MempoolWorld::new();
    test.validate_expected_validity();

    let first = create_transaction(2);
    let max_bytes = to_cbor(&first).len() as u64;
    let limited: ResourceMempool<Transaction> =
        Arc::new(InMemoryMempool::new(MempoolConfig::default().with_max_bytes(max_bytes)));
    limited.set_tip(test.mempool.tip());
    test.world.graph(0).resources().put(limited.clone());
    test.mempool = limited;

    test.insert_batch(vec![first, create_transaction(3)]);

    assert_eq!(test.state().tx_count, 1);
    assert!(test.state().size_bytes <= max_bytes);
}

#[test]
fn world_mempool_revalidation_evicts_transactions_invalidated_by_the_new_tip() {
    let mut test = MempoolWorld::new();
    let tx = create_transaction(4);
    assert!(matches!(test.mempool.insert(tx, TxOrigin::Local), TxInsertResult::Accepted { .. }));
    test.reject_everything();

    test.world.enqueue_msg(0, &test.mempool_stage, MempoolMsg::NewTip(Point::Origin));
    test.world.run_until_horizon(1_000_000_000);

    assert_eq!(test.state().tx_count, 0);
    assert_eq!(test.mempool.tip(), Point::Origin);
}
