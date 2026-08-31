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

#![expect(clippy::expect_used, clippy::panic)]

use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet, BinaryHeap, VecDeque},
    io::ErrorKind,
    net::SocketAddr,
    num::NonZeroUsize,
    time::Duration,
};

use amaru_kernel::{HeaderHash, NonEmptyBytes, Peer};
use amaru_ouroboros::{ConnectionId, ConnectionProvider, ToSocketAddrs};
use amaru_pure_stage::BoxFuture;
use parking_lot::Mutex;
use tokio_util::bytes::{Bytes, BytesMut};

/// Inclusive one-way wire delay range for handshake hops (SYN / Accepted), in nanoseconds.
pub const WIRE_DELAY_MIN_NANOS: u64 = 1_000_000;
pub const WIRE_DELAY_MAX_NANOS: u64 = 5_000_000;

/// Per-send `Deliver` delay cap in slots on preprod (1s slots). An upper bound on one
/// honest payload hop. Not Praos Δ and not a claim that every honest inbox is reached.
pub const HONEST_PAYLOAD_DELAY_SLOTS: u64 = 5;

/// Nanosecond cap for one [`HONEST_PAYLOAD_DELAY_SLOTS`] payload hop on preprod.
pub const HONEST_PAYLOAD_DELAY_MAX_NANOS: u64 = HONEST_PAYLOAD_DELAY_SLOTS * 1_000_000_000;

/// Inclusive lower bound of the long-tail payload bucket (1s). Two orders of magnitude
/// above the 5ms hop.
pub const LONG_TAIL_PAYLOAD_MIN_NANOS: u64 = 1_000_000_000;

/// One in this many seeded samples is drawn from the long-tail bucket.
/// Rare enough that FIFO does not stall an epoch-sized catch-up on every handful of blocks.
pub const LONG_TAIL_PAYLOAD_EVERY: u64 = 1000;

/// Splitmix salt so disconnect *times* do not share the hop-delay sample stream.
const FAULT_TIME_SEED: u64 = 0xD15C_04EC;
/// Splitmix salt so disconnect *picks* do not share the hop-delay sample stream.
const FAULT_PICK_SEED: u64 = 0xC105_EED5;
/// Bounded redraws when a close adjacent pair is required.
const DISCONNECT_SCHEDULE_TRIES: u32 = 64;

fn disconnect_schedule_times(
    seed: u64,
    count: u32,
    earliest_nanos: u64,
    latest_nanos: u64,
    max_adjacent_gap_nanos: Option<u64>,
) -> Vec<u64> {
    let tries = if max_adjacent_gap_nanos.is_some() { DISCONNECT_SCHEDULE_TRIES } else { 1 };
    for try_idx in 0..tries {
        let times: Vec<u64> = (0..count)
            .map(|i| {
                delay_nanos(
                    seed.wrapping_add(FAULT_TIME_SEED)
                        .wrapping_add(u64::from(try_idx).wrapping_mul(0x9E3779B97F4A7C15)),
                    u64::from(i),
                    earliest_nanos,
                    latest_nanos,
                )
            })
            .collect();
        if let Some(gap) = max_adjacent_gap_nanos {
            let mut sorted = times.clone();
            sorted.sort_unstable();
            if sorted.windows(2).any(|pair| pair[1].saturating_sub(pair[0]) < gap) {
                return times;
            }
        } else {
            return times;
        }
    }
    panic!(
        "no disconnect schedule with an adjacent pair closer than {}ns in {DISCONNECT_SCHEDULE_TRIES} tries (count={count}, window=[{earliest_nanos}, {latest_nanos}])",
        max_adjacent_gap_nanos.expect("retries only when a gap is required"),
    );
}

/// Deterministic delay for sample `index` of `seed`, uniformly in `[min_nanos, max_nanos]`.
fn delay_nanos(seed: u64, index: u64, min_nanos: u64, max_nanos: u64) -> u64 {
    assert!(min_nanos <= max_nanos, "delay min ({min_nanos}) exceeds max ({max_nanos})");
    let mix = splitmix64(seed.wrapping_add(index.wrapping_mul(0x9E3779B97F4A7C15)));
    min_nanos + mix % (max_nanos - min_nanos + 1)
}

