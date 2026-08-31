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

#![expect(clippy::panic, clippy::expect_used)]

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    future::Future,
    net::SocketAddr,
    num::NonZeroUsize,
    sync::Arc,
    task::{Context, Waker},
    time::Duration,
};

use amaru_kernel::{HeaderHash, Peer};
use amaru_ouroboros::{ConnectionId, ToSocketAddrs};
use amaru_protocols::{
    manager::ManagerMessage,
    network_effects::{
        AcceptEffect, AcceptError, ConnectEffect, ConnectError, ReceiveError, RecvEffect, SendEffect, SendError,
    },
};
use amaru_pure_stage::{
    Effect, Instant, Name, SendData,
    simulation::{Blocked, SimulationRunning},
    trace_buffer::TraceEntry,
};

use super::{
    GraphWakeReason, HeapLogEntry, InjectorShared, NetworkEvent, WorldConnectionProvider,
    world_connection_provider::WorldHeapItem,
};

/// World loop: pops the one physical `(time, sequence)` heap.
///
/// The heap lives on [`WorldConnectionProvider`]. Network hops enqueue there;
/// graph wakes are scheduled onto the same structure so `(time, sequence)` is
/// global. [`SimulationRunning`] bodies live in `graphs` by index. That Vec is
/// not a scheduler — a graph runs only when its wake is popped.
/// Completes Network UntilResolved effects only via `resume_external_box`.
pub struct WorldLoop {
    provider: Arc<WorldConnectionProvider>,
    graphs: Vec<SimulationRunning>,
    heap_log: Vec<HeapLogEntry>,
    /// Sequences of graph wakes superseded by an earlier reschedule.
    cancelled: BTreeSet<u64>,
    /// Current heap token `(time, sequence)` per graph, if scheduled.
    graph_on_heap: Vec<Option<(u64, u64)>>,
    /// Pending connects keyed by destination listener, not a process-wide FIFO.
    pending_connects: BTreeMap<SocketAddr, VecDeque<PendingConnect>>,
    pending_accepts: BTreeMap<SocketAddr, VecDeque<(usize, Name)>>,
    /// Accepts already paired on `ConnectAttempt` and waiting for the matching `Accepted` hop.
    claimed_accepts: BTreeMap<SocketAddr, VecDeque<(usize, Name)>>,
    pending_sends: BTreeMap<ConnectionId, VecDeque<(usize, Name)>>,
    pending_recvs: BTreeMap<ConnectionId, VecDeque<(usize, Name, NonZeroUsize)>>,
    /// Stages observed via [`Blocked::Terminated`] (and aborted children in that graph's trace).
    ///
    /// Keyed by `(graph_idx, Name)` so two `build_node` graphs can share mux/recv names.
    terminated_stages: BTreeSet<(usize, Name)>,
    /// Serve-only injector and the graph index it occupies. Owned here, not on [`SimulationRunning`].
    injector: Option<(usize, Arc<InjectorShared>)>,
    /// Fragment hashes waiting to become [`WorldHeapItem::Reveal`] hops.
    pending_reveals: VecDeque<HeaderHash>,
    /// True while a [`WorldHeapItem::Reveal`] is already on the heap.
    reveal_scheduled: bool,
}

type Completion = (usize, Name, Box<dyn SendData>);

struct PendingConnect {
    graph_idx: usize,
    stage: Name,
    id: u64,
    attempt_seq: u64,
    timeout_seq: u64,
}

enum Posted {
    Connect { stage: Name, addr: ToSocketAddrs },
    Accept { stage: Name, listener: SocketAddr },
    Send { stage: Name, conn: ConnectionId },
    Recv { stage: Name, conn: ConnectionId, bytes: NonZeroUsize },
}

