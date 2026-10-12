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

use std::{sync::Arc, time::Duration};

use amaru_consensus::{
    block_validator::{BlockValidator, LedgerThreadStop},
    performance::Performance,
    stages::select_chain::SelectChainMsg,
};
use amaru_kernel::{Point, Slot};
use amaru_ouroboros::ChainStore;
use amaru_protocols::manager::ManagerMessage;
use amaru_pure_stage::{
    BoxFuture, Receiver, Sender, StageGraph, StageGraphRunning,
    tokio::{TokioBuilder, TokioRunning},
    trace_buffer::TraceBuffer,
};
use anyhow::anyhow;
use tokio::runtime::Handle;

use crate::{
    ShutdownError, ShutdownReport,
    chain_realign::resolve_stored_point,
    mithril::{MithrilBlockEvent, MithrilBlockSource, MithrilInput, mithril_block_source},
    stage_runtime::{register_ledger_resources, shutdown_stages, wire_validation_stages},
    stages::{
        build_node::{make_block_validator, make_chain_store, make_state},
        config::Config,
    },
};

struct ReplayLifecycle {
    ledger_thread: LedgerThreadStop,
    performance: Box<dyn FnOnce() -> std::thread::Result<()> + Send + Sync>,
}

/// Ledger and stores prepared for immutable replay.
pub(super) struct PreparedReplay {
    pub ledger_tip: Point,
    pub block_validator: Arc<BlockValidator>,
}

/// Open stores, prepare the replay anchor, and install validation resources and workers.
pub(super) fn prepare_replay(
    config: &Config,
    stage_graph: &mut impl StageGraph,
    prepare_chain: impl FnOnce(&dyn ChainStore, Point) -> anyhow::Result<()>,
) -> anyhow::Result<PreparedReplay> {
    let chain_store = make_chain_store(config)?;
    let mut state = make_state(&config.ledger_config, None, chain_store.clone())?;
    state.set_observers(config.observers.clone());
    let ledger_tip = state.tip().into_owned();
    prepare_chain(chain_store.as_ref(), ledger_tip)?;
    let ledger_tip =
        resolve_stored_point(chain_store.as_ref(), ledger_tip.into()).ok_or(anyhow!("ledger tip header not found"))?;
    let pool_summaries = state.pool_summaries();
    let ledger_config = &config.ledger_config;
    let block_validator = Arc::new(make_block_validator(ledger_config, state, chain_store.clone())?);
    register_ledger_resources(
        stage_graph,
        chain_store,
        pool_summaries,
        block_validator.clone(),
        Arc::new(ledger_config.to_consensus_parameters()),
        ledger_config.era_history.clone(),
    );
    let ledger_thread = block_validator.thread_stop();
    stage_graph.resources().put(ledger_thread.clone());
    let performance = Performance::new();
    let join_performance = Box::new(performance.install(stage_graph.resources()));
    stage_graph.resources().put(ReplayLifecycle { ledger_thread, performance: join_performance });
    Ok(PreparedReplay { ledger_tip, block_validator })
}

/// Launch immutable replay with the node's clock, trace buffer, and worker shutdown.
pub(super) fn launch_replay<T>(
    config: &Config,
    runtime: &Handle,
    build: impl FnOnce(&mut TokioBuilder) -> anyhow::Result<T>,
) -> anyhow::Result<(T, RunningReplay)> {
    let trace_buffer = TraceBuffer::new_shared(config.trace_buffer_min_entries, config.trace_buffer_max_size);
    let mut stage_builder = TokioBuilder::default()
        .with_global_epoch_offset(config.compute_global_clock_offset())
        .with_trace_buffer(trace_buffer);
    let stages = build(&mut stage_builder)?;
    let lifecycle = stage_builder.resources().take::<ReplayLifecycle>()?;
    Ok((stages, RunningReplay { tokio_running: stage_builder.run(runtime.clone()), lifecycle }))
}

/// Owns the immutable replay graph and its ledger and performance workers.
pub(super) struct RunningReplay {
    tokio_running: TokioRunning,
    lifecycle: ReplayLifecycle,
}

impl RunningReplay {
    pub(super) fn termination(&self) -> BoxFuture<'static, ()> {
        self.tokio_running.termination()
    }

    pub(super) async fn shutdown(self) -> Result<ShutdownReport, ShutdownError> {
        self.shutdown_with_timeout(Duration::from_secs(10)).await
    }

    pub(super) async fn shutdown_with_timeout(self, timeout: Duration) -> Result<ShutdownReport, ShutdownError> {
        let Self { tokio_running, lifecycle } = self;
        let ReplayLifecycle { ledger_thread, performance } = lifecycle;
        shutdown_stages(tokio_running, ledger_thread, None, performance, timeout).await
    }
}

/// Attach a Mithril source to the same validation and adoption stages as the live node.
#[expect(clippy::wildcard_enum_match_arm)]
pub(super) fn wire_mithril_stages(
    stage_graph: &mut impl StageGraph,
    ledger_tip: Point,
    security_param: u64,
    until_slot: Option<Slot>,
) -> (Sender<MithrilInput>, Receiver<MithrilBlockEvent>) {
    let (events_ref, events) = stage_graph.output::<MithrilBlockEvent>("mithril_events", 2);
    let source = stage_graph.stage("mithril_blocks", mithril_block_source);
    let manager = source.sender().contramap(|message: ManagerMessage| match message {
        ManagerMessage::NewTip(point, _) => MithrilInput::Adopted(point),
        other => MithrilInput::Unexpected(format!("unexpected manager message: {other:?}")),
    });
    let validate_block = wire_validation_stages(
        stage_graph,
        ledger_tip,
        security_param,
        manager,
        None,
        None,
        source.sender().contramap(|message: SelectChainMsg| match message {
            SelectChainMsg::BlockValidationResult(point, valid, ..) => MithrilInput::Validation { point, valid },
            other => MithrilInput::Unexpected(format!("unexpected validation message: {other:?}")),
        }),
        None,
    );
    let source = stage_graph.wire_up(
        source,
        MithrilBlockSource { validate_block, events: events_ref, tip: ledger_tip, until_slot, pending: None },
    );
    (stage_graph.input(source), events)
}