/// Deterministic delay for wire hop `index` of `seed`, uniformly in `[1ms, 5ms]`.
pub fn wire_delay_nanos(seed: u64, index: u64) -> u64 {
    delay_nanos(seed, index, WIRE_DELAY_MIN_NANOS, WIRE_DELAY_MAX_NANOS)
}

/// Long-tail payload delay: most samples stay in the 1–5ms hop; a seeded minority is
/// drawn from `[LONG_TAIL_PAYLOAD_MIN_NANOS, HONEST_PAYLOAD_DELAY_MAX_NANOS]`.
///
/// Uses the same `splitmix64` stream as the wire hop. Not uniform over `[1ms, 5s]`.
pub fn long_tail_payload_delay_nanos(seed: u64, index: u64) -> u64 {
    let mix = splitmix64(seed.wrapping_add(index.wrapping_mul(0x9E3779B97F4A7C15)));
    let (min_nanos, max_nanos) = if (mix >> 32).is_multiple_of(LONG_TAIL_PAYLOAD_EVERY) {
        (LONG_TAIL_PAYLOAD_MIN_NANOS, HONEST_PAYLOAD_DELAY_MAX_NANOS)
    } else {
        (WIRE_DELAY_MIN_NANOS, WIRE_DELAY_MAX_NANOS)
    };
    min_nanos + mix % (max_nanos - min_nanos + 1)
}

fn splitmix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E3779B97F4A7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

/// Discrete-event network simulator for deterministic testing.
///
/// Owns the one physical `(time, sequence)` heap: network hops and graph wakes.
/// [`super::WorldLoop`] is the only popper. Provider methods enqueue onto that
/// heap and return a Future that the loop completes via `resume_external_box` /
/// `provide_external_result`. There is no oneshot table and the provider never
/// wakes a peer.
///
/// Cloning is the use-site's job (`Arc<WorldConnectionProvider>`).
pub struct WorldConnectionProvider {
    inner: Mutex<WorldInner>,
}

impl Default for WorldConnectionProvider {
    fn default() -> Self {
        Self::new(0)
    }
}

/// Event types scheduled on the heap.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum NetworkEvent {
    /// Completes a parked `accept()` via the world loop.
    Accepted { listener: SocketAddr, responder_conn: ConnectionId, initiator_addr: SocketAddr },
    /// SYN arrival for an outbound `connect()`. Listener is checked only when this pops.
    ConnectAttempt { target: SocketAddr, id: u64 },
    /// Fails a parked `connect()` that has not completed by its timeout.
    ConnectTimeout { target: SocketAddr, id: u64 },
    /// Completes a parked `send()` via the world loop.
    SendAck { conn: ConnectionId },
    /// Delivers bytes to `conn`'s inbox and may complete a parked `recv()`.
    Deliver { conn: ConnectionId, data: Bytes },
    /// Closes `conn`. The world loop also heap-schedules `Close` for the peer.
    Close { conn: ConnectionId },
    /// Fault: close one live pair, chosen when this hop pops.
    PeerDisconnect,
}

/// Heap tokens for one `connect()`: the SYN hop and its deadline.
#[derive(Debug, Clone, Copy)]
pub(super) struct ScheduledConnect {
    pub id: u64,
    pub attempt_seq: u64,
    pub timeout_seq: u64,
}

/// First-class unified heap item: a delayed network event or a graph wake.
///
/// Ordered by `(time_nanos, sequence)` so Wait/ready-now and wire hops share one order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct WorldHeapEntry {
    pub time_nanos: u64,
    pub sequence: u64,
    pub item: WorldHeapItem,
}

/// Payload of a [`WorldHeapEntry`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum WorldHeapItem {
    Network(NetworkEvent),
    Graph {
        index: usize,
        reason: GraphWakeReason,
    },
    /// Copy one inventory hash into the injector serving store and enqueue `NewTip`.
    Reveal {
        hash: HeaderHash,
    },
}

/// Logged form of a popped heap event. `Copy` so the log never owns heap data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeapLogEntry {
    pub sequence: u64,
    pub time_nanos: u64,
    pub kind: HeapLogKind,
}

/// Why a graph was placed on the unified heap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum GraphWakeReason {
    /// `has_runnable()` at this time — wake immediately (`now`).
    Runnable,
    /// Sleeping until `next_wakeup`.
    Sleeping,
}

