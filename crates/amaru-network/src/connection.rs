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

use std::{collections::BTreeMap, net::SocketAddr, num::NonZeroUsize, sync::Arc, time::Duration};

use amaru_kernel::{NonEmptyBytes, Peer};
use amaru_observability::{Instrument, debug, debug_span, info};
use amaru_ouroboros::{ConnectionId, ConnectionProvider};
use amaru_pure_stage::BoxFuture;
use bytes::{Buf, BytesMut};
use futures_util::{FutureExt, future::join_all};
use parking_lot::Mutex;
use socket2::{Domain, Socket, Type};
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{
        TcpListener, TcpStream,
        tcp::{OwnedReadHalf, OwnedWriteHalf},
    },
    sync::{Mutex as AsyncMutex, mpsc, watch},
    task::{JoinError, JoinHandle},
};
use tokio_util::sync::CancellationToken;

pub struct Connection {
    peer_addr: SocketAddr,
    reader: Arc<AsyncMutex<(OwnedReadHalf, BytesMut)>>,
    writer: Arc<AsyncMutex<OwnedWriteHalf>>,
}

impl Connection {
    pub fn new(tcp_stream: TcpStream, read_buf_size: usize) -> std::io::Result<Self> {
        tcp_stream.set_nodelay(true)?;
        let (reader, writer) = tcp_stream.into_split();
        let peer_addr = reader.peer_addr()?;
        Ok(Self {
            peer_addr,
            reader: Arc::new(AsyncMutex::new((reader, BytesMut::with_capacity(read_buf_size)))),
            writer: Arc::new(AsyncMutex::new(writer)),
        })
    }

    pub fn peer_addr(&self) -> SocketAddr {
        self.peer_addr
    }
}

struct Connections {
    connections: BTreeMap<ConnectionId, Connection>,
    next_id: ConnectionId,
}

impl Connections {
    fn new() -> Self {
        Self { connections: BTreeMap::new(), next_id: ConnectionId::initial() }
    }

    fn add_connection(&mut self, connection: Connection) -> ConnectionId {
        let id = self.next_id.get_and_increment();
        self.insert(id, connection);
        id
    }

    fn insert(&mut self, id: ConnectionId, connection: Connection) {
        self.connections.insert(id, connection);
    }

    fn get(&self, id: &ConnectionId) -> Option<&Connection> {
        self.connections.get(id)
    }

    fn remove(&mut self, id: &ConnectionId) -> Option<Connection> {
        self.connections.remove(id)
    }
}

#[derive(Clone)]
pub struct TokioConnections {
    inner: Arc<Inner>,
}

/// A listener failure reported during shutdown.
#[derive(Debug, Error)]
pub enum ListenerError {
    #[error("listener accept failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("listener task failed: {0}")]
    Join(#[from] JoinError),
}

struct Inner {
    connections: Mutex<Connections>,
    read_buf_size: usize,
    incoming_tx: mpsc::Sender<PendingAccept>,
    incoming_rx: AsyncMutex<mpsc::Receiver<PendingAccept>>,
    shutdown: CancellationToken,
    tasks: AsyncMutex<BTreeMap<SocketAddr, JoinHandle<std::io::Result<()>>>>,
    first_listener: watch::Sender<Option<Result<SocketAddr, Arc<std::io::Error>>>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        for task in self.tasks.get_mut().values() {
            task.abort();
        }
    }
}

impl TokioConnections {
    pub fn new(read_buf_size: usize) -> Self {
        let (incoming_tx, incoming_rx) = mpsc::channel(128);
        let inner = Arc::new(Inner {
            connections: Mutex::new(Connections::new()),
            read_buf_size,
            incoming_tx,
            incoming_rx: AsyncMutex::new(incoming_rx),
            shutdown: CancellationToken::new(),
            tasks: AsyncMutex::new(BTreeMap::new()),
            first_listener: watch::channel(None).0,
        });
        Self { inner }
    }

    /// Wait for the first listen attempt to finish, retaining its address or original I/O error.
    ///
    /// Later listener restarts do not change this result. If no listen attempt completes,
    /// the caller must cancel this wait when its network manager terminates.
    pub async fn wait_for_listener(&self) -> Result<SocketAddr, Arc<std::io::Error>> {
        let mut receiver = self.inner.first_listener.subscribe();
        loop {
            if let Some(result) = receiver.borrow_and_update().clone() {
                return result;
            }
            receiver.changed().await.map_err(|error| Arc::new(std::io::Error::other(error)))?;
        }
    }

