// Copyright 2025 PRAGMA
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
    collections::VecDeque,
    fmt::{Debug, Formatter},
    sync::Arc,
};

use amaru_consensus::headers_tree::data_generation::Action;
use amaru_protocols::{manager::ManagerMessage, mux::HandlerMessage, protocol::PROTO_N2N_CHAIN_SYNC};
use futures_util::FutureExt;
use parking_lot::Mutex;
use pure_stage::{
    Effect, Instant, Resources, StageGraphRunning, StageRef,
    simulation::{Blocked, SimulationRunning},
    trace_buffer::TraceBuffer,
};

use crate::tests::{
    configuration::{NodeTestConfig, NodeType, SimulationEvent},
    setup::TxInjectMessage,
};

/// A node with its identifier for logging purposes.
///
/// The setup of the node depends on its configuration.
///
/// The node contains:
///
///  - A node id to uniquely identify that node in the logs and traces.
///  - A running stage graph containing the currently running execution graph for the node.
///  - An access point to the manager stage to possibly inject `ManagerMessages`
///  - An access point to the actions stage to inject generated actions if that node is an upstream node.
///  - A list of pending actions to inject in the node during a test, if this node is a peer driving the test
///    (and not the node under test or a downstream node).
///
pub struct Node {
    config: NodeTestConfig,
    running: SimulationRunning,
    manager_stage: StageRef<ManagerMessage>,
    actions_stage: StageRef<Action>,
    tx_inject_stage: StageRef<TxInjectMessage>,
    pending_events: VecDeque<SimulationEvent>,
    initialized: bool,
}

impl Node {
    /// Create a new node, ready to be executed.
    pub fn new(
        config: NodeTestConfig,
        running: SimulationRunning,
        manager_stage: StageRef<ManagerMessage>,
        actions_stage: StageRef<Action>,
        tx_inject_stage: StageRef<TxInjectMessage>,
    ) -> Self {
        // If the config defines some events to execute, we store them as pending for now.
        let events = config.events.clone();

        let mut node = Self {
            config,
            running,
            manager_stage,
            actions_stage,
            tx_inject_stage,
            pending_events: VecDeque::from(events),
            initialized: false,
        };
        node.install_breakpoint_for_initialization();

        node
    }

    /// This function installs a breakpoint that will be triggered when the node is initialized.
    /// We currently consider that it is initialized if the chainsync protocol has been registered.
    fn install_breakpoint_for_initialization(&mut self) {
        self.running.breakpoint("chainsync_registered", move |eff| {
            if let Effect::Send { msg, .. } = eff {
                if let Ok(handler_msg) = msg.cast_ref::<HandlerMessage>() {
                    matches!(handler_msg, HandlerMessage::Registered(proto) if *proto == PROTO_N2N_CHAIN_SYNC.erase() || *proto == PROTO_N2N_CHAIN_SYNC.responder().erase())
                } else {
                    false
                }
            } else {
                false
            }
        });
    }

    /// Run the node until it is blocked (waiting for external effects for example)
    /// If it is blocked because we reached the initialization breakpoint we set the node as initialized.
    #[expect(clippy::panic)]
    pub fn run_until_blocked(&mut self) {
        let _span = self.enter_span();
        match self.running.run_until_blocked() {
            Blocked::Breakpoint(name, effect) => {
                if name.as_str() == "chainsync_registered" {
                    tracing::info!("Node {} chainsync registered", self.node_id());
                    self.initialized = true
                }
                self.running.clear_breakpoint("chainsync_registered");
                self.running.handle_effect(effect);
            }
            Blocked::Sleeping { .. } => {
                panic!("Node {} should not be sleeping", self.node_id());
            }
            Blocked::Deadlock(_) => {
                panic!("Deadlock detected during initialization");
            }
            Blocked::Idle | Blocked::Busy { .. } => {}
            Blocked::Terminated(_) => {}
        }
    }