/// Everything from [`NetworkEvent`] except `Deliver`'s payload (kept as `data_len`),
/// plus first-class graph wakes so tests can assert heap interleaving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeapLogKind {
    Accepted { listener: SocketAddr, responder_conn: ConnectionId, initiator_addr: SocketAddr },
    ConnectAttempt { target: SocketAddr },
    ConnectTimeout { target: SocketAddr },
    SendAck { conn: ConnectionId },
    Deliver { conn: ConnectionId, data_len: usize },
    Close { conn: ConnectionId },
    PeerDisconnect,
    Reveal { hash: HeaderHash },
    GraphWake { graph: usize, reason: GraphWakeReason },
}

impl From<&WorldHeapEntry> for HeapLogEntry {
    fn from(entry: &WorldHeapEntry) -> Self {
        HeapLogEntry {
            sequence: entry.sequence,
            time_nanos: entry.time_nanos,
            kind: match &entry.item {
                WorldHeapItem::Network(event) => match event {
                    NetworkEvent::Accepted { listener, responder_conn, initiator_addr } => HeapLogKind::Accepted {
                        listener: *listener,
                        responder_conn: *responder_conn,
                        initiator_addr: *initiator_addr,
                    },
                    NetworkEvent::ConnectAttempt { target, id: _ } => HeapLogKind::ConnectAttempt { target: *target },
                    NetworkEvent::ConnectTimeout { target, id: _ } => HeapLogKind::ConnectTimeout { target: *target },
                    NetworkEvent::SendAck { conn } => HeapLogKind::SendAck { conn: *conn },
                    NetworkEvent::Deliver { conn, data } => HeapLogKind::Deliver { conn: *conn, data_len: data.len() },
                    NetworkEvent::Close { conn } => HeapLogKind::Close { conn: *conn },
                    NetworkEvent::PeerDisconnect => HeapLogKind::PeerDisconnect,
                },
                WorldHeapItem::Graph { index, reason } => HeapLogKind::GraphWake { graph: *index, reason: *reason },
                WorldHeapItem::Reveal { hash } => HeapLogKind::Reveal { hash: *hash },
            },
        }
    }
}

/// How `Deliver` delays are drawn. Handshake hops stay `[1ms, 5ms]` either way.
enum PayloadDelay {
    Uniform { min_nanos: u64, max_nanos: u64 },
    LongTail,
}

struct WorldInner {
    heap: BinaryHeap<Reverse<WorldHeapEntry>>,
    next_sequence: u64,
    current_time_nanos: u64,
    seed: u64,
    latency_samples: u64,
    payload_delay: PayloadDelay,
    listeners: BTreeMap<SocketAddr, Listener>,
    endpoints: BTreeMap<ConnectionId, ConnectionEndpoint>,
    /// Last scheduled `Deliver` arrival on each destination. One connection is FIFO:
    /// hop delay may be long-tail, but a later send cannot pass an earlier one.
    last_deliver_at: BTreeMap<ConnectionId, u64>,
    next_conn_id: ConnectionId,
    next_connect_id: u64,
    last_scheduled_connect: Option<ScheduledConnect>,
    disconnect_picks: u64,
    faulted_conns: BTreeSet<ConnectionId>,
}

struct Listener {
    pending_handshakes: VecDeque<PendingHandshake>,
}

struct PendingHandshake {
    responder_conn: ConnectionId,
    initiator_addr: SocketAddr,
}

struct ConnectionEndpoint {
    inbox: VecDeque<Bytes>,
    read_buffer: BytesMut,
    peer_conn_id: ConnectionId,
}

impl WorldConnectionProvider {
    pub fn new(seed: u64) -> Self {
        Self::with_payload_delay(seed, WIRE_DELAY_MIN_NANOS, WIRE_DELAY_MAX_NANOS)
    }

    /// Build a world whose honest payloads are delayed uniformly in `[min_nanos, max_nanos]`.
    /// Handshake hops stay `[1ms, 5ms]`. Default [`Self::new`] keeps the 1–5ms hop for both
    /// so existing tests stay in that band.
    pub fn with_payload_delay(seed: u64, min_nanos: u64, max_nanos: u64) -> Self {
        assert!(min_nanos <= max_nanos, "payload delay min ({min_nanos}) exceeds max ({max_nanos})");
        Self::with_delay(seed, PayloadDelay::Uniform { min_nanos, max_nanos })
    }