impl WorldLoop {
    pub fn new(provider: Arc<WorldConnectionProvider>, mut graphs: Vec<SimulationRunning>) -> Self {
        for graph in &mut graphs {
            graph.breakpoint("world_external", |effect| matches!(effect, Effect::External { .. }));
        }
        let graph_on_heap = vec![None; graphs.len()];
        let mut world = Self {
            provider,
            graphs,
            heap_log: Vec::new(),
            cancelled: BTreeSet::new(),
            graph_on_heap,
            pending_connects: BTreeMap::new(),
            pending_accepts: BTreeMap::new(),
            claimed_accepts: BTreeMap::new(),
            pending_sends: BTreeMap::new(),
            pending_recvs: BTreeMap::new(),
            terminated_stages: BTreeSet::new(),
            injector: None,
            pending_reveals: VecDeque::new(),
            reveal_scheduled: false,
        };
        for index in 0..world.graphs.len() {
            world.schedule_graph_if_needed(index);
        }
        world
    }

    pub fn graph(&self, index: usize) -> &SimulationRunning {
        &self.graphs[index]
    }

    /// Borrow the node graphs owned by this world.
    pub fn graphs(&self) -> &[SimulationRunning] {
        &self.graphs
    }

    /// Attach the serve-only injector handle this loop owns, at `graph_index` in `graphs`.
    pub fn with_injector(mut self, graph_index: usize, injector: Arc<InjectorShared>) -> Self {
        self.injector = Some((graph_index, injector));
        self
    }

    /// Number of inventory blocks the injector scanned at construction.
    pub fn inventory_len(&self) -> usize {
        self.injector.as_ref().map(|(_, injector)| injector.inventory_len()).unwrap_or(0)
    }

    /// Reveal inventory through `hash` and advertise that tip to the injector manager.
    pub fn reveal(&mut self, hash: HeaderHash) -> anyhow::Result<()> {
        let (graph_idx, injector) = self.injector.clone().ok_or_else(|| anyhow::anyhow!("world has no injector"))?;
        let point = injector.reveal_through(hash)?;
        let manager = injector.manager();
        self.graphs[graph_idx].enqueue_msg(&manager, [ManagerMessage::new_tip(point)]);
        self.schedule_graph_if_needed(graph_idx);
        Ok(())
    }

    /// Queue fragment hashes as Reveal hops, paced by the injector mailbox.
    pub fn schedule_reveals(&mut self, hashes: impl IntoIterator<Item = HeaderHash>) {
        self.pending_reveals.extend(hashes);
        self.kick_pending_reveal();
    }

    /// Place at most one Reveal on the heap, and only when the injector mailbox has room.
    fn kick_pending_reveal(&mut self) {
        if self.reveal_scheduled {
            return;
        }
        let Some((graph_idx, injector)) = self.injector.clone() else {
            return;
        };
        if self.pending_reveals.is_empty() {
            return;
        }
        let manager = injector.manager();
        if self.graphs[graph_idx].mailbox_len(&manager) >= self.graphs[graph_idx].mailbox_size() {
            return;
        }
        let hash = self.pending_reveals.pop_front().expect("non-empty");
        let now = self.provider.current_time_nanos();
        self.provider.schedule_item(now, WorldHeapItem::Reveal { hash });
        self.reveal_scheduled = true;
    }

    /// Run until no more heap events or graph wakes at-or-before horizon.
    ///
    /// Synchronous: the loop never waits on wall-clock time. Production graphs may
    /// `Handle::block_on` `DurationDist::Zero` effects, which cannot run inside an
    /// existing Tokio context.
    pub fn run_until_horizon(&mut self, horizon_nanos: u64) {
        self.run_until_horizon_with(horizon_nanos, Duration::MAX, |_| {});
    }