    /// Run an effect on the node if possible.
    #[expect(clippy::panic)]
    pub fn run_effect(&mut self) {
        let _span = self.enter_span();
        match self.running.try_effect() {
            Ok(effect) => {
                self.running.handle_effect(effect);
            }
            Err(Blocked::Sleeping { .. }) => {
                // Advance clock to next wakeup
                self.running.skip_to_next_wakeup(None);
            }
            Err(Blocked::Idle) | Err(Blocked::Busy { .. }) | Err(Blocked::Terminated(_)) => {}
            Err(Blocked::Deadlock(_)) => {
                panic!("Deadlock detected");
            }
            Err(Blocked::Breakpoint(name, ..)) => {
                // A breakpoint might have been set but is not handled. Warn the user.
                tracing::warn!("The breakpoint {name} is not handled");
            }
        }
    }

    /// Await for external effects or the arrival of a new input message.
    pub fn advance_inputs(&mut self) {
        let _span = self.enter_span();
        self.running.await_external_effect().now_or_never();
        self.running.receive_inputs();
    }

    /// Return true when the node is initialized
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Return true if the node is terminated now
    pub fn is_terminated(&self) -> bool {
        self.running.is_terminated()
    }

    /// Pop one pending action and enqueue it for execution
    pub fn enqueue_pending_event(&mut self) {
        let Some(event) = self.pending_events.pop_front() else {
            return;
        };

        match event {
            SimulationEvent::PeerAction(action) => {
                self.running.enqueue_msg(&self.actions_stage, [action]);
            }
            SimulationEvent::InjectTx(tx) => {
                self.running.enqueue_msg(&self.tx_inject_stage, [TxInjectMessage::InjectTx(tx)]);
            }
        }
    }

    /// Enqueue a message for the Manager stage
    pub fn enqueue_manager_message(&mut self, msg: ManagerMessage) {
        self.running.enqueue_msg(&self.manager_stage, [msg]);
    }

    /// The node is identified by its listen_address for now.
    pub fn node_id(&self) -> &str {
        &self.config.listen_address
    }

    /// Return the resources used by the node in the simulation.
    pub fn resources(&self) -> Resources {
        self.running.resources().clone()
    }

    /// Return the trace buffer used by the node in the simulation.
    pub fn trace_buffer(&self) -> Arc<Mutex<TraceBuffer>> {
        self.config.trace_buffer.clone()
    }

    /// Return true if the node still has pending events to enqueue.
    /// or enqueued actions in the actions_stage mailbox.
    pub fn has_waiting_events(&self) -> bool {
        !self.pending_events.is_empty() || self.running.mailbox_len(&self.actions_stage) > 0
    }

    /// Return true if the node still has runnable effects.
    pub fn has_runnable_effects(&self) -> bool {
        self.running.has_runnable()
    }

    /// Return the next wakeup time for this node, if any.
    pub fn next_wakeup(&self) -> Option<Instant> {
        self.running.next_wakeup()
    }

    /// Advance this node to the given wakeup time and wake any tasks scheduled for it.
    pub fn advance_to_wakeup(&mut self, at: Instant) -> bool {
        self.running.skip_to_next_wakeup(Some(at))
    }

    /// Return true for the node under test
    pub fn is_node_under_test(&self) -> bool {
        self.config.node_type == NodeType::NodeUnderTest
    }

    /// Return true for an upstream node
    pub fn is_upstream(&self) -> bool {
        self.config.node_type == NodeType::UpstreamNode
    }

    /// Return true for a downstream node
    pub fn is_downstream(&self) -> bool {
        self.config.node_type == NodeType::DownstreamNode
    }

    /// Enter a tracing span with this node's identifier for logging purposes.
    fn enter_span(&self) -> tracing::span::EnteredSpan {
        tracing::info_span!("node", id = %self.node_id()).entered()
    }
}

impl Debug for Node {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Node").field("node_id", &self.node_id()).field("config", &self.config).finish()
    }
}