    /// Opt in to a long-tail honest payload delay. Most `Deliver`s stay in the 1–5ms hop;
    /// a seeded minority is much later, capped at [`HONEST_PAYLOAD_DELAY_MAX_NANOS`].
    /// Handshake hops stay `[1ms, 5ms]`.
    pub fn with_long_tail_payload_delay(seed: u64) -> Self {
        Self::with_delay(seed, PayloadDelay::LongTail)
    }

    fn with_delay(seed: u64, payload_delay: PayloadDelay) -> Self {
        Self {
            inner: Mutex::new(WorldInner {
                heap: BinaryHeap::new(),
                next_sequence: 0,
                current_time_nanos: 0,
                seed,
                latency_samples: 0,
                payload_delay,
                listeners: BTreeMap::new(),
                endpoints: BTreeMap::new(),
                last_deliver_at: BTreeMap::new(),
                next_conn_id: ConnectionId::initial(),
                next_connect_id: 0,
                last_scheduled_connect: None,
                disconnect_picks: 0,
                faulted_conns: BTreeSet::new(),
            }),
        }
    }

    /// Advance simulated time to the given instant (in nanoseconds).
    pub(super) fn set_time(&self, time_nanos: u64) {
        let mut inner = self.inner.lock();
        assert!(time_nanos >= inner.current_time_nanos, "time cannot go backward");
        inner.current_time_nanos = time_nanos;
    }

    /// Get current simulated time in nanoseconds.
    pub(super) fn current_time_nanos(&self) -> u64 {
        self.inner.lock().current_time_nanos
    }

    /// Pop the next heap item at-or-before the horizon. [`super::WorldLoop`] is the only caller.
    pub(super) fn pop_at_or_before(&self, horizon_nanos: u64) -> Option<WorldHeapEntry> {
        let mut inner = self.inner.lock();
        let Reverse(first) = inner.heap.peek()?;
        if first.time_nanos > horizon_nanos {
            return None;
        }
        inner.heap.pop().map(|Reverse(entry)| entry)
    }

    /// Peek at the next heap time without popping.
    pub(super) fn peek_next_event_time(&self) -> Option<u64> {
        self.inner.lock().heap.peek().map(|Reverse(e)| e.time_nanos)
    }

    /// Live heap entries (not pop order). [`super::WorldLoop`] filters cancelled wakes.
    pub(super) fn heap_entries(&self) -> Vec<WorldHeapEntry> {
        self.inner.lock().heap.iter().map(|Reverse(entry)| entry.clone()).collect()
    }

    /// Enqueue a network hop or graph wake onto the one physical heap.
    pub(super) fn schedule_item(&self, time_nanos: u64, item: WorldHeapItem) -> u64 {
        let mut inner = self.inner.lock();
        schedule_item_locked(&mut inner, time_nanos, item)
    }

    /// Manually schedule an event at a specific time (for testing).
    pub fn schedule_event_at(&self, time_nanos: u64, event: NetworkEvent) {
        let mut inner = self.inner.lock();
        schedule_event_locked(&mut inner, time_nanos, event);
    }

    /// Schedule an event at current_time + delta_nanos.
    pub fn schedule_event(&self, delta_nanos: u64, event: NetworkEvent) {
        let mut inner = self.inner.lock();
        let time_nanos = inner.current_time_nanos + delta_nanos;
        schedule_event_locked(&mut inner, time_nanos, event);
    }

    /// Schedule a one-way wire hop at `now + delay` (`delay` ∈ `[1ms, 5ms]`).
    pub(super) fn schedule_wire(&self, event: NetworkEvent) {
        let mut inner = self.inner.lock();
        schedule_wire_locked(&mut inner, event);
    }

    /// Schedule an honest payload at `now + delay` (configured payload distribution).
    pub fn schedule_payload(&self, event: NetworkEvent) {
        let mut inner = self.inner.lock();
        schedule_payload_locked(&mut inner, event);
    }

    /// Pair a connect that has arrived at `target` if a listener is bound there.
    pub(super) fn pair_if_listening(&self, target: SocketAddr) -> Option<ConnectionId> {
        let mut inner = self.inner.lock();
        inner.listeners.contains_key(&target).then(|| pair_connect_locked(&mut inner, target))
    }

    /// Add data to an endpoint inbox (Deliver).
    pub(super) fn deliver_to_inbox(&self, conn: ConnectionId, data: Bytes) {
        let mut inner = self.inner.lock();
        if let Some(endpoint) = inner.endpoints.get_mut(&conn) {
            endpoint.inbox.push_back(data);
        }
    }

