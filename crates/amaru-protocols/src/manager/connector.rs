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

//! Parallel outbound connection pool for the connection manager.
//!
//! The [`stage`] connector receives connect requests from the [`super::Manager`] and farms them
//! out to up to [`DEFAULT_PARALLEL_CONNECTION`] worker sub-stages. Workers perform the blocking
//! `connect` effect and report results back to the connector, which marks them idle and forwards
//! the result to the manager.
//!
//! Reconnect delays are applied at the connector (via `schedule_after`) so that a delayed request
//! does not occupy a worker slot.

use std::{collections::VecDeque, time::Duration};

use amaru_kernel::Peer;
use amaru_ouroboros::ConnectionId;
use amaru_pure_stage::{Effects, StageRef};

use super::ManagerMessage;
use crate::network_effects::{ConnectError, Network, NetworkOps};

/// Maximum number of concurrent outbound connection attempts.
pub const DEFAULT_PARALLEL_CONNECTION: usize = 10;

/// Messages handled by the connector stage.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ConnectorMsg {
    /// Request a connection attempt, optionally after `delay`.
    ///
    /// Sent by the manager (and re-enqueued by the connector itself after a reconnect delay).
    Connect { peer: Peer, delay: Duration },
    /// A worker finished a connection attempt and is idle again.
    WorkerDone { peer: Peer, result: Result<ConnectionId, ConnectError>, worker: StageRef<Peer> },
}

/// State of the connector stage: a pool of workers and a queue of pending peers.
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Connector {
    manager: StageRef<ManagerMessage>,
    connection_timeout: Duration,
    idle: Vec<StageRef<Peer>>,
    pending: VecDeque<Peer>,
    workers_created: usize,
}

impl Connector {
    pub fn new(connection_timeout: Duration, manager: StageRef<ManagerMessage>) -> Self {
        Self { manager, connection_timeout, idle: Vec::new(), pending: VecDeque::new(), workers_created: 0 }
    }

    async fn try_dispatch(&mut self, eff: &Effects<ConnectorMsg>) {
        while let Some(peer) = self.pending.pop_front() {
            if let Some(worker) = self.idle.pop() {
                eff.send(&worker, peer).await;
            } else if self.workers_created < DEFAULT_PARALLEL_CONNECTION {
                let name = format!("connect-worker-{}", self.workers_created);
                let build = eff.stage(name, worker_stage).await;
                let worker = eff.wire_up(build, Worker::new(eff.me(), self.connection_timeout)).await;
                self.workers_created += 1;
                eff.send(&worker, peer).await;
            } else {
                self.pending.push_front(peer);
                break;
            }
        }
    }
}