    /// Like [`Self::run_until_horizon`], calling `progress` at start, then every
    /// `progress_every` of wall time, then once more when the horizon is reached.
    pub fn run_until_horizon_with(
        &mut self,
        horizon_nanos: u64,
        progress_every: Duration,
        mut progress: impl FnMut(&Self),
    ) {
        let mut last_progress = std::time::Instant::now();
        progress(self);
        while let Some(entry) = self.provider.pop_at_or_before(horizon_nanos) {
            if self.cancelled.remove(&entry.sequence) {
                continue;
            }

            self.provider.set_time(entry.time_nanos);
            self.heap_log.push(HeapLogEntry::from(&entry));

            match entry.item {
                WorldHeapItem::Network(event) => {
                    let completions = self.completions_for_event(&event);
                    for completion in completions {
                        let graph_idx = completion.0;
                        self.resume(completion);
                        self.schedule_graph_if_needed(graph_idx);
                    }
                }
                WorldHeapItem::Graph { index, reason: _ } => {
                    self.graph_on_heap[index] = None;
                    self.wake_and_run_graph(index);
                    self.schedule_graph_if_needed(index);
                }
                WorldHeapItem::Reveal { hash } => {
                    self.reveal_scheduled = false;
                    self.reveal(hash).unwrap_or_else(|e| panic!("scheduled reveal {hash}: {e}"));
                }
            }
            self.kick_pending_reveal();

            let now = std::time::Instant::now();
            if now.saturating_duration_since(last_progress) >= progress_every {
                progress(self);
                last_progress = now;
            }
        }
        progress(self);
    }

    fn schedule_graph_if_needed(&mut self, index: usize) {
        let graph = &mut self.graphs[index];
        graph.receive_inputs();
        let now = self.provider.current_time_nanos();
        let (time_nanos, reason) = if graph.has_runnable() {
            (now, GraphWakeReason::Runnable)
        } else if let Some(wakeup) = graph.next_wakeup() {
            (instant_nanos(wakeup), GraphWakeReason::Sleeping)
        } else {
            return;
        };
        self.schedule_graph(index, time_nanos, reason);
    }

    fn schedule_graph(&mut self, index: usize, time_nanos: u64, reason: GraphWakeReason) {
        if let Some((old_time, old_seq)) = self.graph_on_heap[index] {
            if old_time <= time_nanos {
                return;
            }
            self.cancelled.insert(old_seq);
        }
        let sequence = self.provider.schedule_item(time_nanos, WorldHeapItem::Graph { index, reason });
        self.graph_on_heap[index] = Some((time_nanos, sequence));
    }

    fn wake_and_run_graph(&mut self, index: usize) {
        let time_nanos = self.provider.current_time_nanos();
        let graph = &mut self.graphs[index];
        // Instant Ord uses duration_since_global_epoch. A zero-offset max_time
        // misses waits scheduled with the graph's epoch offset and reschedules
        // the same Sleeping wake forever.
        let graph_now = graph.now();
        let instant = graph_now - graph_now.sim_elapsed() + Duration::from_nanos(time_nanos);
        let clock_behind = instant_nanos(graph_now) < time_nanos;
        let wakeup_due = graph.next_wakeup().is_some_and(|t| instant_nanos(t) <= time_nanos);
        if clock_behind || wakeup_due {
            graph.skip_to_next_wakeup(Some(instant));
        }
        self.run_graph_until_clock(index);
    }

    /// Run until the graph wants to advance the clock. External effects fall out via the breakpoint.
    fn run_graph_until_clock(&mut self, index: usize) {
        loop {
            match self.graphs[index].run_until_sleeping_or_blocked() {
                Blocked::Breakpoint(_, effect) => {
                    self.on_external(index, effect);
                }
                Blocked::Deadlock(deadlock) => {
                    panic!("graph {index} deadlock: {deadlock:?}");
                }
                Blocked::Terminated(name) => {
                    self.drop_pending_for_terminated(index, &name);
                    break;
                }
                Blocked::Idle | Blocked::Sleeping { .. } | Blocked::Busy { .. } => {
                    break;
                }
            }
        }
    }

    fn on_external(&mut self, graph_idx: usize, effect: Effect) {
        let posted = classify_network(&effect);
        if let Some(Blocked::Terminated(name)) = self.graphs[graph_idx].handle_effect(effect) {
            self.drop_pending_for_terminated(graph_idx, &name);
        }
        kick_external(&mut self.graphs[graph_idx]);
        if let Some(posted) = posted {
            self.track_or_complete(graph_idx, posted);
        }
    }