    /// Try to complete recv from buffered inbox data.
    ///
    /// Returns `Some` when enough bytes are available, or when the connection is gone
    /// (`connection reset`). Otherwise leaves unread bytes in the buffer so a later
    /// Deliver can finish the same recv.
    pub(super) fn try_complete_recv(
        &self,
        conn: ConnectionId,
        bytes_needed: NonZeroUsize,
    ) -> Option<std::io::Result<NonEmptyBytes>> {
        let mut inner = self.inner.lock();
        try_complete_recv_locked(&mut inner, conn, bytes_needed)
    }

    /// Pop a queued handshake and return its ids. Both endpoints are already installed at pair time.
    pub(super) fn take_handshake(&self, listener: SocketAddr) -> Option<(ConnectionId, SocketAddr)> {
        let mut inner = self.inner.lock();
        install_handshake_locked(&mut inner, listener)
    }

    /// Remove a closed endpoint and return the peer id if that side is still live.
    pub(super) fn close_endpoint(&self, conn: ConnectionId) -> Option<ConnectionId> {
        let mut inner = self.inner.lock();
        let endpoint = inner.endpoints.remove(&conn)?;
        inner.last_deliver_at.remove(&conn);
        inner.endpoints.contains_key(&endpoint.peer_conn_id).then_some(endpoint.peer_conn_id)
    }

    /// True when `conn` exists and its peer endpoint is still installed.
    pub(super) fn can_send(&self, conn: ConnectionId) -> bool {
        let inner = self.inner.lock();
        let Some(endpoint) = inner.endpoints.get(&conn) else {
            return false;
        };
        inner.endpoints.contains_key(&endpoint.peer_conn_id)
    }

    /// Take the hop tokens for the most recently posted `connect()`, if any.
    pub(super) fn take_last_scheduled_connect(&self) -> Option<ScheduledConnect> {
        self.inner.lock().last_scheduled_connect.take()
    }

    /// Place `count` [`NetworkEvent::PeerDisconnect`] hops uniformly in `[earliest, latest]`.
    ///
    /// Times use a dedicated splitmix stream so they do not consume wire-delay samples.
    /// When `max_adjacent_gap_nanos` is set, the whole schedule is redrawn until two adjacent
    /// times fall inside that gap, or a bounded number of tries is exhausted.
    pub fn schedule_peer_disconnects(
        &self,
        count: u32,
        earliest_nanos: u64,
        latest_nanos: u64,
        max_adjacent_gap_nanos: Option<u64>,
    ) {
        assert!(
            earliest_nanos <= latest_nanos,
            "disconnect window min ({earliest_nanos}) exceeds max ({latest_nanos})"
        );
        if max_adjacent_gap_nanos.is_some() {
            assert!(count >= 2, "need at least two disconnects to require a close pair");
        }

        let mut inner = self.inner.lock();
        let times = disconnect_schedule_times(inner.seed, count, earliest_nanos, latest_nanos, max_adjacent_gap_nanos);
        for at in times {
            schedule_event_locked(&mut inner, at, NetworkEvent::PeerDisconnect);
        }
    }

    /// Choose a live endpoint that has not already been faulted this run.
    ///
    /// Marks both ends of the pair so a later hop does not close the same connection twice.
    pub(super) fn pick_live_connection_for_fault(&self) -> Option<ConnectionId> {
        let mut inner = self.inner.lock();
        let candidates: Vec<ConnectionId> =
            inner.endpoints.keys().copied().filter(|id| !inner.faulted_conns.contains(id)).collect();
        if candidates.is_empty() {
            return None;
        }
        let mix = splitmix64(
            inner
                .seed
                .wrapping_add(FAULT_PICK_SEED)
                .wrapping_add(inner.disconnect_picks.wrapping_mul(0x9E3779B97F4A7C15)),
        );
        inner.disconnect_picks += 1;
        let chosen = candidates[(mix as usize) % candidates.len()];
        inner.faulted_conns.insert(chosen);
        if let Some(peer) = inner.endpoints.get(&chosen).map(|endpoint| endpoint.peer_conn_id) {
            inner.faulted_conns.insert(peer);
        }
        Some(chosen)
    }
}