/// Connector stage: queues requests, manages the worker pool, and forwards results to the manager.
pub async fn stage(mut state: Connector, msg: ConnectorMsg, eff: Effects<ConnectorMsg>) -> Connector {
    match msg {
        ConnectorMsg::Connect { peer, delay } => {
            if delay > Duration::ZERO {
                eff.schedule_after(ConnectorMsg::Connect { peer, delay: Duration::ZERO }, delay).await;
                return state;
            }
            state.pending.push_back(peer);
            state.try_dispatch(&eff).await;
        }
        ConnectorMsg::WorkerDone { peer, result, worker } => {
            eff.send(&state.manager, ManagerMessage::ConnectionResult(peer, result)).await;
            state.idle.push(worker);
            state.try_dispatch(&eff).await;
        }
    }
    state
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct Worker {
    connector: StageRef<ConnectorMsg>,
    connection_timeout: Duration,
}

impl Worker {
    fn new(connector: StageRef<ConnectorMsg>, connection_timeout: Duration) -> Self {
        Self { connector, connection_timeout }
    }
}

async fn worker_stage(state: Worker, peer: Peer, eff: Effects<Peer>) -> Worker {
    let result = Network::new(&eff).connect(peer, state.connection_timeout).await;
    eff.send(&state.connector, ConnectorMsg::WorkerDone { peer, result, worker: eff.me() }).await;
    state
}

pub fn register_deserializers() -> amaru_pure_stage::DeserializerGuards {
    use amaru_pure_stage::register_data_deserializer;
    vec![
        register_data_deserializer::<Connector>().boxed(),
        register_data_deserializer::<ConnectorMsg>().boxed(),
        register_data_deserializer::<Worker>().boxed(),
    ]
}

#[cfg(test)]
mod tests {
    use amaru_pure_stage::{
        Effect, StageGraph,
        simulation::{Run, SimulationBuilder, running::OverrideResult},
        stage_ref::StageStateRef,
    };

    use super::*;
    use crate::network_effects::{ConnectEffect, ConnectError};

    fn setup_connector(
        timeout: Duration,
    ) -> (SimulationBuilder, StageStateRef<ConnectorMsg, Connector>, amaru_pure_stage::Receiver<ManagerMessage>) {
        let mut network = SimulationBuilder::default().with_mailbox_size(64);
        let (manager_out, rx) = network.output::<ManagerMessage>("manager", 64);
        let connector = network.stage("connector", stage);
        let connector = network.wire_up(connector, Connector::new(timeout, manager_out));
        (network, connector, rx)
    }

    #[test]
    fn runs_connects_in_parallel_up_to_pool_limit() {
        let timeout = Duration::from_secs(10);
        let (network, connector, _rx) = setup_connector(timeout);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut running = network.run(rt.handle());

        running.breakpoint(
            "connect",
            |eff| matches!(eff, Effect::External { effect, .. } if effect.is::<ConnectEffect>()),
        );

        let peers: Vec<_> = (0..DEFAULT_PARALLEL_CONNECTION + 1).map(|i| Peer::for_test(3000 + i as u16)).collect();

        for &peer in &peers {
            running.enqueue_msg(&connector, [ConnectorMsg::Connect { peer, delay: Duration::ZERO }]);
        }

        let mut workers = Vec::new();
        for _ in 0..DEFAULT_PARALLEL_CONNECTION {
            running.run(Run::skip_wakeups()).assert_breakpoint("connect");
            {
                let hit = running.breakpoint_effect();
                let Effect::External { at_stage, effect } = hit.effect() else {
                    panic!("expected ConnectEffect, got {:?}", hit.effect());
                };
                assert!(effect.is::<ConnectEffect>(), "expected ConnectEffect, got {effect:?}");
                workers.push(at_stage.clone());
            }
        }

        // Pool is full: connector queued the 11th peer; 10 workers are mid-connect.
        running
            .run(Run::skip_wakeups())
            .assert_busy((0..DEFAULT_PARALLEL_CONNECTION).map(|i| format!("connect-worker-{i}")));

        let state = running.get_state(&connector).expect("connector idle after dispatching");
        assert_eq!(state.pending.len(), 1);
        assert_eq!(state.pending.front(), Some(&peers[DEFAULT_PARALLEL_CONNECTION]));
        assert_eq!(state.workers_created, DEFAULT_PARALLEL_CONNECTION);
        assert!(state.idle.is_empty());
        assert_eq!(workers.len(), DEFAULT_PARALLEL_CONNECTION);

        // Complete one worker: the pending peer should get a worker and hit connect.
        let worker_name = workers.remove(0);
        running.complete_external(&worker_name, Ok::<ConnectionId, ConnectError>(ConnectionId::initial()));

        running.run(Run::skip_wakeups()).assert_breakpoint("connect");
        {
            let hit = running.breakpoint_effect();
            let Effect::External { at_stage, effect } = hit.effect() else {
                panic!("expected ConnectEffect, got {:?}", hit.effect());
            };
            let got = effect.cast_ref::<ConnectEffect>().expect("ConnectEffect");
            assert_eq!(got, &ConnectEffect { peer: peers[DEFAULT_PARALLEL_CONNECTION], timeout });
            assert!(at_stage.as_str().starts_with("connect-worker-"));
        }
    }

    #[test]
    fn delay_does_not_occupy_a_worker() {
        let timeout = Duration::from_secs(10);
        let (network, connector, _rx) = setup_connector(timeout);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut running = network.run(rt.handle());

        running.breakpoint(
            "connect",
            |eff| matches!(eff, Effect::External { effect, .. } if effect.is::<ConnectEffect>()),
        );

        let peer = Peer::for_test(3001);
        let delay = Duration::from_secs(2);
        running.enqueue_msg(&connector, [ConnectorMsg::Connect { peer, delay }]);

        // skip_wakeups advances sleep; default stops so the delay is visible.
        let sleeping_until = running.run(Run::default()).assert_sleeping();
        let state = running.get_state(&connector).expect("connector idle during delay");
        assert_eq!(state.workers_created, 0);
        assert!(state.pending.is_empty());
        assert!(state.idle.is_empty());

        assert!(running.skip_to_next_wakeup(Some(sleeping_until)), "expected to wake up connector");

        running.run(Run::skip_wakeups()).assert_breakpoint("connect");
        {
            let hit = running.breakpoint_effect();
            let Effect::External { at_stage, effect } = hit.effect() else {
                panic!("expected ConnectEffect, got {:?}", hit.effect());
            };
            assert!(at_stage.as_str().starts_with("connect-worker-0-"), "expected first worker, got {at_stage}");
            let got = effect.cast_ref::<ConnectEffect>().expect("ConnectEffect");
            assert_eq!(got, &ConnectEffect { peer, timeout });
        }
    }

    #[test]
    fn forwards_connection_result_to_manager() {
        let timeout = Duration::from_secs(10);
        let (network, connector, mut rx) = setup_connector(timeout);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut running = network.run(rt.handle());

        let conn_id = ConnectionId::initial();
        running.override_external_effect::<ConnectEffect>(usize::MAX, move |_| OverrideResult::handled(Ok(conn_id)));

        let peer = Peer::for_test(4000);
        running.enqueue_msg(&connector, [ConnectorMsg::Connect { peer, delay: Duration::ZERO }]);
        running.run(Run::skip_and_resolve()).assert_idle();

        let msgs: Vec<_> = rx.drain().collect();
        assert_eq!(msgs, vec![ManagerMessage::ConnectionResult(peer, Ok(conn_id))]);

        let state = running.get_state(&connector).unwrap();
        assert_eq!(state.idle.len(), 1);
        assert!(state.pending.is_empty());
        assert_eq!(state.workers_created, 1);
    }
}
