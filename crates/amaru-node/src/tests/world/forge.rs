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

//! Deterministic forge-stage race scenarios.
//!
//! These tests use the world scheduler around the production forge stage. They deliberately
//! keep the ledger constraint resource configurable so that a tip change can be placed at the
//! forge boundary without relying on wall-clock timing. The test comments document the event
//! ordering, oracle, invariant, and regression covered by each scenario.

use std::{ops::Range, sync::Arc, time::Duration};

use amaru_consensus::{
    effects::{ResourceBlockBuilderConstraints, ResourceConsensusParameters, ResourceEraHistory, ResourceTxValidation},
    performance::{Performance, ResourcePerformance},
    stages::{
        forge_block::{
            DueLead, EpochSchedule, ForgeBlock, ForgeBlockMsg, ResourceForgingCredentials, TestCredentials, stage,
        },
        mempool::{MempoolStageState, stage as mempool_stage_fn},
        test_utils::start_in_era,
    },
};
use amaru_kernel::{
    Block, BlockBodyBuilder, BlockBuilderConstraints, ConsensusParameters, Epoch, HasTransactionId, HeaderHash,
    IsHeader, Nonce, PREPROD_DEFAULT_PROTOCOL_PARAMETERS, PREPROD_ERA_HISTORY, PREPROD_GLOBAL_PARAMETERS, Point,
    ProtocolVersion, Redeemers, ReferenceInputs, Slot, Transaction, WitnessSet, cbor::WithOriginalBytes,
    maths::FixedDecimal,
};
use amaru_mempool::InMemoryMempool;
use amaru_ouroboros::{MempoolMsg, ResourceMempool, TxOrigin, in_memory_chain_store::InMemoryChainStore};
use amaru_ouroboros_traits::{
    BaseReadChainStore, BlockBuilderConstraintsResponse, CanReadBlockBuilderConstraints, DiagnosticChainStore,
    ForgingCredentials, LedgerThreadTerminated, MockBlockValidator, MockCanValidateTxs, Nonces,
};
use amaru_protocols::store_effects::{ResourceHeaderStore, ResourceParameters};
use amaru_pure_stage::{StageGraph, StageRef, simulation::SimulationBuilder};
use async_trait::async_trait;
use test_case::test_case;
use tokio::runtime::Runtime;

use super::{WorldConnectionProvider, WorldLoop};
use crate::tests::test_data::create_transaction;

const WORLD_SEED: u64 = 0x464F5247455F5241;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct SubmitterState {
    mempool: StageRef<MempoolMsg>,
}

struct FixedConstraints {
    tip: Point,
    constraints: BlockBuilderConstraints,
}

#[async_trait]
impl CanReadBlockBuilderConstraints for FixedConstraints {
    async fn block_builder_constraints(&self) -> Result<BlockBuilderConstraintsResponse, LedgerThreadTerminated> {
        Ok(BlockBuilderConstraintsResponse { tip: self.tip, constraints: self.constraints.clone() })
    }
}

struct TipChangingConstraints {
    response: BlockBuilderConstraintsResponse,
    mempool: ResourceMempool<Transaction>,
    tip: Point,
}

#[async_trait]
impl CanReadBlockBuilderConstraints for TipChangingConstraints {
    async fn block_builder_constraints(&self) -> Result<BlockBuilderConstraintsResponse, LedgerThreadTerminated> {
        self.mempool.set_tip(self.tip);
        Ok(self.response.clone())
    }
}

struct ForgeWorld {
    world: WorldLoop,
    _runtime: Runtime,
    forge: StageRef<ForgeBlockMsg>,
    submit: StageRef<WithOriginalBytes<Transaction>>,
    mempool_stage: StageRef<MempoolMsg>,
    mempool: ResourceMempool<Transaction>,
    store: Arc<InMemoryChainStore>,
}

impl ForgeWorld {
    fn new(ledger_tip: Point) -> Self {
        Self::new_with_tips(ledger_tip, ledger_tip)
    }

