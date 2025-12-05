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

use bytes::{Buf, Bytes, BytesMut};
use parking_lot::Mutex;
use pure_stage::BoxFuture;
#[expect(clippy::disallowed_types)]
use std::collections::HashMap;
use std::{
    fmt,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{
        TcpStream,
        tcp::{OwnedReadHalf, OwnedWriteHalf},
    },
    sync::Mutex as AsyncMutex,
};

#[derive(
    Debug, PartialEq, Eq, Hash, Clone, Copy, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct ConnectionId(u64);

#[expect(clippy::new_without_default)]
impl ConnectionId {
    pub fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

impl fmt::Display for ConnectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub struct Connection {
    reader: Arc<AsyncMutex<(OwnedReadHalf, BytesMut)>>,
    writer: Arc<AsyncMutex<OwnedWriteHalf>>,
}

#[expect(clippy::disallowed_types)]
type Connections = HashMap<ConnectionId, Connection>;

#[derive(Clone)]
pub struct ConnectionResource {
    connections: Arc<Mutex<Connections>>,
    read_buf_size: usize,
}

impl ConnectionResource {
    pub fn new(read_buf_size: usize) -> Self {
        Self {
            connections: Arc::new(Mutex::new(Connections::new())),
            read_buf_size,
        }
    }

    pub fn connect(
        &self,
        addr: Vec<SocketAddr>,
    ) -> BoxFuture<'static, std::io::Result<ConnectionId>> {
        let resource = self.connections.clone();
        let read_buf_size = self.read_buf_size;
        Box::pin(async move {
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
        })
    }

    pub fn send(&self, conn: ConnectionId, data: Bytes) -> BoxFuture<'static, std::io::Result<()>> {
        let resource = self.connections.clone();
        Box::pin(async move {
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
        })
    }

    pub fn recv(
        &self,
        conn: ConnectionId,
        bytes: usize,
    ) -> BoxFuture<'static, std::io::Result<Bytes>> {
        let resource = self.connections.clone();
        Box::pin(async move {
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
            buf.reserve(bytes - buf.remaining().min(bytes));
            while buf.remaining() < bytes {
                if reader.read_buf(buf).await? == 0 {
                    return Err(std::io::ErrorKind::UnexpectedEof.into());
                };
            }
            Ok(buf.copy_to_bytes(bytes))
        })
    }

    pub fn close(&self, conn: ConnectionId) -> BoxFuture<'static, std::io::Result<()>> {
        let resource = self.connections.clone();
        Box::pin(async move {
            let connection = resource.lock().remove(&conn).ok_or_else(|| {
                std::io::Error::other(format!("connection {conn} not found for close"))
            })?;
            connection.writer.lock().await.shutdown().await?;
            Ok(())
        })
    }
}
