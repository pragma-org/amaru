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

use amaru_kernel::{NonEmptyBytes, Peer};
use amaru_ouroboros::{ConnectionId, ConnectionProvider, ToSocketAddrs};
use parking_lot::Mutex;
use pure_stage::BoxFuture;
use std::collections::{BTreeMap, VecDeque};
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::AtomicU16;
use std::task::{Poll, Waker};
use std::time::Duration;
use tokio_util::bytes::{Buf, Bytes, BytesMut};

/// A connection provider that uses in-memory channels instead of TCP sockets.
/// This implementation uses synchronous data structures with wakers to avoid
/// deadlocks when used with the SimulationBuilder.
#[derive(Clone, Default)]
pub struct InMemoryConnectionProvider {
    inner: Arc<Inner>,
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
        let wakers: Vec<Waker> = self
            .inner
            .connect_wakers
            .lock()
            .remove(addr)
            .unwrap_or_default();
        for waker in wakers {
            waker.wake();
        }
    }

    /// Wake recv wakers for a specific connection (called when data is sent)
    fn wake_recv_wakers(&self, conn: ConnectionId) {
        let waker = {
            let connections = self.inner.connections.lock();
            connections
                .get(&conn)
                .and_then(|e| e.recv_waker.lock().take())
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
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
                    return Err(std::io::Error::other(format!(
                        "listener already bound to {addr}"
                    )));
                }

                listeners.insert(
                    addr,
                    Listener {
                        pending_connects: VecDeque::new(),
                    },
                );
            }

            // Wake any connect wakers waiting for this address
            provider.wake_connect_wakers(&addr);

            tracing::debug!("listener bound to {addr}");
            Ok(addr)
        })
    }

    /// Accept an incoming connection for any of the registered listeners. Returns the peer and connection ID.
    /// If no peer is currently trying to connect, this waits until a connection is available
    /// and wakes the caller.
    fn accept(&self) -> BoxFuture<'static, std::io::Result<(Peer, ConnectionId)>> {
        let inner = self.inner.clone();
        Box::pin(std::future::poll_fn(move |cx| {
            // Try to get a pending connection from any listener
            let pending = {
                let mut listeners = inner.listeners.lock();
                listeners
                    .values_mut()
                    .find_map(|l| l.pending_connects.pop_front())
            };

            let Some(pending) = pending else {
                // No pending connections, register waker and return Pending
                inner.accept_wakers.lock().push(cx.waker().clone());
                return Poll::Pending;
            };

            // Register the responder's endpoint, the initiator is already registered in connect().
            let conn_id = inner.register_endpoint(pending.responder_endpoint);

            tracing::debug!(
                "accepted in-memory connection from {} with id {conn_id}",
                pending.initiator_addr
            );
            Poll::Ready(Ok((Peer::from_addr(&pending.initiator_addr), conn_id)))
        }))
    }

    /// Connect to one of the given addresses. If a listener for the target address is already registered,
    /// the connection is established immediately.
    fn connect(
        &self,
        addr: Vec<SocketAddr>,
        _timeout: Duration,
    ) -> BoxFuture<'static, std::io::Result<ConnectionId>> {
        let inner = self.inner.clone();
        let provider = self.clone();
        tracing::debug!("InMemoryConnectionProvider::connect called for {addr:?}");

        Box::pin(std::future::poll_fn(move |cx| {
            let Some(target_addr) = addr.first() else {
                return Poll::Ready(Err(std::io::Error::other("no addresses provided")));
            };

            // Create bidirectional channel pair using VecDeque
            let initiator_to_responder = Arc::new(Mutex::new(VecDeque::new()));
            let responder_to_initiator = Arc::new(Mutex::new(VecDeque::new()));

            // Create initiator endpoint
            let initiator_endpoint = InMemoryEndpoint {
                read_queue: responder_to_initiator.clone(),
                write_queue: initiator_to_responder.clone(),
                ..Default::default()
            };

            // Create responder endpoint
            let responder_endpoint = InMemoryEndpoint {
                read_queue: initiator_to_responder,
                write_queue: responder_to_initiator,
                ..Default::default()
            };

            // Try to add to listener's pending queue
            let queued = inner.connect(target_addr, responder_endpoint, cx.waker());
            if !queued {
                // No listener yet, the waker is registered. Return Pending
                return Poll::Pending;
            }

            // Register initiator's connection only after successfully queuing
            let conn_id = inner.register_endpoint(initiator_endpoint);

            // Wake accept wakers since we queued a connection
            provider.wake_accept_wakers();

            tracing::debug!("connected to {target_addr} with id {conn_id}");
            Poll::Ready(Ok(conn_id))
        }))
    }

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

    fn send(
        &self,
        conn: ConnectionId,
        data: NonEmptyBytes,
    ) -> BoxFuture<'static, std::io::Result<()>> {
        let inner = self.inner.clone();
        let provider = self.clone();
        Box::pin(async move {
            let (write_queue, peer_conn_id) = {
                let connections = inner.connections.lock();
                let endpoint = connections.get(&conn).ok_or_else(|| {
                    std::io::Error::other(format!("connection {conn} not found for send"))
                })?;
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
                let all_conn_ids: Vec<ConnectionId> =
                    inner.connections.lock().keys().copied().collect();
                for id in all_conn_ids {
                    if id != conn {
                        provider.wake_recv_wakers(id);
                    }
                }
            }

            Ok(())
        })
    }

    fn recv(
        &self,
        conn: ConnectionId,
        bytes: NonZeroUsize,
    ) -> BoxFuture<'static, std::io::Result<NonEmptyBytes>> {
        let inner = self.inner.clone();
        Box::pin(std::future::poll_fn(move |cx| {
            let connections = inner.connections.lock();
            let endpoint = match connections.get(&conn) {
                Some(e) => e,
                None => {
                    return Poll::Ready(Err(std::io::Error::other(format!(
                        "connection {conn} not found for recv"
                    ))));
                }
            };

            // Drain any available data from the read queue into our buffer
            let mut buffer = endpoint.read_buffer.lock();
            {
                let mut read_queue = endpoint.read_queue.lock();
                while let Some(data) = read_queue.pop_front() {
                    buffer.extend_from_slice(&data);
                }
            }

            // Check if we have enough bytes in the buffer
            if buffer.remaining() >= bytes.get() {
                #[expect(clippy::expect_used)]
                let result = buffer
                    .copy_to_bytes(bytes.get())
                    .try_into()
                    .expect("guaranteed by NonZeroUsize");
                return Poll::Ready(Ok(result));
            }

            // Not enough data yet, register waker and return Pending
            *endpoint.recv_waker.lock() = Some(cx.waker().clone());
            Poll::Pending
        }))
    }

    fn close(&self, conn: ConnectionId) -> BoxFuture<'static, std::io::Result<()>> {
        let inner = self.inner.clone();
        Box::pin(async move {
            let mut connections = inner.connections.lock();
            connections.remove(&conn).ok_or_else(|| {
                std::io::Error::other(format!("connection {conn} not found for close"))
            })?;
            // Dropping the endpoint closes its channels
            tracing::debug!("closed in-memory connection {conn}");
            Ok(())
        })
    }
}

