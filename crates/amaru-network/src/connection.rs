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

use crate::socket_addr::resolve;
use amaru_kernel::bytes::NonEmptyBytes;
use amaru_kernel::peer::Peer;
use amaru_ouroboros::{ConnectionId, ConnectionProvider, ToSocketAddrs};
use bytes::{Buf, BytesMut};
use parking_lot::Mutex;
use pure_stage::{BoxFuture, Sender};
use socket2::{Domain, Socket, Type};
use std::{collections::BTreeMap, net::SocketAddr, num::NonZeroUsize, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{
        TcpListener, TcpStream,
        tcp::{OwnedReadHalf, OwnedWriteHalf},
    },
    sync::Mutex as AsyncMutex,
};
use tracing::Instrument;

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
            reader: Arc::new(AsyncMutex::new((
                reader,
                BytesMut::with_capacity(read_buf_size),
            ))),
            writer: Arc::new(AsyncMutex::new(writer)),
        })
    }

    pub fn peer_addr(&self) -> SocketAddr {
        self.peer_addr
    }
}

struct Connections {
    connections: BTreeMap<ConnectionId, Connection>,
}

impl Connections {
    fn new() -> Self {
        Self {
            connections: BTreeMap::new(),
        }
    }

    fn add_connection(&mut self, connection: Connection) -> ConnectionId {
        let id = if let Some((&last_id, _)) = self.connections.iter().next_back() {
            last_id.next()
        } else {
            ConnectionId::initial()
        };
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
    connections: Arc<Mutex<Connections>>,
    read_buf_size: usize,
    listener: Arc<AsyncMutex<Option<TcpListener>>>,
    accept_sender: Option<Sender<(Peer, ConnectionId)>>,
}

impl TokioConnections {
    pub fn new(read_buf_size: usize) -> Self {
        Self {
            connections: Arc::new(Mutex::new(Connections::new())),
            read_buf_size,
            listener: Arc::new(AsyncMutex::new(None)),
            accept_sender: None,
        }
    }

    pub fn with_accept_sender(mut self, accept_sender: Sender<(Peer, ConnectionId)>) -> Self {
        self.accept_sender = Some(accept_sender);
        self
    }
}

async fn connect(
    addr: Vec<SocketAddr>,
    resource: Arc<Mutex<Connections>>,
    read_buf_size: usize,
    timeout: Duration,
) -> std::io::Result<ConnectionId> {
    let stream = tokio::time::timeout(timeout, TcpStream::connect(&*addr)).await??;
    tracing::debug!(?addr, "connected");
    let mut connections = resource.lock();
    let id = connections.add_connection(Connection::new(stream, read_buf_size)?);
    Ok(id)
}

impl ConnectionProvider for TokioConnections {
    fn bind(&self, addr: SocketAddr) -> BoxFuture<'static, std::io::Result<SocketAddr>> {
        let listener = self.listener.clone();
        Box::pin(
            async move {
                let mut guard = listener.lock().await;
                if guard.is_some() {
                    return Err(std::io::Error::other(
                        "listener already created; cannot bind multiple times",
                    ));
                }
                let bound = bind_address(addr)?;
                let addr = bound.local_addr()?;
                tracing::debug!(%addr, "listening");
                *guard = Some(bound);
                Ok(addr)
            }
            .instrument(tracing::debug_span!("bind_listener", %addr)),
        )
    }