    fn new_with_tips(ledger_tip: Point, mempool_tip: Point) -> Self {
        let mempool: ResourceMempool<Transaction> = Arc::new(InMemoryMempool::default());
        mempool.set_tip(mempool_tip);
        Self::new_with_mempool(mempool, Arc::new(MockBlockValidator::new(ledger_tip)))
    }

    fn new_with_resource(mempool_tip: Point, validator: Arc<dyn CanReadBlockBuilderConstraints + Send + Sync>) -> Self {
        let mempool: ResourceMempool<Transaction> = Arc::new(InMemoryMempool::default());
        mempool.set_tip(mempool_tip);
        Self::new_with_mempool(mempool, validator)
    }

    fn new_with_mempool(
        mempool: ResourceMempool<Transaction>,
        validator: Arc<dyn CanReadBlockBuilderConstraints + Send + Sync>,
    ) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("runtime");
        let consensus_parameters = ConsensusParameters::new(PREPROD_GLOBAL_PARAMETERS.clone(), &PREPROD_ERA_HISTORY);
        let store = Arc::new(InMemoryChainStore::new());
        let forge = StageRef::named_for_tests("forge-race/select-chain");
        let slot = simulation_slot();
        let credentials = TestCredentials::for_test_keys(
            consensus_parameters.slot_to_kes_period(slot),
            u64::from(PREPROD_GLOBAL_PARAMETERS.max_kes_evolution),
        );
        let epoch = PREPROD_ERA_HISTORY.slot_to_epoch_unchecked_horizon(slot).expect("simulation epoch");
        let schedule = ready_schedule(epoch, slot..slot + 1);
        let mut state = ForgeBlock::new(
            forge.clone(),
            consensus_parameters.clone(),
            PREPROD_GLOBAL_PARAMETERS.system_start,
            PREPROD_GLOBAL_PARAMETERS.consensus_security_param,
            credentials.pool_id(),
            ProtocolVersion::new(11, 0),
            credentials.issuer_fields(),
        );
        state.data.adopted_tip = Point::Origin;
        state.data.adopted_parent = Point::Origin;
        state.data.schedule.request(epoch, Nonce::from([0u8; 32]));
        state.data.schedule.install(schedule);

        let provider = Arc::new(WorldConnectionProvider::new(WORLD_SEED));
        let mut builder =
            SimulationBuilder::default().with_seed(WORLD_SEED).with_global_epoch_offset(start_in_era().relative_time);
        let forge_stage = builder.stage("forge-race", stage);
        let forge_stage = builder.wire_up(forge_stage, state);
        let mempool_stage = builder.stage("forge-race/mempool", mempool_stage_fn);
        let mempool_stage = builder.wire_up(mempool_stage, MempoolStageState::default());
        let submit_stage = builder.stage("forge-race/submit", async |state: SubmitterState, tx, eff| {
            let _ = eff
                .call(&state.mempool, Duration::from_secs(1), move |caller| MempoolMsg::Insert {
                    tx: Box::new(tx),
                    origin: TxOrigin::Local,
                    caller,
                })
                .await;
            state
        });
        let submit_stage =
            builder.wire_up(submit_stage, SubmitterState { mempool: mempool_stage.clone().without_state() });
        builder.resources().put::<ResourceHeaderStore>(store.clone());
        builder.resources().put::<ResourceMempool<Transaction>>(mempool.clone());
        builder.resources().put::<ResourceBlockBuilderConstraints>(validator);
        builder.resources().put::<ResourceConsensusParameters>(Arc::new(consensus_parameters));
        builder.resources().put::<ResourceParameters>(PREPROD_GLOBAL_PARAMETERS.clone());
        builder.resources().put::<ResourceEraHistory>(PREPROD_ERA_HISTORY.clone());
        builder.resources().put::<ResourceTxValidation>(Arc::new(MockCanValidateTxs));
        builder.resources().put::<ResourcePerformance>(Arc::new(Performance::new()));
        builder
            .resources()
            .put::<amaru_consensus::stages::forge_block::ResourceForgingCredentials>(Some(
                Arc::new(credentials) as Arc<dyn ForgingCredentials>
            ));
        let mut running = builder.run(runtime.handle());
        running.override_external_effect::<amaru_consensus::effects::ValidateHeaderEffect>(usize::MAX, |_| {
            amaru_pure_stage::simulation::running::OverrideResult::handled(Ok(Nonces::for_tests()))
        });
        let world = WorldLoop::new(provider, vec![running]);
        Self {
            world,
            _runtime: runtime,
            forge: forge_stage.without_state(),
            submit: submit_stage.without_state(),
            mempool_stage: mempool_stage.without_state(),
            mempool,
            store,
        }
    }

    fn due_lead(&mut self) {
        self.world.enqueue_msg(0, &self.forge, ForgeBlockMsg::from(DueLead { slot: simulation_slot(), generation: 0 }));
    }

    fn submit(&mut self, tx: WithOriginalBytes<Transaction>) {
        self.world.enqueue_msg(0, &self.submit, tx);
    }

    fn submit_and_settle(&mut self, tx: WithOriginalBytes<Transaction>) {
        self.submit(tx);
        self.world.run_until_horizon(1);
    }

    fn load_block(&self, header: &HeaderHash) -> Block {
        self.store
            .load_block(header)
            .expect("load forged block")
            .expect("forged block")
            .decode()
            .expect("decode forged block")
    }
}