    /// Cancel pending accepts, stop and join all listener tasks, then close every active connection.
    ///
    /// New listeners are rejected once shutdown begins.
    ///
    /// Return every listener I/O and task failure, excluding expected task cancellations.
    pub async fn shutdown(&self) -> Vec<ListenerError> {
        self.inner.shutdown.cancel();
        let tasks = std::mem::take(&mut *self.inner.tasks.lock().await);
        tasks.values().for_each(JoinHandle::abort);
        let failures = join_all(tasks.into_values())
            .await
            .into_iter()
            .filter_map(|result| match result {
                Ok(Err(error)) => Some(ListenerError::Io(error)),
                Err(error) if !error.is_cancelled() => Some(ListenerError::Join(error)),
                Ok(Ok(())) | Err(_) => None,
            })
            .collect();

        self.inner.connections.lock().connections.clear();
        let mut incoming = self.inner.incoming_rx.lock().await;
        incoming.close();
        while let Some(pending) = incoming.recv().await {
            drop(pending);
        }

        failures
    }
}

async fn connect(peer: Peer, resource: Arc<Inner>, timeout: Duration) -> std::io::Result<ConnectionId> {
    let addr = SocketAddr::from(peer);
    let stream = tokio::time::timeout(timeout, TcpStream::connect(addr)).await??;
    debug!(network::connection::CONNECTED, peer);
    let mut connections = resource.connections.lock();
    let id = connections.add_connection(Connection::new(stream, resource.read_buf_size)?);
    Ok(id)
}

impl ConnectionProvider for TokioConnections {
    fn listen(&self, address: SocketAddr) -> BoxFuture<'static, std::io::Result<SocketAddr>> {
        let inner = self.inner.clone();
        let first_listener = inner.first_listener.clone();

        Box::pin(
            async move {
                let mut tasks = inner.tasks.lock().await;
                if inner.shutdown.is_cancelled() {
                    return Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "connection listener shut down"));
                }

                // If a listener already exists for this address, abort and join it.
                // This allows supervised restarts to work correctly.
                if let Some(task) = tasks.get_mut(&address) {
                    info!(network::connection::LISTENER_RESTART, address = address.to_string());
                    task.abort();
                    // Wait for the task to complete so the TcpListener is dropped and the port is released.
                    let _ = task.await;
                    tasks.remove(&address);
                }

                if inner.shutdown.is_cancelled() {
                    return Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "connection listener shut down"));
                }

                // Bind the listener with SO_REUSEADDR
                let listener = bind_address(address)?;
                let local = listener.local_addr()?;
                debug!(network::connection::LISTENING, local = local.to_string());

                // Accept incoming connections and send them into the channel.
                let incoming_tx = inner.incoming_tx.clone();
                let task = tokio::spawn(
                    // this task contains the listener and the sender, dropping them upon abort()
                    async move {
                        let result = loop {
                            let (stream, peer_addr) = match listener.accept().await {
                                Ok(connection) => connection,
                                Err(error) => break Err(error),
                            };
                            let Ok(_) = incoming_tx.send(PendingAccept { stream, peer_addr }).await else {
                                break Ok(());
                            };
                        };
                        info!(network::connection::ACCEPT_LOOP_STOPPED, local = local.to_string());
                        result
                    }
                    .instrument(debug_span!(network::connection::ACCEPT_LOOP,)),
                );

                tasks.insert(local, task);

                Ok(local)
            }
            .map(move |result| {
                let result = result.map_err(Arc::new);
                first_listener.send_if_modified(|first| {
                    if first.is_some() {
                        return false;
                    }
                    *first = Some(result.clone());
                    true
                });
                result.map_err(|error| std::io::Error::new(error.kind(), error))
            })
            .instrument(debug_span!(network::connection::LISTEN,)),
        )
    }

    /// NOTE: For now there is only one listener used in the tokio implementation so we don't need
    /// to use the _listener_addr.
    fn accept(&self, _listener_addr: SocketAddr) -> BoxFuture<'static, std::io::Result<(Peer, ConnectionId)>> {
        let inner = self.inner.clone();

        Box::pin(
            async move {
                let pending = tokio::select! {
                    biased;
                    _ = inner.shutdown.cancelled() => None,
                    pending = async {
                        let mut rx = inner.incoming_rx.lock().await;
                        rx.recv().await
                    } => pending,
                };
                let PendingAccept { stream, peer_addr } = pending.ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::BrokenPipe, "connection listener shut down")
                })?;

                debug!(network::connection::ACCEPTED, peer_addr = peer_addr.to_string());
                let peer = Peer::try_from(peer_addr)
                    .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
                let id = inner.connections.lock().add_connection(Connection::new(stream, inner.read_buf_size)?);
                Ok((peer, id))
            }
            .instrument(debug_span!(network::connection::ACCEPT,)),
        )
    }

    fn connect(&self, peer: Peer, timeout: Duration) -> BoxFuture<'static, std::io::Result<ConnectionId>> {
        Box::pin(connect(peer, self.inner.clone(), timeout).instrument(debug_span!(network::connection::CONNECT,)))
    }

    fn send(&self, conn: ConnectionId, data: NonEmptyBytes) -> BoxFuture<'static, std::io::Result<()>> {
        let resource = self.inner.clone();
        Box::pin(
            async move {
                let connection = resource
                    .connections
                    .lock()
                    .get(&conn)
                    .ok_or_else(|| std::io::Error::other(format!("connection {conn} not found for send")))?
                    .writer
                    .clone();
                tokio::time::timeout(Duration::from_secs(100), connection.lock().await.write_all(&data)).await??;
                Ok(())
            }
            .instrument(debug_span!(network::connection::SEND,)),
        )
    }

    fn recv(&self, conn: ConnectionId, bytes: NonZeroUsize) -> BoxFuture<'static, std::io::Result<NonEmptyBytes>> {
        let resource = self.inner.clone();
        Box::pin(
            async move {
                let connection = resource
                    .connections
                    .lock()
                    .get(&conn)
                    .ok_or_else(|| std::io::Error::other(format!("connection {conn} not found for recv")))?
                    .reader
                    .clone();
                let mut guard = connection.lock().await;
                let (reader, buf) = &mut *guard;
                buf.reserve(bytes.get() - buf.remaining().min(bytes.get()));
                while buf.remaining() < bytes.get() {
                    if reader.read_buf(buf).await? == 0 {
                        return Err(std::io::ErrorKind::UnexpectedEof.into());
                    };
                }
                #[expect(clippy::expect_used)]
                Ok(buf.copy_to_bytes(bytes.get()).try_into().expect("guaranteed by NonZeroUsize"))
            }
            .instrument(debug_span!(network::connection::RECV,)),
        )
    }

    fn close(&self, conn: ConnectionId) -> BoxFuture<'static, std::io::Result<()>> {
        let resource = self.inner.clone();
        Box::pin(
            async move {
                let connection = resource.connections.lock().remove(&conn).ok_or_else(|| {
                    // TODO: figure out how to not raise an error for a connection that has simply been closed already
                    std::io::Error::other(format!("connection {conn} not found for close"))
                })?;
                connection.writer.lock().await.shutdown().await?;
                Ok(())
            }
            .instrument(debug_span!(network::connection::CLOSE,)),
        )
    }
}