/// An endpoint for one side of an in-memory connection.
/// Uses synchronous data structures with wakers to support simulation.
struct InMemoryEndpoint {
    /// Buffer for partial reads - data received but not yet consumed
    read_buffer: Mutex<BytesMut>,
    /// Queue for data sent by the peer (shared with peer's write_queue)
    read_queue: Arc<Mutex<VecDeque<Bytes>>>,
    /// Queue to write data to peer's read_queue
    write_queue: Arc<Mutex<VecDeque<Bytes>>>,
    /// Waker for recv operations waiting for data
    recv_waker: Mutex<Option<Waker>>,
    /// The peer's connection ID (for waking their recv waker on send)
    peer_conn_id: Mutex<Option<ConnectionId>>,
}

impl Default for InMemoryEndpoint {
    fn default() -> Self {
        Self {
            read_buffer: Mutex::new(BytesMut::with_capacity(65536)),
            read_queue: Arc::new(Mutex::new(VecDeque::new())),
            write_queue: Arc::new(Mutex::new(VecDeque::new())),
            recv_waker: Mutex::new(None),
            peer_conn_id: Mutex::new(None),
        }
    }
}

// Manual Debug implementation since Waker doesn't implement Debug
impl std::fmt::Debug for InMemoryEndpoint {
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
    responder_endpoint: InMemoryEndpoint,
    /// The address of the connecting (initiator) side.
    /// This identifies the downstream peer.
    initiator_addr: SocketAddr,
}