fn simulation_slot() -> Slot {
    PREPROD_ERA_HISTORY
        .relative_time_to_slot(start_in_era().relative_time + Duration::from_secs(10))
        .expect("simulation slot")
}

fn ready_schedule(epoch: Epoch, slots: Range<Slot>) -> EpochSchedule {
    let nonce = Nonce::from([0u8; 32]);
    EpochSchedule::compute(
        epoch,
        nonce,
        slots,
        FixedDecimal::one(),
        FixedDecimal::one(),
        &amaru_consensus::stages::forge_block::test_vrf_key(),
    )
}

#[derive(Clone, Copy, Debug)]
enum CapacityDimension {
    Size,
    ReferenceScript,
    Memory,
    Cpu,
}

fn reference_script_transaction(id: u16) -> WithOriginalBytes<Transaction> {
    let mut transaction = create_transaction(id).into_inner();
    let mut reference_input = vec![0x81, 0x82, 0x58, 0x20];
    reference_input.extend([0; 32]);
    reference_input.push(0);
    let mut protocol_version = ProtocolVersion::default();
    transaction.body.reference_inputs = Some(
        amaru_kernel::cbor::from_cbor_no_leftovers_with::<_, ReferenceInputs>(&reference_input, &mut protocol_version)
            .expect("valid reference input"),
    );
    transaction.body.fee = 1_000_000_000;
    transaction.into()
}

fn execution_units_transaction(id: u16) -> WithOriginalBytes<Transaction> {
    let mut protocol_version = ProtocolVersion::default();
    let redeemers: Redeemers = amaru_kernel::cbor::from_cbor_no_leftovers_with(
        &[0x81, 0x84, 0x00, 0x00, 0x00, 0x82, 0x07, 0x0b],
        &mut protocol_version,
    )
    .expect("valid redeemer");
    let witnesses = WitnessSet { redeemer: Some(redeemers), ..WitnessSet::default() };
    let witness_size =
        amaru_kernel::cbor::to_vec_with(&witnesses, &mut protocol_version).expect("valid witness set").len();
    Transaction {
        witnesses: amaru_kernel::cbor::WithSize::new(witnesses, witness_size),
        ..create_transaction(id).into_inner()
    }
    .into()
}

fn capacity_transactions(
    dimension: CapacityDimension,
) -> (WithOriginalBytes<Transaction>, WithOriginalBytes<Transaction>) {
    match dimension {
        CapacityDimension::Size => (create_transaction(14), create_transaction(15)),
        CapacityDimension::ReferenceScript => (reference_script_transaction(14), reference_script_transaction(15)),
        CapacityDimension::Memory | CapacityDimension::Cpu => {
            (execution_units_transaction(14), execution_units_transaction(15))
        }
    }
}