fn alloc_sequence_locked(inner: &mut WorldInner) -> u64 {
    let sequence = inner.next_sequence;
    inner.next_sequence += 1;
    sequence
}

fn schedule_item_locked(inner: &mut WorldInner, time_nanos: u64, item: WorldHeapItem) -> u64 {
    let sequence = alloc_sequence_locked(inner);
    inner.heap.push(Reverse(WorldHeapEntry { time_nanos, sequence, item }));
    sequence
}

fn schedule_event_locked(inner: &mut WorldInner, time_nanos: u64, event: NetworkEvent) -> u64 {
    schedule_item_locked(inner, time_nanos, WorldHeapItem::Network(event))
}

fn schedule_delayed_locked(inner: &mut WorldInner, min_nanos: u64, max_nanos: u64, event: NetworkEvent) -> u64 {
    let delay = delay_nanos(inner.seed, inner.latency_samples, min_nanos, max_nanos);
    inner.latency_samples += 1;
    let time_nanos = inner.current_time_nanos + delay;
    schedule_event_locked(inner, time_nanos, event)
}

fn schedule_wire_locked(inner: &mut WorldInner, event: NetworkEvent) -> u64 {
    schedule_delayed_locked(inner, WIRE_DELAY_MIN_NANOS, WIRE_DELAY_MAX_NANOS, event)
}

fn schedule_payload_locked(inner: &mut WorldInner, event: NetworkEvent) {
    let delay = match inner.payload_delay {
        PayloadDelay::Uniform { min_nanos, max_nanos } => {
            delay_nanos(inner.seed, inner.latency_samples, min_nanos, max_nanos)
        }
        PayloadDelay::LongTail => long_tail_payload_delay_nanos(inner.seed, inner.latency_samples),
    };
    inner.latency_samples += 1;
    let hop = inner.current_time_nanos + delay;
    // Long-tail is a hop setting, not a license to reorder. One destination inbox is FIFO.
    let time_nanos = match &event {
        NetworkEvent::Deliver { conn, .. } => {
            let arrive = inner.last_deliver_at.get(conn).copied().map_or(hop, |last| hop.max(last));
            inner.last_deliver_at.insert(*conn, arrive);
            arrive
        }
        NetworkEvent::Accepted { .. }
        | NetworkEvent::ConnectAttempt { .. }
        | NetworkEvent::ConnectTimeout { .. }
        | NetworkEvent::SendAck { .. }
        | NetworkEvent::Close { .. }
        | NetworkEvent::PeerDisconnect => hop,
    };
    schedule_event_locked(inner, time_nanos, event);
}

fn try_complete_recv_locked(
    inner: &mut WorldInner,
    conn: ConnectionId,
    bytes_needed: NonZeroUsize,
) -> Option<std::io::Result<NonEmptyBytes>> {
    let Some(endpoint) = inner.endpoints.get_mut(&conn) else {
        return Some(Err(std::io::Error::new(ErrorKind::ConnectionReset, "connection reset")));
    };

    while let Some(data) = endpoint.inbox.pop_front() {
        endpoint.read_buffer.extend_from_slice(&data);
    }

    if endpoint.read_buffer.len() >= bytes_needed.get() {
        let bytes = endpoint.read_buffer.split_to(bytes_needed.get()).freeze();
        Some(NonEmptyBytes::try_from(bytes).map_err(|_| std::io::Error::other("empty bytes")))
    } else {
        None
    }
}

fn install_handshake_locked(inner: &mut WorldInner, listener: SocketAddr) -> Option<(ConnectionId, SocketAddr)> {
    let handshake = inner.listeners.get_mut(&listener)?.pending_handshakes.pop_front()?;
    Some((handshake.responder_conn, handshake.initiator_addr))
}

/// Pair an arrived connect with an existing listener.
/// Both endpoints (and both `peer_conn_id`s) are installed here so a send after
/// connect completion can Deliver before accept takes the handshake.
fn pair_connect_locked(inner: &mut WorldInner, target_addr: SocketAddr) -> ConnectionId {
    let initiator_conn = inner.next_conn_id.get_and_increment();
    let responder_conn = inner.next_conn_id.get_and_increment();

    inner.endpoints.insert(
        initiator_conn,
        ConnectionEndpoint {
            inbox: VecDeque::new(),
            read_buffer: BytesMut::with_capacity(65536),
            peer_conn_id: responder_conn,
        },
    );
    inner.endpoints.insert(
        responder_conn,
        ConnectionEndpoint {
            inbox: VecDeque::new(),
            read_buffer: BytesMut::with_capacity(65536),
            peer_conn_id: initiator_conn,
        },
    );

    let listener = inner.listeners.get_mut(&target_addr).expect("pair_connect requires a listener");
    listener.pending_handshakes.push_back(PendingHandshake {
        responder_conn,
        initiator_addr: SocketAddr::from(([127, 0, 0, 1], 5000 + initiator_conn.as_u64() as u16)),
    });

    initiator_conn
}

