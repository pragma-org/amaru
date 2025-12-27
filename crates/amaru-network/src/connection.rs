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
use amaru_ouroboros::{ConnectionId, ConnectionProvider, ToSocketAddrs};
use bytes::{Buf, BytesMut};
use parking_lot::Mutex;
use pure_stage::BoxFuture;
use std::{collections::BTreeMap, net::SocketAddr, num::NonZeroUsize, sync::Arc};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{
        TcpStream,
        tcp::{OwnedReadHalf, OwnedWriteHalf},
    },
    sync::Mutex as AsyncMutex,
};
use tracing::Instrument;

pub struct Connection {
    reader: Arc<AsyncMutex<(OwnedReadHalf, BytesMut)>>,
    writer: Arc<AsyncMutex<OwnedWriteHalf>>,
}

type Connections = BTreeMap<ConnectionId, Connection>;

#[derive(Clone)]
pub struct TokioConnections {
    connections: Arc<Mutex<Connections>>,
    read_buf_size: usize,
}

impl TokioConnections {
    pub fn new(read_buf_size: usize) -> Self {
        Self {
            connections: Arc::new(Mutex::new(Connections::new())),
            read_buf_size,
        }
    }
}

impl ConnectionProvider for TokioConnections {
    fn connect(&self, addr: Vec<SocketAddr>) -> BoxFuture<'static, std::io::Result<ConnectionId>> {
        let resource = self.connections.clone();
        let read_buf_size = self.read_buf_size;
        let addr2 = addr.clone();
        Box::pin(
            async move {
                let (reader, writer) = TcpStream::connect(&*addr).await?.into_split();
                let id = ConnectionId::new();
                resource.lock().insert(
                    id,
                    Connection {
                        reader: Arc::new(AsyncMutex::new((
                            reader,
                            BytesMut::with_capacity(read_buf_size),
                        ))),
                        writer: Arc::new(AsyncMutex::new(writer)),
                    },
                );
                Ok(id)
            }
            .instrument(tracing::debug_span!("connect", ?addr2)),
        )
    }

    fn connect_addrs(
        &self,
        addr: ToSocketAddrs,
    ) -> BoxFuture<'static, std::io::Result<ConnectionId>> {
        let resource = self.connections.clone();
        let read_buf_size = self.read_buf_size;
        let addr2 = addr.clone();
        Box::pin(
            async move {
                let addr = resolve(addr).await?;
                let (reader, writer) = TcpStream::connect(&*addr).await?.into_split();
                let id = ConnectionId::new();
                resource.lock().insert(
                    id,
                    Connection {
                        reader: Arc::new(AsyncMutex::new((
                            reader,
                            BytesMut::with_capacity(read_buf_size),
                        ))),
                        writer: Arc::new(AsyncMutex::new(writer)),
                    },
                );
                Ok(id)
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
                connection.lock().await.write_all(&data).await?;
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
                    std::io::Error::other(format!("connection {conn} not found for close"))
                })?;
                connection.writer.lock().await.shutdown().await?;
                Ok(())
            }
            .instrument(tracing::trace_span!("close", %conn)),
        )
    }
}