fn capacity_constraints(
    dimension: CapacityDimension,
    first: &WithOriginalBytes<Transaction>,
) -> BlockBuilderConstraints {
    let mut parameters = (*PREPROD_DEFAULT_PROTOCOL_PARAMETERS).clone();
    let baseline = BlockBuilderConstraints::from(&parameters);
    let mut builder = BlockBodyBuilder::default();
    assert!(builder.try_push(first, &baseline).expect("first transaction must be measurable"));

    match dimension {
        CapacityDimension::Size => parameters.max_block_body_size = builder.size(),
        CapacityDimension::ReferenceScript => {
            let reference_script_size = builder.ref_scripts_size();
            assert!(reference_script_size > 0, "reference script transaction must have a non-zero estimate");
            parameters.max_ref_script_size_per_block =
                u32::try_from(reference_script_size).expect("reference script estimate fits protocol limit");
        }
        CapacityDimension::Memory => parameters.max_block_ex_units.mem = builder.ex_units().mem,
        CapacityDimension::Cpu => parameters.max_block_ex_units.steps = builder.ex_units().steps,
    }

    BlockBuilderConstraints::from(&parameters)
}

/// `forge.stale_parent`: the ledger constraint tip is changed before body selection.
///
/// The `DueLead` is already scheduled, but `TakeForForgeEffect` observes a ledger tip that is
/// neither the requested parent nor the mempool tip. The oracle is the chain store: no header or
/// block may be written. This proves that a forge request cannot publish a block for a stale fork.
#[test]
fn world_forge_skips_when_ledger_tip_does_not_match_parent() {
    let stale_tip = Point::Specific(Slot::from(1), amaru_kernel::Hash::new([1u8; 32]), 1.into());
    let mut forge = ForgeWorld::new(stale_tip);
    forge.due_lead();
    forge.world.run_until_horizon(20_000_000_000);

    assert!(forge.store.load_headers().next().is_none(), "stale forge must not store a header");
    assert!(forge.world.graph(0).contains_stage(forge.forge.name()), "forge stage must remain live");
}

/// `forge.ledger_ahead`: the ledger has advanced beyond the requested forge slot.
///
/// The ledger hash still matches the requested parent, so this isolates the slot-order guard
/// rather than the parent-hash guard. The oracle is again the chain store: an old lead cannot
/// publish a block after the ledger has already moved past its slot.
#[test]
fn world_forge_skips_when_ledger_tip_is_ahead_of_requested_slot() {
    let slot = simulation_slot();
    let ledger_tip = Point::Specific(slot + 1, amaru_kernel::ORIGIN_HASH, 1.into());
    let mut forge = ForgeWorld::new(ledger_tip);
    forge.due_lead();
    forge.world.run_until_horizon(20_000_000_000);

    assert!(forge.store.load_headers().next().is_none(), "an old forge slot must not store a header");
    assert!(forge.world.graph(0).contains_stage(forge.forge.name()), "forge stage must remain live");
}

/// `forge.mempool_tip_lag`: ledger constraints and the mempool describe different points.
///
/// The ledger remains at the requested parent while the mempool advertises a later point. The
/// oracle is the absence of a stored header, proving that transaction selection cannot combine
/// a current ledger view with transactions validated against another tip.
#[test]
fn world_forge_skips_when_mempool_tip_lags_the_ledger() {
    let ledger_tip = Point::Origin;
    let mempool_tip = Point::Specific(Slot::from(1), amaru_kernel::ORIGIN_HASH, 1.into());
    let mut forge = ForgeWorld::new_with_tips(ledger_tip, mempool_tip);
    forge.due_lead();
    forge.world.run_until_horizon(20_000_000_000);

    assert!(forge.store.load_headers().next().is_none(), "a stale mempool view must not store a header");
    assert!(forge.world.graph(0).contains_stage(forge.forge.name()), "forge stage must remain live");
}

