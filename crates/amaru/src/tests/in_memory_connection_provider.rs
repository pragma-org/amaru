// Copyright 2024 PRAGMA
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
    collections::{BTreeMap, VecDeque},
    net::SocketAddr,
    num::NonZeroUsize,
    sync::{Arc, atomic::AtomicU16},
    task::{Poll, Waker},
    time::Duration,
};

use amaru_kernel::{NonEmptyBytes, Peer};
use amaru_ouroboros::{ConnectionId, ConnectionProvider, ToSocketAddrs};
use parking_lot::Mutex;
use pure_stage::BoxFuture;
use tokio_util::bytes::{Buf, Bytes, BytesMut};

/// A connection provider that uses in-memory channels instead of TCP sockets.
/// This implementation uses synchronous data structures with wakers to notify callers expecting
/// to connect or receive a message for example.
///
/// This provider can be shared between several StageGraph implementations representing different
/// Amaru nodes communicating together.
#[derive(Clone, Default)]
pub struct InMemoryConnectionProvider {
    inner: Arc<Inner>,
}

impl ConnectionProvider for InMemoryConnectionProvider {
    /// Create a new listener for the given address.
    /// Fails if the address is already in use (this is a configuration error).
    fn listen(&self, addr: SocketAddr) -> BoxFuture<'static, std::io::Result<SocketAddr>> {
        let inner = self.inner.clone();
        let provider = self.clone();
        Box::pin(async move {
            tracing::debug!("InMemoryConnectionProvider::listen for {addr}");
            {
                let mut listeners = inner.listeners.lock();

                if listeners.contains_key(&addr) {
                    return Err(std::io::Error::other(format!("listener already bound to {addr}")));
                }

                listeners.insert(addr, Listener { pending_connects: VecDeque::new() });
            }

            // Wake any connect wakers waiting for this address
            provider.wake_connect_wakers(&addr);

            tracing::debug!("listener bound to {addr}");
            Ok(addr)
        })
    }

    /// Accept an incoming connection for the specified listener. Returns the peer and connection ID.
    /// If no peer is currently trying to connect, this waits until a connection is available
    /// and wakes the caller.
    fn accept(&self, listener_addr: SocketAddr) -> BoxFuture<'static, std::io::Result<(Peer, ConnectionId)>> {
        let inner = self.inner.clone();
        Box::pin(std::future::poll_fn(move |cx| {
            // Try to get a pending connection from the specified listener only
            let pending = {
                let mut listeners = inner.listeners.lock();
                listeners.get_mut(&listener_addr).and_then(|l| l.pending_connects.pop_front())
            };

            let Some(pending) = pending else {
                // No pending connections, register waker and return Pending
                inner.accept_wakers.lock().push(cx.waker().clone());
                return Poll::Pending;
            };

            // Register the responder's endpoint, the initiator is already registered in connect().
            let conn_id = inner.register_endpoint(pending.responder_endpoint);

            // Set the initiator's peer_conn_id to the responder's conn_id
            *pending.initiator_peer_conn_id_slot.lock() = Some(conn_id);

            tracing::debug!("accepted in-memory connection from {} with id {conn_id}", pending.initiator_addr);
            Poll::Ready(Ok((Peer::from_addr(&pending.initiator_addr), conn_id)))
        }))
    }

    /// Connect to one of the given addresses. If a listener for the target address is already registered,
    /// the connection is established immediately.
    ///
    /// This creates 2 connection endpoints with shared read / write queues.
    ///
    fn connect(&self, addr: Vec<SocketAddr>, _timeout: Duration) -> BoxFuture<'static, std::io::Result<ConnectionId>> {
        let inner = self.inner.clone();
        let provider = self.clone();
        tracing::debug!("InMemoryConnectionProvider::connect called for {addr:?}");

        Box::pin(std::future::poll_fn(move |cx| {
            let Some(target_addr) = addr.first() else {
                return Poll::Ready(Err(std::io::Error::other("no addresses provided")));
            };

            // Check if the listener exists before allocating anything
            {
                let listeners = inner.listeners.lock();
                if !listeners.contains_key(target_addr) {
                    inner.connect_wakers.lock().entry(*target_addr).or_default().push(cx.waker().clone());
                    return Poll::Pending;
                }
            }

            // Create bidirectional channel pair using VecDeque
            let initiator_to_responder = Arc::new(Mutex::new(VecDeque::new()));
            let responder_to_initiator = Arc::new(Mutex::new(VecDeque::new()));

            // Create shared peer_conn_id slots for linking
            let initiator_peer_conn_id_slot = Arc::new(Mutex::new(None));
            let responder_peer_conn_id_slot = Arc::new(Mutex::new(None));

            // Create initiator endpoint
            let initiator_endpoint = ConnectionEndpoint {
                read_queue: responder_to_initiator.clone(),
                write_queue: initiator_to_responder.clone(),
                peer_conn_id: initiator_peer_conn_id_slot.clone(),
                ..Default::default()
            };

            // Create responder endpoint
            let responder_endpoint = ConnectionEndpoint {
                read_queue: initiator_to_responder,
                write_queue: responder_to_initiator,
                peer_conn_id: responder_peer_conn_id_slot.clone(),
                ..Default::default()
            };

            // Try to add to listener's pending queue (includes initiator's slot for accept to fill)
            let queued = inner.connect(target_addr, responder_endpoint, initiator_peer_conn_id_slot, cx.waker());
            if !queued {
                // No listener yet, the waker is registered. Return Pending
                return Poll::Pending;
            }

            // Register initiator's connection only after successfully queuing
            let conn_id = inner.register_endpoint(initiator_endpoint);

            // Set responder's peer_conn_id to the initiator's conn_id
            // (The initiator's peer_conn_id will be set by accept() when it registers the responder)
            *responder_peer_conn_id_slot.lock() = Some(conn_id);

            // Wake accept wakers since we queued a connection
            provider.wake_accept_wakers();

            tracing::debug!("connected to {target_addr} with id {conn_id}");
            Poll::Ready(Ok(conn_id))
        }))
    }

    /// Connect to a given peer, at a list of SocketAddr.
    fn connect_addrs(
        &self,
        addr: ToSocketAddrs,
        timeout: Duration,
    ) -> BoxFuture<'static, std::io::Result<ConnectionId>> {
        let inner = self.inner.clone();
        Box::pin(async move {
            let addrs = addr.to_socket_addrs().map_err(std::io::Error::other)?;
            let provider = InMemoryConnectionProvider { inner };
            provider.connect(addrs, timeout).await
        })
    }

    /// Send a message to the connection peer:
    ///
    /// - Add bytes to the connection write_queue (this is the read_queue of the peer on its connection)
    /// - Wake the peer so that it can read the data.
    ///
    fn send(&self, conn: ConnectionId, data: NonEmptyBytes) -> BoxFuture<'static, std::io::Result<()>> {
        let inner = self.inner.clone();
        let provider = self.clone();
        Box::pin(async move {
            let (write_queue, peer_conn_id) = {
                let connections = inner.connection_endpoints.lock();
                let endpoint = connections
                    .get(&conn)
                    .ok_or_else(|| std::io::Error::other(format!("connection {conn} not found for send")))?;
                (endpoint.write_queue.clone(), *endpoint.peer_conn_id.lock())
            };

            write_queue.lock().push_back(Bytes::copy_from_slice(&data));

            // Wake the peer's recv waker if we know the peer connection ID
            if let Some(peer_id) = peer_conn_id {
                provider.wake_recv_wakers(peer_id);
            } else {
                // We don't know the peer's connection ID yet, so we need to wake
                // all recv wakers. This is less efficient but correct.
                // In practice, the peer will set their ID when they start receiving.
                let all_conn_ids: Vec<ConnectionId> = inner.connection_endpoints.lock().keys().copied().collect();
                for id in all_conn_ids {
                    if id != conn {
                        provider.wake_recv_wakers(id);
                    }
                }
            }

            Ok(())
        })
    }

    /// Receive a new message by reading from the message queue when a new message has arrived
    fn recv(&self, conn: ConnectionId, bytes: NonZeroUsize) -> BoxFuture<'static, std::io::Result<NonEmptyBytes>> {
        let inner = self.inner.clone();
        Box::pin(std::future::poll_fn(move |cx| {
            // Snapshot only the Arc handles we need, then release the outer lock immediately
            let (read_buffer, read_queue, recv_waker) = {
                let connections = inner.connection_endpoints.lock();
                match connections.get(&conn) {
                    Some(e) => (e.read_buffer.clone(), e.read_queue.clone(), e.recv_waker.clone()),
                    None => {
                        return Poll::Ready(Err(std::io::Error::other(format!(
                            "connection {conn} not found for recv"
                        ))));
                    }
                }
            }; // outer lock released here

            let mut buffer = read_buffer.lock();
            {
                let mut rq = read_queue.lock();
                while let Some(data) = rq.pop_front() {
                    buffer.extend_from_slice(&data);
                }
            }

            if buffer.remaining() >= bytes.get() {
                #[expect(clippy::expect_used)]
                let result = buffer.copy_to_bytes(bytes.get()).try_into().expect("guaranteed by NonZeroUsize");
                return Poll::Ready(Ok(result));
            }

            *recv_waker.lock() = Some(cx.waker().clone());
            Poll::Pending
        }))
    }

    /// Close a connection, given its connection id.
    /// This just removes the corresponding endpoint from the endpoints map.
    fn close(&self, conn: ConnectionId) -> BoxFuture<'static, std::io::Result<()>> {
        let inner = self.inner.clone();
        Box::pin(async move {
            let removed = {
                let mut connections = inner.connection_endpoints.lock();
                connections
                    .remove(&conn)
                    .ok_or_else(|| std::io::Error::other(format!("connection {conn} not found for close")))?
            };
            // Wake any recv task blocked on this connection so it gets an error, not a hang
            if let Some(waker) = removed.recv_waker.lock().take() {
                waker.wake();
            }

            tracing::debug!("closed in-memory connection {conn}");
            Ok(())
        })
    }
}