    fn track_or_complete(&mut self, graph_idx: usize, posted: Posted) {
        match posted {
            Posted::Connect { stage, addr } => {
                let addrs = addr.clone().to_socket_addrs().unwrap_or_default();
                if let Some(target) = addrs.first().copied() {
                    let scheduled = self.provider.take_last_scheduled_connect().expect("connect scheduled a hop");
                    self.pending_connects.entry(target).or_default().push_back(PendingConnect {
                        graph_idx,
                        stage,
                        id: scheduled.id,
                        attempt_seq: scheduled.attempt_seq,
                        timeout_seq: scheduled.timeout_seq,
                    });
                } else {
                    self.resume((
                        graph_idx,
                        stage,
                        Box::new(Err::<ConnectionId, ConnectError>(ConnectError::new(addr, "connection refused")))
                            as Box<dyn SendData>,
                    ));
                }
            }
            Posted::Accept { stage, listener } => {
                self.pending_accepts.entry(listener).or_default().push_back((graph_idx, stage));
            }
            Posted::Send { stage, conn } => {
                if self.provider.can_send(conn) {
                    self.pending_sends.entry(conn).or_default().push_back((graph_idx, stage));
                } else {
                    self.resume((
                        graph_idx,
                        stage,
                        Box::new(Err::<(), SendError>(SendError::new(conn, "connection reset"))) as Box<dyn SendData>,
                    ));
                }
            }
            Posted::Recv { stage, conn, bytes } => {
                self.pending_recvs.entry(conn).or_default().push_back((graph_idx, stage, bytes));
                for completion in self.drain_recvs(conn) {
                    self.resume(completion);
                }
            }
        }
    }

    fn take_pending_connect(&mut self, listener: SocketAddr, id: u64) -> Option<PendingConnect> {
        let queue = self.pending_connects.get_mut(&listener)?;
        let idx = queue.iter().position(|pending| pending.id == id)?;
        let item = queue.remove(idx)?;
        if queue.is_empty() {
            self.pending_connects.remove(&listener);
        }
        Some(item)
    }

    fn drain_recvs(&mut self, conn: ConnectionId) -> Vec<Completion> {
        let mut out = Vec::new();
        while let Some((graph_idx, stage, bytes_needed)) =
            self.pending_recvs.get(&conn).and_then(|q| q.front().cloned())
        {
            match self.provider.try_complete_recv(conn, bytes_needed) {
                Some(result) => {
                    if let Some(queue) = self.pending_recvs.get_mut(&conn) {
                        queue.pop_front();
                    }
                    if self.pending_recvs.get(&conn).is_some_and(|q| q.is_empty()) {
                        self.pending_recvs.remove(&conn);
                    }
                    out.push((
                        graph_idx,
                        stage,
                        Box::new(result.map_err(|e| ReceiveError::new(conn, e))) as Box<dyn SendData>,
                    ));
                }
                None => break,
            }
        }
        out
    }