/// `forge.tip_changes_during_take`: the mempool tip changes after the ledger snapshot is read.
///
/// The constraints provider changes the shared mempool tip immediately after returning the
/// ledger response. The oracle is the absence of a header and the live forge stage, proving that
/// the body-selection effect compares both views at the selection boundary rather than trusting
/// the first snapshot it receives.
#[test]
fn world_forge_skips_when_tip_changes_during_take() {
    let changed_tip = Point::Specific(Slot::from(1), amaru_kernel::ORIGIN_HASH, 1.into());
    let mempool: ResourceMempool<Transaction> = Arc::new(InMemoryMempool::default());
    mempool.set_tip(Point::Origin);
    let constraints = BlockBuilderConstraints::from(&*PREPROD_DEFAULT_PROTOCOL_PARAMETERS);
    let provider = TipChangingConstraints {
        response: BlockBuilderConstraintsResponse { tip: Point::Origin, constraints },
        mempool: mempool.clone(),
        tip: changed_tip,
    };
    let mut forge = ForgeWorld::new_with_mempool(mempool, Arc::new(provider));
    forge.due_lead();
    forge.world.run_until_horizon(20_000_000_000);

    assert!(forge.store.load_headers().next().is_none(), "a changing tip must abort body selection");
    assert!(forge.world.graph(0).contains_stage(forge.forge.name()), "forge stage must remain live");
}

/// `forge.duplicate_lead`: two identical lead messages are queued before the stage runs.
///
/// The first message consumes the slot and publishes at most one block. The second message is
/// stale after the schedule generation advances. The oracle is the chain store, proving that
/// duplicate scheduling signals cannot create two blocks for one slot.
#[test]
fn world_forge_does_not_publish_duplicate_leads() {
    let mut forge = ForgeWorld::new(Point::Origin);
    forge.due_lead();
    forge.due_lead();
    forge.world.run_until_horizon(20_000_000_000);

    let headers = forge.store.load_headers().count();
    let trace = forge.world.graph(0).trace_buffer().lock().hydrate_without_timestamps();
    assert_eq!(
        headers,
        1,
        "one slot must publish exactly one header; trace={trace:?}; heap={:?}",
        forge.world.heap_log()
    );
}

/// `forge.submit_before_take`: a locally submitted transaction is accepted before the lead.
///
/// The submitter uses the same `MempoolMsg::Insert` call as the local submission boundary and
/// only sends `DueLead` after the mempool acknowledges acceptance. The oracle is the stored block
/// and the mempool, proving that accepted local traffic is selected and removed exactly once.
#[test]
fn world_forge_includes_transaction_submitted_before_take() {
    let mut forge = ForgeWorld::new(Point::Origin);
    let tx = create_transaction(10);
    let tx_id = tx.tx_id();
    forge.submit_and_settle(tx);
    forge.due_lead();
    forge.world.run_until_horizon(20_000_000_000);

    let header = forge.store.load_headers().next().expect("submitted transaction must trigger a forge");
    let block = forge.load_block(&header.hash());

    assert_eq!(block.into_iter().map(|(_, tx)| tx.tx_id()).collect::<Vec<_>>(), vec![tx_id]);
    assert!(forge.mempool.read().iter().next().is_none(), "selected transaction must leave the mempool");
}

/// `forge.submit_after_take`: a submission arriving after body selection belongs to a later block.
///
/// The lead is processed first and creates an empty block; the local submission is then routed
/// through the mempool stage. The oracle is the first block plus the remaining mempool entry,
/// proving that selection has a stable boundary and does not mutate an already sealed body.
#[test]
fn world_forge_keeps_transaction_submitted_after_take() {
    let mut forge = ForgeWorld::new(Point::Origin);
    forge.due_lead();
    forge.world.run_until_horizon(20_000_000_000);
    let tx = create_transaction(11);
    let tx_id = tx.tx_id();
    forge.submit_and_settle(tx);
    forge.world.run_until_horizon(20_000_000_000);

    let header = forge.store.load_headers().next().expect("lead must forge an empty block");
    let block = forge.load_block(&header.hash());
    assert!(block.into_iter().next().is_none(), "late submission must not enter the sealed block");
    assert_eq!(forge.mempool.read().iter().map(HasTransactionId::tx_id).collect::<Vec<_>>(), vec![tx_id]);
}