impl InMemoryConnectionProvider {
    /// Wake all accept wakers (called when a new connection is queued)
    fn wake_accept_wakers(&self) {
        let wakers: Vec<Waker> = self.inner.accept_wakers.lock().drain(..).collect();
        for waker in wakers {
            waker.wake();
        }
    }

    /// Wake connect wakers for a specific address (called when a listener is added)
    fn wake_connect_wakers(&self, addr: &SocketAddr) {
        let wakers: Vec<Waker> = self.inner.connect_wakers.lock().remove(addr).unwrap_or_default();
        for waker in wakers {
            waker.wake();
        }
    }

    /// Wake recv wakers for a specific connection (called when data is sent)
    fn wake_recv_wakers(&self, conn: ConnectionId) {
        let waker = {
            let connections = self.inner.connection_endpoints.lock();
            connections.get(&conn).and_then(|e| e.recv_waker.lock().take())
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

/// An endpoint for one side of an in-memory connection:
///
///  - The read_queue stores bytes read from the other side of the connection.
///  - The read_buffer aggregates bytes received on the read_queue so that a message can be read when enough bytes have been received.
///  - The write_queue collects data to be read by the other side of the connection.
///  - A recv waker can be set to be notified when a message has been received.
///  - The peer connection id is set when the peer has accepted this connection.
///
struct ConnectionEndpoint {
    /// Buffer for partial reads - data received but not yet consumed
    read_buffer: Arc<Mutex<BytesMut>>,
    /// Queue for data sent by the peer (shared with peer's write_queue)
    read_queue: Arc<Mutex<VecDeque<Bytes>>>,
    /// Queue to write data to peer's read_queue
    write_queue: Arc<Mutex<VecDeque<Bytes>>>,
    /// Waker for recv operations waiting for data
    recv_waker: Arc<Mutex<Option<Waker>>>,
    /// The peer's connection ID (for waking their recv waker on send)
    /// This is shared between endpoints to allow linking during connection setup.
    peer_conn_id: Arc<Mutex<Option<ConnectionId>>>,
}

impl Default for ConnectionEndpoint {
    fn default() -> Self {
        Self {
            read_buffer: Arc::new(Mutex::new(BytesMut::with_capacity(65536))),
            read_queue: Arc::new(Mutex::new(VecDeque::new())),
            write_queue: Arc::new(Mutex::new(VecDeque::new())),
            recv_waker: Arc::new(Mutex::new(None)),
            peer_conn_id: Arc::new(Mutex::new(None)),
        }
    }
}

// Manual Debug implementation since Waker doesn't implement Debug
impl std::fmt::Debug for ConnectionEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InMemoryEndpoint")
            .field("read_buffer", &self.read_buffer)
            .field("read_queue", &self.read_queue)
            .field("write_queue", &self.write_queue)
            .field("recv_waker", &"...")
            .field("peer_conn_id", &self.peer_conn_id)
            .finish()
    }
}

/// A listener waiting for incoming connections.
struct Listener {
    /// Queue for pending connections
    pending_connects: VecDeque<PendingConnect>,
}

/// A pending connection waiting to be accepted.
struct PendingConnect {
    /// The endpoint for the accepting (responder) side.
    responder_endpoint: ConnectionEndpoint,
    /// The address of the connecting (initiator) side.
    /// This identifies the downstream peer.
    initiator_addr: SocketAddr,
    /// Shared slot to set the initiator's peer_conn_id when the responder is registered.
    initiator_peer_conn_id_slot: Arc<Mutex<Option<ConnectionId>>>,
}

/// Shared state for the in-memory connection provider.
///
/// This tracks:
///
///  - A list of listeners.
///  - A map of connection endpoints, keyed by ConnectionId.
///  - A counter for ports created by the listener when accepting a connection.
///  - Wakers for waiting on accepted connections and waiting on upstream connections.
///
struct Inner {
    /// Active listeners by address
    listeners: Mutex<BTreeMap<SocketAddr, Listener>>,
    /// All active connection endpoints by ConnectionId
    connection_endpoints: Mutex<BTreeMap<ConnectionId, ConnectionEndpoint>>,
    /// Counter for generating ports for initiators
    last_connection_port: AtomicU16,
    /// Wakers for accept operations waiting for connections
    accept_wakers: Mutex<Vec<Waker>>,
    /// Wakers for connect operations waiting for listeners, keyed by target address
    connect_wakers: Mutex<BTreeMap<SocketAddr, Vec<Waker>>>,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            listeners: Mutex::new(BTreeMap::new()),
            connection_endpoints: Mutex::new(BTreeMap::new()),
            last_connection_port: AtomicU16::new(5000),
            accept_wakers: Mutex::new(Vec::new()),
            connect_wakers: Mutex::new(BTreeMap::new()),
        }
    }
}