/// Shared state for the in-memory connection provider.
struct Inner {
    /// Active listeners by address
    listeners: Mutex<BTreeMap<SocketAddr, Listener>>,
    /// All active connection endpoints by ConnectionId
    connections: Mutex<BTreeMap<ConnectionId, InMemoryEndpoint>>,
    /// Counter for generating ephemeral ports for initiators
    ephemeral_port: AtomicU16,
    /// Wakers for accept operations waiting for connections
    accept_wakers: Mutex<Vec<Waker>>,
    /// Wakers for connect operations waiting for listeners, keyed by target address
    connect_wakers: Mutex<BTreeMap<SocketAddr, Vec<Waker>>>,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            listeners: Mutex::new(BTreeMap::new()),
            connections: Mutex::new(BTreeMap::new()),
            ephemeral_port: AtomicU16::new(5000),
            accept_wakers: Mutex::new(Vec::new()),
            connect_wakers: Mutex::new(BTreeMap::new()),
        }
    }
}

impl Inner {
    fn register_endpoint(&self, endpoint: InMemoryEndpoint) -> ConnectionId {
        let mut connections = self.connections.lock();
        let connection_id = if let Some((&last_id, _)) = connections.iter().next_back() {
            last_id.next()
        } else {
            ConnectionId::initial()
        };
        connections.insert(connection_id, endpoint);
        connection_id
    }

    fn new_port(&self) -> u16 {
        self.ephemeral_port
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }

    /// Try to connect to a listener at the target address. If a listener exists, queue the connection and return true.
    /// If no listener exists, register the waker and return false.
    fn connect(
        &self,
        target_addr: &SocketAddr,
        responder_endpoint: InMemoryEndpoint,
        waker: &Waker,
    ) -> bool {
        let pending = PendingConnect {
            responder_endpoint,
            initiator_addr: SocketAddr::from(([127, 0, 0, 1], self.new_port())),
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
            self.connect_wakers
                .lock()
                .entry(*target_addr)
                .or_default()
                .push(waker.clone());
        }
        queued
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use amaru_ouroboros_traits::ConnectionProvider;

    /// Test basic connection setup and data transfer.
    #[tokio::test]
    async fn test_connection_and_send_recv() -> std::io::Result<()> {
        let provider = InMemoryConnectionProvider::default();
        let listener_addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();

        // Set up listener
        provider.listen(listener_addr).await?;

        // Connect from initiator side
        let initiator_conn_id = provider
            .connect(vec![listener_addr], Duration::from_secs(1))
            .await?;

        // Accept on responder side
        let (_peer, responder_conn_id) = provider.accept().await?;

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
        let initiator_conn_id = provider
            .connect(vec![listener_addr], Duration::from_secs(1))
            .await?;
        let (_peer, responder_conn_id) = provider.accept().await?;

        // Send multiple messages
        let messages = vec!["first", "second", "third"];
        for msg in &messages {
            let data = NonEmptyBytes::try_from(Bytes::from(*msg)).unwrap();
            provider.send(initiator_conn_id, data).await?;
        }

        // Receive all messages
        for msg in &messages {
            let received = provider
                .recv(responder_conn_id, NonZeroUsize::new(msg.len()).unwrap())
                .await?;
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
        let connect_handle = tokio::spawn(async move {
            provider_clone
                .connect(vec![listener_addr], Duration::from_secs(10))
                .await
        });

        // Give connect a moment to start
        tokio::task::yield_now().await;

        // Now start listening
        provider.listen(listener_addr).await?;

        // Connect should now complete
        let initiator_conn_id = connect_handle.await.unwrap()?;

        // Accept should work
        let (_peer, _responder_conn_id) = provider.accept().await?;

        provider.close(initiator_conn_id).await?;

        Ok(())
    }
}