/// `forge.invalidated_transaction`: revalidation happens before the next forge lead.
///
/// The transaction is admitted through the local submission stage, then the validator is
/// replaced with one that rejects every transaction and the production mempool stage receives
/// `NewTip`. The oracle is the mempool and the next stored block: the transaction must be evicted
/// before selection, and the resulting block must therefore be empty.
#[test]
fn world_forge_does_not_include_transaction_invalidated_by_new_tip() {
    let mut forge = ForgeWorld::new(Point::Origin);
    let tx = create_transaction(13);
    forge.submit_and_settle(tx);

    let validator: ResourceTxValidation = Arc::new(|_: &Transaction| {
        Err(amaru_ouroboros_traits::TransactionValidationError::from(anyhow::anyhow!(
            "transaction invalidated by test tip"
        )))
    });
    forge.world.graph(0).resources().put(validator);
    forge.world.enqueue_msg(0, &forge.mempool_stage, MempoolMsg::NewTip(Point::Origin));
    forge.world.run_until_horizon(1_000_000_000);
    assert!(forge.mempool.read().iter().next().is_none(), "revalidation must evict the invalid transaction");

    forge.due_lead();
    forge.world.run_until_horizon(20_000_000_000);

    let header = forge.store.load_headers().next().expect("revalidation must not stop forging");
    let block = forge.load_block(&header.hash());
    assert!(block.into_iter().next().is_none(), "an invalidated transaction must not be forged");
}

/// `forge.capacity_boundary`: transaction selection stops at an exact block constraint boundary.
///
/// The test matrix derives the limit from one real transaction, then submits a second transaction
/// with the same constrained resource. The oracle checks both the forged block and insertion-
/// ordered mempool, proving that `BlockBodyBuilder::try_push` applies each limit atomically without
/// dropping a transaction that did not fit.
#[test_case(CapacityDimension::Size; "serialized body size")]
#[test_case(CapacityDimension::ReferenceScript; "reference script size estimate")]
#[test_case(CapacityDimension::Memory; "execution memory units")]
#[test_case(CapacityDimension::Cpu; "execution cpu units")]
fn world_forge_stops_at_block_capacity(dimension: CapacityDimension) {
    let (first, second) = capacity_transactions(dimension);
    let constraints = capacity_constraints(dimension, &first);
    let mut forge =
        ForgeWorld::new_with_resource(Point::Origin, Arc::new(FixedConstraints { tip: Point::Origin, constraints }));
    let first_id = first.tx_id();
    let second_id = second.tx_id();

    forge.submit_and_settle(first);
    forge.submit_and_settle(second);
    forge.due_lead();
    forge.world.run_until_horizon(20_000_000_000);

    let header = forge.store.load_headers().next().expect("capacity-bound forge must publish a block");
    let block = forge.load_block(&header.hash());

    assert_eq!(block.into_iter().map(|(_, tx)| tx.tx_id()).collect::<Vec<_>>(), vec![first_id]);
    assert_eq!(forge.mempool.read().iter().map(HasTransactionId::tx_id).collect::<Vec<_>>(), vec![second_id]);
}

/// `forge.failure_after_take`: signing fails after the body has been removed from the mempool.
///
/// The credentials resource is removed only after admission, so the production stage reaches
/// `SignHeaderEffect` with a non-empty body. The oracle is deliberately limited to safety: no
/// header is stored and the forge stage terminates rather than publishing a partial block. The
/// current loss-of-service behavior is kept visible here until selection restoration is added.
#[test]
fn world_forge_signing_failure_publishes_no_partial_block() {
    let mut forge = ForgeWorld::new(Point::Origin);
    forge.submit_and_settle(create_transaction(16));
    forge.world.graph(0).resources().put::<ResourceForgingCredentials>(None);
    forge.due_lead();
    forge.world.run_until_horizon(20_000_000_000);

    assert!(forge.store.load_headers().next().is_none(), "signing failure must not publish a partial block");
    assert!(!forge.world.graph(0).contains_stage(forge.forge.name()), "signing failure must terminate forge stage");
    assert!(forge.mempool.read().iter().next().is_none(), "current take semantics remove the selected body");
}