impl Inner {
    fn register_endpoint(&self, endpoint: ConnectionEndpoint) -> ConnectionId {
        let mut connections = self.connection_endpoints.lock();
        let connection_id = if let Some((&last_id, _)) = connections.iter().next_back() {
            last_id.next()
        } else {
            ConnectionId::initial()
        };
        connections.insert(connection_id, endpoint);
        connection_id
    }

    fn new_port(&self) -> u16 {
        self.last_connection_port.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }

    /// Try to connect to a listener at the target address. If a listener exists, queue the connection and return true.
    /// If no listener exists, register the waker and return false.
    fn connect(
        &self,
        target_addr: &SocketAddr,
        responder_endpoint: ConnectionEndpoint,
        initiator_peer_conn_id_slot: Arc<Mutex<Option<ConnectionId>>>,
        waker: &Waker,
    ) -> bool {
        let pending = PendingConnect {
            responder_endpoint,
            initiator_addr: SocketAddr::from(([127, 0, 0, 1], self.new_port())),
            initiator_peer_conn_id_slot,
        };

        // Try to add to listener's pending queue
        let queued = {
            let mut listeners = self.listeners.lock();
            if let Some(listener) = listeners.get_mut(target_addr) {
                listener.pending_connects.push_back(pending);
                true
            } else {
                false
            }
        };

        if !queued {
            // No listener yet, register waker
            self.connect_wakers.lock().entry(*target_addr).or_default().push(waker.clone());
        }
        queued
    }
}