    fn completions_for_event(&mut self, event: &NetworkEvent) -> Vec<Completion> {
        match event {
            NetworkEvent::ConnectAttempt { target, id } => {
                let Some(pending) = self.take_pending_connect(*target, *id) else {
                    return Vec::new();
                };
                self.cancelled.insert(pending.timeout_seq);
                if let Some(initiator_conn) = self.provider.pair_if_listening(*target) {
                    // Pair at most one queued accept per ConnectAttempt. Extra inbound
                    // handshakes stay queued until a later accept() posts.
                    if let Some(waiting) = self.pending_accepts.get_mut(target).and_then(|q| q.pop_front()) {
                        if self.pending_accepts.get(target).is_some_and(|q| q.is_empty()) {
                            self.pending_accepts.remove(target);
                        }
                        let (responder_conn, initiator_addr) =
                            self.provider.take_handshake(*target).expect("pair_connect queues a handshake");
                        self.claimed_accepts.entry(*target).or_default().push_back(waiting);
                        self.provider.schedule_wire(NetworkEvent::Accepted {
                            listener: *target,
                            responder_conn,
                            initiator_addr,
                        });
                    }
                    vec![(
                        pending.graph_idx,
                        pending.stage,
                        Box::new(Ok::<ConnectionId, ConnectError>(initiator_conn)) as Box<dyn SendData>,
                    )]
                } else {
                    vec![(
                        pending.graph_idx,
                        pending.stage,
                        Box::new(Err::<ConnectionId, ConnectError>(ConnectError::new(
                            (*target).into(),
                            "connection refused",
                        ))) as Box<dyn SendData>,
                    )]
                }
            }
            NetworkEvent::ConnectTimeout { target, id } => {
                let Some(pending) = self.take_pending_connect(*target, *id) else {
                    return Vec::new();
                };
                self.cancelled.insert(pending.attempt_seq);
                vec![(
                    pending.graph_idx,
                    pending.stage,
                    Box::new(Err::<ConnectionId, ConnectError>(ConnectError::new((*target).into(), "timed out")))
                        as Box<dyn SendData>,
                )]
            }
            NetworkEvent::Accepted { listener, responder_conn, initiator_addr } => {
                let waiting = self
                    .claimed_accepts
                    .get_mut(listener)
                    .and_then(|q| q.pop_front())
                    .or_else(|| self.pending_accepts.get_mut(listener).and_then(|q| q.pop_front()));
                if self.claimed_accepts.get(listener).is_some_and(|q| q.is_empty()) {
                    self.claimed_accepts.remove(listener);
                }
                if self.pending_accepts.get(listener).is_some_and(|q| q.is_empty()) {
                    self.pending_accepts.remove(listener);
                }
                if let Some((graph_idx, stage_name)) = waiting {
                    let peer = Peer::from_addr(initiator_addr);
                    vec![(
                        graph_idx,
                        stage_name,
                        Box::new(Ok::<_, AcceptError>((peer, *responder_conn))) as Box<dyn SendData>,
                    )]
                } else {
                    Vec::new()
                }
            }
            NetworkEvent::SendAck { conn } => {
                if let Some((graph_idx, stage_name)) = self.pending_sends.get_mut(conn).and_then(|q| q.pop_front()) {
                    if self.pending_sends.get(conn).is_some_and(|q| q.is_empty()) {
                        self.pending_sends.remove(conn);
                    }
                    vec![(graph_idx, stage_name, Box::new(Ok::<(), SendError>(())) as Box<dyn SendData>)]
                } else {
                    Vec::new()
                }
            }
            NetworkEvent::Deliver { conn, data } => {
                self.provider.deliver_to_inbox(*conn, data.clone());
                self.drain_recvs(*conn)
            }
            NetworkEvent::Close { conn } => {
                let peer = self.provider.close_endpoint(*conn);
                let out = fail_pending_on(&mut self.pending_sends, &mut self.pending_recvs, *conn);
                if let Some(peer) = peer {
                    self.provider.schedule_wire(NetworkEvent::Close { conn: peer });
                }
                out
            }
            NetworkEvent::PeerDisconnect => match self.provider.pick_live_connection_for_fault() {
                Some(conn) => self.completions_for_event(&NetworkEvent::Close { conn }),
                None => Vec::new(),
            },
        }
    }

    fn resume(&mut self, (graph_idx, stage_name, result): Completion) {
        if self.terminated_stages.contains(&(graph_idx, stage_name.clone())) {
            return;
        }
        // Supervised children terminate with a tombstone to the parent, not
        // `Blocked::Terminated`. A later hop must not resume a removed stage.
        if !self.graphs[graph_idx].contains_stage(&stage_name) {
            self.terminated_stages.insert((graph_idx, stage_name));
            self.drop_matching_pending();
            return;
        }
        self.graphs[graph_idx]
            .resume_external_box(&stage_name, result)
            .unwrap_or_else(|e| panic!("failed to resume stage {stage_name}: {e}"));
        kick_external(&mut self.graphs[graph_idx]);
    }