impl ConnectionProvider for WorldConnectionProvider {
    fn listen(&self, addr: SocketAddr) -> BoxFuture<'static, std::io::Result<SocketAddr>> {
        let mut inner = self.inner.lock();
        if inner.listeners.contains_key(&addr) {
            drop(inner);
            return Box::pin(async move { Err(std::io::Error::new(ErrorKind::AddrInUse, "address already in use")) });
        }
        inner.listeners.insert(addr, Listener { pending_handshakes: VecDeque::new() });
        drop(inner);
        Box::pin(async move { Ok(addr) })
    }

    fn accept(&self, listener_addr: SocketAddr) -> BoxFuture<'static, std::io::Result<(Peer, ConnectionId)>> {
        let mut inner = self.inner.lock();
        if let Some((responder_conn, initiator_addr)) = install_handshake_locked(&mut inner, listener_addr) {
            schedule_wire_locked(
                &mut inner,
                NetworkEvent::Accepted { listener: listener_addr, responder_conn, initiator_addr },
            );
        }
        drop(inner);
        Box::pin(std::future::pending())
    }

    fn connect(&self, addrs: Vec<SocketAddr>, timeout: Duration) -> BoxFuture<'static, std::io::Result<ConnectionId>> {
        let mut inner = self.inner.lock();
        inner.last_scheduled_connect = None;
        if let Some(target) = addrs.first().copied() {
            let id = inner.next_connect_id;
            inner.next_connect_id += 1;
            let attempt_seq = schedule_wire_locked(&mut inner, NetworkEvent::ConnectAttempt { target, id });
            let timeout_nanos = u64::try_from(timeout.as_nanos()).unwrap_or(u64::MAX);
            let timeout_at = inner.current_time_nanos.saturating_add(timeout_nanos);
            let timeout_seq =
                schedule_event_locked(&mut inner, timeout_at, NetworkEvent::ConnectTimeout { target, id });
            inner.last_scheduled_connect = Some(ScheduledConnect { id, attempt_seq, timeout_seq });
        }
        drop(inner);
        Box::pin(std::future::pending())
    }

    fn connect_addrs(
        &self,
        addr: ToSocketAddrs,
        timeout: Duration,
    ) -> BoxFuture<'static, std::io::Result<ConnectionId>> {
        match addr.to_socket_addrs() {
            Ok(addrs) => self.connect(addrs, timeout),
            Err(e) => {
                let msg = e.to_string();
                Box::pin(async move { Err(std::io::Error::other(msg)) })
            }
        }
    }

    fn send(&self, conn: ConnectionId, data: NonEmptyBytes) -> BoxFuture<'static, std::io::Result<()>> {
        let mut inner = self.inner.lock();
        let peer_live =
            inner.endpoints.get(&conn).is_some_and(|endpoint| inner.endpoints.contains_key(&endpoint.peer_conn_id));
        if peer_live {
            let peer_id = inner.endpoints[&conn].peer_conn_id;
            let time_nanos = inner.current_time_nanos;
            schedule_event_locked(&mut inner, time_nanos, NetworkEvent::SendAck { conn });
            schedule_payload_locked(
                &mut inner,
                NetworkEvent::Deliver { conn: peer_id, data: Bytes::copy_from_slice(&data) },
            );
        }
        drop(inner);
        Box::pin(std::future::pending())
    }

    fn recv(&self, _conn: ConnectionId, _bytes: NonZeroUsize) -> BoxFuture<'static, std::io::Result<NonEmptyBytes>> {
        Box::pin(std::future::pending())
    }

    fn close(&self, conn: ConnectionId) -> BoxFuture<'static, std::io::Result<()>> {
        self.schedule_event(0, NetworkEvent::Close { conn });
        Box::pin(async move { Ok(()) })
    }
}