#[cfg(test)]
mod tests {
    use amaru_ouroboros_traits::ConnectionProvider;

    use super::*;

    /// Test basic connection setup and data transfer.
    #[tokio::test]
    async fn test_connection_and_send_recv() -> std::io::Result<()> {
        let provider = InMemoryConnectionProvider::default();
        let listener_addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();

        // Set up listener
        provider.listen(listener_addr).await?;

        // Connect from initiator side
        let initiator_conn_id = provider.connect(vec![listener_addr], Duration::from_secs(1)).await?;

        // Accept on responder side
        let (_peer, responder_conn_id) = provider.accept(listener_addr).await?;

        // Send first, then recv (data is already buffered when recv is called)
        let msg1 = NonEmptyBytes::try_from(Bytes::from("hello from initiator")).unwrap();
        provider.send(initiator_conn_id, msg1.clone()).await?;

        let received1 = provider.recv(responder_conn_id, msg1.len()).await?;
        assert_eq!(received1.as_ref(), msg1.as_ref());

        // Test bidirectional: responder sends to initiator
        let msg2 = NonEmptyBytes::try_from(Bytes::from("hello from responder")).unwrap();
        provider.send(responder_conn_id, msg2.clone()).await?;

        let received2 = provider.recv(initiator_conn_id, msg2.len()).await?;
        assert_eq!(received2.as_ref(), msg2.as_ref());

        // Clean up
        provider.close(initiator_conn_id).await?;
        provider.close(responder_conn_id).await?;

        Ok(())
    }