    /// Cancel queued network completions for a stage we have already seen terminate.
    ///
    /// Aborted children are taken from this graph's trace (`push_terminated`), not a
    /// world-global name. Heap events may still pop and log. They must not resume a gone stage.
    fn drop_pending_for_terminated(&mut self, graph_idx: usize, stage: &Name) {
        self.terminated_stages.insert((graph_idx, stage.clone()));
        let from_trace = self.graphs[graph_idx].trace_buffer().lock().hydrate_without_timestamps();
        for entry in from_trace {
            if let TraceEntry::Terminated { stage, .. } = entry {
                self.terminated_stages.insert((graph_idx, stage));
            }
        }
        self.drop_matching_pending();
    }

    fn drop_matching_pending(&mut self) {
        let gone = &self.terminated_stages;
        let mut cancel = Vec::new();
        self.pending_connects.retain(|_, queue| {
            queue.retain(|pending| {
                if gone.contains(&(pending.graph_idx, pending.stage.clone())) {
                    cancel.push(pending.attempt_seq);
                    cancel.push(pending.timeout_seq);
                    false
                } else {
                    true
                }
            });
            !queue.is_empty()
        });
        self.cancelled.extend(cancel);
        drop_stages_from_pending(&mut self.pending_accepts, gone);
        drop_stages_from_pending(&mut self.claimed_accepts, gone);
        drop_stages_from_pending(&mut self.pending_sends, gone);
        self.pending_recvs.retain(|_, queue| {
            queue.retain(|(graph_idx, name, _)| !gone.contains(&(*graph_idx, name.clone())));
            !queue.is_empty()
        });
    }

    /// Run until no more events and all graphs idle/terminated.
    pub fn run_to_completion(&mut self) {
        self.run_until_horizon(u64::MAX);
        self.assert_graphs_settled();
    }

    /// A serve-only injector stays parked on `accept` (immediate re-PullAccept).
    /// That is Busy, not Idle — the listen loop is the product.
    pub fn assert_serving_accept(&mut self, graph_idx: usize) {
        match self.graphs[graph_idx].run_until_sleeping_or_blocked() {
            Blocked::Busy { stages, .. } if stages.iter().any(|name| format!("{name}").contains("accept")) => {}
            other @ (Blocked::Idle
            | Blocked::Sleeping { .. }
            | Blocked::Deadlock(_)
            | Blocked::Breakpoint(..)
            | Blocked::Busy { .. }
            | Blocked::Terminated(_)) => {
                panic!("graph {graph_idx} expected parked accept, got {other:?}")
            }
        }
    }

    fn assert_graphs_settled(&mut self) {
        for (graph_idx, graph) in self.graphs.iter_mut().enumerate() {
            match graph.run_until_sleeping_or_blocked() {
                Blocked::Idle | Blocked::Terminated(_) => {}
                other @ (Blocked::Sleeping { .. }
                | Blocked::Deadlock(_)
                | Blocked::Breakpoint(..)
                | Blocked::Busy { .. }) => {
                    panic!("graph {graph_idx} expected idle/terminated, got {other:?}")
                }
            }
        }
    }

    /// Simulated time of the last popped heap item (nanoseconds).
    pub fn now_nanos(&self) -> u64 {
        self.provider.current_time_nanos()
    }

    /// Get the event log (network events and graph wakes, in pop order).
    pub fn heap_log(&self) -> Vec<HeapLogEntry> {
        self.heap_log.clone()
    }

    /// Borrow the event log without cloning.
    pub fn heap_log_ref(&self) -> &[HeapLogEntry] {
        &self.heap_log
    }

    /// Number of heap events popped so far (network hops and graph wakes).
    pub fn heap_len(&self) -> usize {
        self.heap_log.len()
    }