    fn listening_port(&self) -> BoxFuture<'static, std::io::Result<u16>> {
        let listener = self.listener.clone();
        Box::pin(
            async move {
                let guard = listener.lock().await;
                let l = guard.as_ref().ok_or_else(|| {
                    std::io::Error::other("listener not bound; call bind() first")
                })?;
                Ok(l.local_addr()?.port())
            }
            .instrument(tracing::debug_span!("listening_port")),
        )
    }

    fn accept(&self, timeout: Duration) -> BoxFuture<'static, std::io::Result<ConnectionId>> {
        let listener = self.listener.clone();
        let resource = self.connections.clone();
        let read_buf_size = self.read_buf_size;
        let accept_sender = self.accept_sender.clone();
        Box::pin(
            async move {
                let guard = listener.lock().await;
                let l = guard.as_ref().ok_or_else(|| {
                    std::io::Error::other("listener not bound; call bind() first")
                })?;

                let (stream, peer_addr) = tokio::time::timeout(timeout, l.accept()).await??;
                drop(guard);

                tracing::debug!(%peer_addr, "accepted connection");
                let id = {
                    let mut connections = resource.lock();
                    connections.add_connection(Connection::new(stream, read_buf_size)?)
                };

                if let Some(sender) = accept_sender {
                    let peer = Peer::new(peer_addr.to_string().as_str());
                    sender.send((peer, id)).await.map_err(|_| {
                        std::io::Error::other("failed to send accepted connection to accept stage")
                    })?;
                }
                Ok(id)
            }
            .instrument(tracing::debug_span!("accept")),
        )
    }

    fn connect(
        &self,
        addr: Vec<SocketAddr>,
        timeout: Duration,
    ) -> BoxFuture<'static, std::io::Result<ConnectionId>> {
        let addr2 = addr.clone();
        Box::pin(
            connect(addr, self.connections.clone(), self.read_buf_size, timeout)
                .instrument(tracing::debug_span!("connect", ?addr2)),
        )
    }

    fn connect_addrs(
        &self,
        addr: ToSocketAddrs,
        timeout: Duration,
    ) -> BoxFuture<'static, std::io::Result<ConnectionId>> {
        let resource = self.connections.clone();
        let read_buf_size = self.read_buf_size;
        let addr2 = addr.clone();
        Box::pin(
            async move {
                let addr = resolve(addr).await?;
                tracing::debug!(?addr, "resolved addresses");
                connect(addr, resource, read_buf_size, timeout).await
            }
            .instrument(tracing::debug_span!("connect_addrs", ?addr2)),
        )
    }

    fn send(
        &self,
        conn: ConnectionId,
        data: NonEmptyBytes,
    ) -> BoxFuture<'static, std::io::Result<()>> {
        let resource = self.connections.clone();
        let len = data.len();
        Box::pin(
            async move {
                let connection = resource
                    .lock()
                    .get(&conn)
                    .ok_or_else(|| {
                        std::io::Error::other(format!("connection {conn} not found for send"))
                    })?
                    .writer
                    .clone();
                tokio::time::timeout(
                    Duration::from_secs(100),
                    connection.lock().await.write_all(&data),
                )
                .await??;
                Ok(())
            }
            .instrument(tracing::trace_span!("send", %conn, len)),
        )
    }

    fn recv(
        &self,
        conn: ConnectionId,
        bytes: NonZeroUsize,
    ) -> BoxFuture<'static, std::io::Result<NonEmptyBytes>> {
        let resource = self.connections.clone();
        Box::pin(
            async move {
                let connection = resource
                    .lock()
                    .get(&conn)
                    .ok_or_else(|| {
                        std::io::Error::other(format!("connection {conn} not found for recv"))
                    })?
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
                Ok(buf
                    .copy_to_bytes(bytes.get())
                    .try_into()
                    .expect("guaranteed by NonZeroUsize"))
            }
            .instrument(tracing::trace_span!("recv", %conn, bytes)),
        )
    }

    fn close(&self, conn: ConnectionId) -> BoxFuture<'static, std::io::Result<()>> {
        let resource = self.connections.clone();
        Box::pin(
            async move {
                let connection = resource.lock().remove(&conn).ok_or_else(|| {
                    // TODO: figure out how to not raise an error for a connection that has simply been closed already
                    std::io::Error::other(format!("connection {conn} not found for close"))
                })?;
                connection.writer.lock().await.shutdown().await?;
                Ok(())
            }
            .instrument(tracing::trace_span!("close", %conn)),
        )
    }
}

/// Binds a TCP listener to the specified address with
/// `SO_REUSEADDR` (and `SO_REUSEPORT` on Linux) enabled.
fn bind_address(addr: SocketAddr) -> std::io::Result<TcpListener> {
    let domain = match addr {
        SocketAddr::V4(_) => Domain::IPV4,
        SocketAddr::V6(_) => Domain::IPV6,
    };

    let socket = Socket::new(domain, Type::STREAM, None)?;

    // This makes sure that ports can be reused immediately after the program exits.
    socket.set_reuse_address(true)?;

    #[cfg(target_os = "linux")]
    socket.set_reuse_port(true)?;

    socket.bind(&addr.into())?;
    socket.listen(1024)?;

    socket.set_nonblocking(true)?;
    TcpListener::from_std(socket.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use tokio::task::JoinHandle;

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
        let connection_id = connections
            .connect(vec![addr], Duration::from_secs(1))
            .await?;

        connections.send(connection_id, non_empty(b"ping")).await?;
        let reply = connections
            .recv(connection_id, NonZeroUsize::new(4).expect("non-zero"))
            .await?;
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

        let addr = connections
            .bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await?;

        // Start a client that connects to the listener and
        // sends "hello", expecting "world" in response.
        let client: JoinHandle<std::io::Result<()>> = tokio::spawn(async move {
            let mut stream = TcpStream::connect(addr).await?;
            stream.write_all(b"hello").await?;

            let mut buf = [0u8; 5];
            stream.read_exact(&mut buf).await?;
            assert_eq!(&buf, b"world");

            Ok(())
        });

        // Receive "hello" from the client and respond with "world".
        let connection_id = connections.accept(Duration::from_secs(1)).await?;
        let result = connections
            .recv(connection_id, const { NonZeroUsize::new(5).unwrap() })
            .await?;
        assert_eq!(result.as_ref(), b"hello");

        connections.send(connection_id, non_empty(b"world")).await?;
        connections.close(connection_id).await?;

        client.await.expect("client task panicked")?;
        Ok(())
    }

    // HELPERS

    fn non_empty(data: &'static [u8]) -> NonEmptyBytes {
        Bytes::from_static(data)
            .try_into()
            .expect("test data must be non-empty")
    }
}