    /// Test that multiple messages can be sent and received correctly.
    #[tokio::test]
    async fn test_multiple_messages() -> std::io::Result<()> {
        let provider = InMemoryConnectionProvider::default();
        let listener_addr: SocketAddr = "127.0.0.1:9001".parse().unwrap();

        provider.listen(listener_addr).await?;
        let initiator_conn_id = provider.connect(vec![listener_addr], Duration::from_secs(1)).await?;
        let (_peer, responder_conn_id) = provider.accept(listener_addr).await?;

        // Send multiple messages
        let messages = vec!["first", "second", "third"];
        for msg in &messages {
            let data = NonEmptyBytes::try_from(Bytes::from(*msg)).unwrap();
            provider.send(initiator_conn_id, data).await?;
        }

        // Receive all messages
        for msg in &messages {
            let received = provider.recv(responder_conn_id, NonZeroUsize::new(msg.len()).unwrap()).await?;
            assert_eq!(received.as_ref(), msg.as_bytes());
        }

        provider.close(initiator_conn_id).await?;
        provider.close(responder_conn_id).await?;

        Ok(())
    }

    /// Test that connect waits for listen when using concurrent tasks.
    #[tokio::test]
    async fn test_connect_waits_for_listen() -> std::io::Result<()> {
        let provider = InMemoryConnectionProvider::default();
        let listener_addr: SocketAddr = "127.0.0.1:9002".parse().unwrap();

        let provider_clone = provider.clone();

        // Start connect first (will wait for listener)
        let connect_handle =
            tokio::spawn(async move { provider_clone.connect(vec![listener_addr], Duration::from_secs(10)).await });

        // Give connect a moment to start
        tokio::task::yield_now().await;

        // Now start listening
        provider.listen(listener_addr).await?;

        // Connect should now complete
        let initiator_conn_id = connect_handle.await.unwrap()?;

        // Accept should work
        let (_peer, _responder_conn_id) = provider.accept(listener_addr).await?;

        provider.close(initiator_conn_id).await?;

        Ok(())
    }
}