    /// Take the event log, leaving it empty.
    pub fn take_heap_log(&mut self) -> Vec<HeapLogEntry> {
        std::mem::take(&mut self.heap_log)
    }

    /// Peek next event time on the one physical heap.
    pub fn peek_next_event_time(&self) -> Option<u64> {
        self.provider.peek_next_event_time()
    }

    /// Live heap contents (not pop order), excluding cancelled graph wakes.
    ///
    /// Sorted by `(time, sequence)` so tests can assert a graph wake and a
    /// `NetworkEvent` share one heap before the loop pops either.
    pub fn heap_contents(&self) -> Vec<HeapLogEntry> {
        let mut entries: Vec<_> = self
            .provider
            .heap_entries()
            .iter()
            .filter(|entry| !self.cancelled.contains(&entry.sequence))
            .map(HeapLogEntry::from)
            .collect();
        entries.sort_by_key(|e| (e.time_nanos, e.sequence));
        entries
    }
}

fn instant_nanos(instant: Instant) -> u64 {
    u64::try_from(instant.sim_elapsed().as_nanos()).expect("sim time fits u64")
}

fn classify_network(effect: &Effect) -> Option<Posted> {
    use std::any::Any;

    let Effect::External { at_stage, effect: eff } = effect else {
        return None;
    };
    let eff_any = &**eff as &dyn Any;
    if let Some(connect) = eff_any.downcast_ref::<ConnectEffect>() {
        Some(Posted::Connect { stage: at_stage.clone(), addr: connect.addr.clone() })
    } else if let Some(accept) = eff_any.downcast_ref::<AcceptEffect>() {
        Some(Posted::Accept { stage: at_stage.clone(), listener: accept.listener_addr })
    } else if let Some(send) = eff_any.downcast_ref::<SendEffect>() {
        Some(Posted::Send { stage: at_stage.clone(), conn: send.conn })
    } else {
        eff_any.downcast_ref::<RecvEffect>().map(|recv| Posted::Recv {
            stage: at_stage.clone(),
            conn: recv.conn,
            bytes: recv.bytes,
        })
    }
}

fn drop_stages_from_pending<K: Ord>(
    pending: &mut BTreeMap<K, VecDeque<(usize, Name)>>,
    gone: &BTreeSet<(usize, Name)>,
) {
    pending.retain(|_, queue| {
        queue.retain(|(graph_idx, name)| !gone.contains(&(*graph_idx, name.clone())));
        !queue.is_empty()
    });
}

fn fail_pending_on(
    pending_sends: &mut BTreeMap<ConnectionId, VecDeque<(usize, Name)>>,
    pending_recvs: &mut BTreeMap<ConnectionId, VecDeque<(usize, Name, NonZeroUsize)>>,
    conn: ConnectionId,
) -> Vec<Completion> {
    let mut out = Vec::new();
    if let Some(queue) = pending_sends.remove(&conn) {
        for (graph_idx, stage_name) in queue {
            out.push((
                graph_idx,
                stage_name,
                Box::new(Err::<(), SendError>(SendError::new(conn, "connection closed"))) as Box<dyn SendData>,
            ));
        }
    }
    if let Some(queue) = pending_recvs.remove(&conn) {
        for (graph_idx, stage_name, _) in queue {
            out.push((
                graph_idx,
                stage_name,
                Box::new(Err::<amaru_kernel::NonEmptyBytes, ReceiveError>(ReceiveError::new(conn, "connection closed")))
                    as Box<dyn SendData>,
            ));
        }
    }
    out
}

/// Poll `await_external_effect` once so provider methods run and Ready
/// futures (Listen, Close) complete. Never waits.
fn kick_external(graph: &mut SimulationRunning) {
    let mut fut = std::pin::pin!(graph.await_external_effect());
    let _ = Future::poll(fut.as_mut(), &mut Context::from_waker(Waker::noop()));
}