/// Local sruct holding a pending accepted connection
/// until it is picked up by and accept call and added to the list of connections.
struct PendingAccept {
    stream: TcpStream,
    peer_addr: SocketAddr,
}

/// Binds a TCP listener to the specified address with
/// `SO_REUSEADDR` enabled.
fn bind_address(addr: SocketAddr) -> std::io::Result<TcpListener> {
    let domain = match addr {
        SocketAddr::V4(_) => Domain::IPV4,
        SocketAddr::V6(_) => Domain::IPV6,
    };

    let socket = Socket::new(domain, Type::STREAM, None)?;

    // Allow rebinding to a port that was recently used (e.g., still in TIME_WAIT).
    socket.set_reuse_address(true)?;

    socket.bind(&addr.into())?;
    socket.listen(1024)?;

    socket.set_nonblocking(true)?;
    TcpListener::from_std(socket.into())
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use tokio::{task::JoinHandle, time::timeout};

    use super::*;

    #[tokio::test]
    async fn connect_to_a_server() -> anyhow::Result<()> {
        // Start a TCP listener that echoes "pong" when it receives "ping".
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let addr = listener.local_addr()?;
        let server: JoinHandle<std::io::Result<()>> = tokio::spawn(async move {
            let (mut stream, _peer) = listener.accept().await?;

            let mut buf = [0u8; 4];
            stream.read_exact(&mut buf).await?;
            assert_eq!(&buf, b"ping");

            stream.write_all(b"pong").await?;
            Ok(())
        });

        // Use TokioConnections to connect to the listener.
        let connections = TokioConnections::new(1024);
        let connection_id = connections.connect(Peer::try_from(addr)?, Duration::from_secs(1)).await?;
        connections.send(connection_id, non_empty(b"ping")).await?;
        let reply = connections.recv(connection_id, const { NonZeroUsize::new(4).unwrap() }).await?;
        assert_eq!(reply.as_ref(), b"pong");

        connections.close(connection_id).await?;
        server.await.expect("server task panicked")?;

        Ok(())
    }

    #[tokio::test]
    async fn bind_and_accept_a_client_connection() -> anyhow::Result<()> {
        // Create a TokioConnections instance and bind a TCP listener
        // to an ephemeral port.
        let connections = TokioConnections::new(1024);

        let listen_addr = SocketAddr::from(([127, 0, 0, 1], 0));
        let addr = connections.listen(listen_addr).await?;

        // Start a client that connects to the listener and
        // sends "hello", expecting "world" in response.
        let client: JoinHandle<std::io::Result<()>> = tokio::spawn(async move {
            let mut stream = TcpStream::connect(addr).await?;
            stream.write_all(b"hello").await?;

            let mut buf = String::new();
            stream.read_to_string(&mut buf).await?;
            assert_eq!(&buf, "world");

            Ok(())
        });

        // Receive "hello" from the client and respond with "world".
        let connection_id = timeout(Duration::from_secs(1), connections.accept(listen_addr)).await??.1;
        let result = connections.recv(connection_id, const { NonZeroUsize::new(5).unwrap() }).await?;
        assert_eq!(result.as_ref(), b"hello");

        connections.send(connection_id, non_empty(b"world")).await?;
        connections.close(connection_id).await?;

        client.await.expect("client task panicked")?;
        Ok(())
    }

    #[tokio::test]
    async fn shutdown_releases_listener() -> anyhow::Result<()> {
        let connections = TokioConnections::new(1024);
        let addr = connections.listen(SocketAddr::from(([127, 0, 0, 1], 0))).await?;

        assert!(connections.shutdown().await.is_empty());

        let listener = TcpListener::bind(addr).await?;
        assert_eq!(listener.local_addr()?, addr);
        Ok(())
    }

    #[tokio::test]
    async fn shutdown_rejects_new_listeners() -> anyhow::Result<()> {
        let connections = TokioConnections::new(1024);
        let address = SocketAddr::from(([127, 0, 0, 1], 0));
        let listening = connections.listen(address);

        assert!(connections.shutdown().await.is_empty());

        assert_eq!(listening.await.unwrap_err().kind(), std::io::ErrorKind::BrokenPipe);
        assert_eq!(connections.listen(address).await.unwrap_err().kind(), std::io::ErrorKind::BrokenPipe);
        assert!(connections.inner.tasks.lock().await.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn shutdown_unblocks_pending_accepts() -> anyhow::Result<()> {
        let connections = TokioConnections::new(1024);
        let addr = connections.listen(SocketAddr::from(([127, 0, 0, 1], 0))).await?;
        let mut accepting = connections.accept(addr);
        let mut waiting = connections.accept(addr);
        assert!(futures_util::poll!(&mut accepting).is_pending());
        assert!(futures_util::poll!(&mut waiting).is_pending());

        let (shutdown, accepted, waited) =
            timeout(Duration::from_secs(1), async { tokio::join!(connections.shutdown(), accepting, waiting) }).await?;
        assert!(shutdown.is_empty());
        assert_eq!(accepted.unwrap_err().kind(), std::io::ErrorKind::BrokenPipe);
        assert_eq!(waited.unwrap_err().kind(), std::io::ErrorKind::BrokenPipe);

        let error = timeout(Duration::from_secs(1), connections.accept(addr)).await?.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
        let listener = TcpListener::bind(addr).await?;
        assert_eq!(listener.local_addr()?, addr);
        Ok(())
    }

    #[tokio::test]
    async fn shutdown_reports_all_listener_failures() -> anyhow::Result<()> {
        let connections = TokioConnections::new(1024);
        let messages = ["first listener failed", "second listener failed"];
        let tasks = [
            tokio::spawn(async move { panic!("{}", messages[0]) }),
            tokio::spawn(async move { panic!("{}", messages[1]) }),
            tokio::spawn(async { Err(std::io::Error::new(std::io::ErrorKind::ConnectionAborted, "accept failed")) }),
            tokio::spawn(async { Ok(()) }),
        ];
        for (port, task) in (1..).zip(tasks) {
            timeout(Duration::from_secs(1), async {
                while !task.is_finished() {
                    tokio::task::yield_now().await;
                }
            })
            .await?;
            connections.inner.tasks.lock().await.insert(SocketAddr::from(([127, 0, 0, 1], port)), task);
        }
        let address = connections.listen(SocketAddr::from(([127, 0, 0, 1], 0))).await?;

        let failures = connections.shutdown().await;
        assert_eq!(failures.len(), messages.len() + 1);
        for (failure, message) in failures.iter().zip(messages) {
            assert!(matches!(failure, ListenerError::Join(error) if error.is_panic()));
            assert!(failure.to_string().contains(message), "{failure}");
        }
        let ListenerError::Io(error) = &failures[messages.len()] else {
            panic!("expected a listener I/O error");
        };
        assert_eq!(error.kind(), std::io::ErrorKind::ConnectionAborted);
        assert_eq!(error.to_string(), "accept failed");
        let listener = TcpListener::bind(address).await?;
        assert_eq!(listener.local_addr()?, address);
        Ok(())
    }

    // HELPERS

    fn non_empty(data: &'static [u8]) -> NonEmptyBytes {
        Bytes::from_static(data).try_into().expect("test data must be non-empty")
    }
}
