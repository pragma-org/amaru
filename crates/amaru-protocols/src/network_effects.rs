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

use amaru_kernel::{NonEmptyBytes, Peer};
use amaru_ouroboros::{ConnectionId, ConnectionsResource, ToSocketAddrs};
use pure_stage::{BoxFuture, Effects, ExternalEffect, ExternalEffectAPI, Resources, SendData};
use std::fmt::{Display, Formatter};
use std::io::ErrorKind;
use std::{net::SocketAddr, num::NonZeroUsize, time::Duration};

pub fn register_deserializers() -> pure_stage::DeserializerGuards {
    vec![
        pure_stage::register_data_deserializer::<ListenEffect>().boxed(),
        pure_stage::register_data_deserializer::<AcceptEffect>().boxed(),
        pure_stage::register_data_deserializer::<ConnectEffect>().boxed(),
        pure_stage::register_data_deserializer::<SendEffect>().boxed(),
        pure_stage::register_data_deserializer::<RecvEffect>().boxed(),
        pure_stage::register_data_deserializer::<CloseEffect>().boxed(),
    ]
}

pub trait NetworkOps {
    fn listen(&self, addr: SocketAddr) -> BoxFuture<'static, Result<SocketAddr, ListenError>>;

    fn accept(&self) -> BoxFuture<'static, Result<(Peer, ConnectionId), AcceptError>>;

    fn connect(
        &self,
        addr: ToSocketAddrs,
        timeout: Duration,
    ) -> BoxFuture<'static, Result<ConnectionId, ConnectError>>;

    fn send(
        &self,
        conn: ConnectionId,
        data: NonEmptyBytes,
    ) -> BoxFuture<'static, Result<(), SendError>>;

    fn recv(
        &self,
        conn: ConnectionId,
        bytes: NonZeroUsize,
    ) -> BoxFuture<'static, Result<NonEmptyBytes, ReceiveError>>;

    fn close(&self, conn: ConnectionId) -> BoxFuture<'static, Result<(), CloseError>>;
}

pub struct Network<'a, T>(&'a Effects<T>);

impl<'a, T> Network<'a, T> {
    pub fn new(eff: &'a Effects<T>) -> Self {
        Network(eff)
    }
}

impl<T> NetworkOps for Network<'_, T> {
    fn listen(&self, addr: SocketAddr) -> BoxFuture<'static, Result<SocketAddr, ListenError>> {
        self.0.external(ListenEffect { addr })
    }

    fn accept(&self) -> BoxFuture<'static, Result<(Peer, ConnectionId), AcceptError>> {
        self.0.external(AcceptEffect)
    }

    fn connect(
        &self,
        addr: ToSocketAddrs,
        timeout: Duration,
    ) -> BoxFuture<'static, Result<ConnectionId, ConnectError>> {
        self.0.external(ConnectEffect { addr, timeout })
    }

    fn send(
        &self,
        conn: ConnectionId,
        data: NonEmptyBytes,
    ) -> BoxFuture<'static, Result<(), SendError>> {
        self.0.external(SendEffect { conn, data })
    }

    fn recv(
        &self,
        conn: ConnectionId,
        bytes: NonZeroUsize,
    ) -> BoxFuture<'static, Result<NonEmptyBytes, ReceiveError>> {
        self.0.external(RecvEffect { conn, bytes })
    }

    fn close(&self, conn: ConnectionId) -> BoxFuture<'static, Result<(), CloseError>> {
        self.0.external(CloseEffect { conn })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ListenEffect {
    pub addr: SocketAddr,
}

impl ExternalEffect for ListenEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap(async move {
            #[expect(clippy::expect_used)]
            let resource = resources
                .get::<ConnectionsResource>()
                .expect("ListenEffect requires a ConnectionResource")
                .clone();
            resource
                .listen(self.addr)
                .await
                .map_err(|e| ListenError(format!("{e}")))
        })
    }
}

impl ExternalEffectAPI for ListenEffect {
    type Response = Result<SocketAddr, ListenError>;
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ListenError(String);

impl Display for ListenError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let ListenError(error) = self;
        write!(f, "ListenError: {error}")
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AcceptEffect;

impl ExternalEffect for AcceptEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap(async move {
            #[expect(clippy::expect_used)]
            let resource = resources
                .get::<ConnectionsResource>()
                .expect("AcceptEffect requires a ConnectionResource")
                .clone();
            #[expect(clippy::wildcard_enum_match_arm)]
            resource.accept().await.map_err(|e| match e.kind() {
                ErrorKind::ConnectionAborted => AcceptError::ConnectionAborted,
                other => AcceptError::Other(format!("{other}")),
            })
        })
    }
}

impl ExternalEffectAPI for AcceptEffect {
    type Response = Result<(Peer, ConnectionId), AcceptError>;
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AcceptError {
    ConnectionAborted,
    Other(String),
}

impl Display for AcceptError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            AcceptError::ConnectionAborted => {
                write!(f, "AcceptError: connection aborted")
            }
            AcceptError::Other(e) => write!(f, "AcceptError: {e}"),
        }
    }
}
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ConnectEffect {
    pub addr: ToSocketAddrs,
    pub timeout: Duration,
}

impl ExternalEffect for ConnectEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap(async move {
            #[expect(clippy::expect_used)]
            let resource = resources
                .get::<ConnectionsResource>()
                .expect("ConnectEffect requires a ConnectionResource")
                .clone();
            resource
                .connect_addrs(self.addr.clone(), self.timeout)
                .await
                .map_err(|e| ConnectError {
                    addr: self.addr,
                    error: format!("{e}"),
                })
        })
    }
}

impl ExternalEffectAPI for ConnectEffect {
    type Response = Result<ConnectionId, ConnectError>;
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ConnectError {
    addr: ToSocketAddrs,
    error: String,
}

impl Display for ConnectError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let ConnectError { addr, error } = self;
        write!(f, "ConnectError on {addr:?}: {error}")
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SendEffect {
    pub conn: ConnectionId,
    pub data: NonEmptyBytes,
}

impl ExternalEffect for SendEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap(async move {
            #[expect(clippy::expect_used)]
            let resource = resources
                .get::<ConnectionsResource>()
                .expect("SendEffect requires a ConnectionResource")
                .clone();
            resource
                .send(self.conn, self.data)
                .await
                .map_err(|e| SendError {
                    conn: self.conn,
                    error: format!("{e}"),
                })
        })
    }
}

impl ExternalEffectAPI for SendEffect {
    type Response = Result<(), SendError>;
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SendError {
    conn: ConnectionId,
    error: String,
}

impl Display for SendError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let SendError { conn, error } = self;
        write!(f, "SendError on {conn:?}: {error}")
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecvEffect {
    pub conn: ConnectionId,
    pub bytes: NonZeroUsize,
}

impl ExternalEffect for RecvEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap(async move {
            #[expect(clippy::expect_used)]
            let resource = resources
                .get::<ConnectionsResource>()
                .expect("RecvEffect requires a ConnectionResource")
                .clone();
            resource
                .recv(self.conn, self.bytes)
                .await
                .map_err(|e| ReceiveError {
                    conn: self.conn,
                    error: format!("{e}"),
                })
        })
    }
}

impl ExternalEffectAPI for RecvEffect {
    type Response = Result<NonEmptyBytes, ReceiveError>;
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReceiveError {
    conn: ConnectionId,
    error: String,
}

impl Display for ReceiveError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let ReceiveError { conn, error } = self;
        write!(f, "ReceiveError on {conn:?}: {error}")
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CloseEffect {
    pub conn: ConnectionId,
}

impl ExternalEffect for CloseEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap(async move {
            #[expect(clippy::expect_used)]
            let resource = resources
                .get::<ConnectionsResource>()
                .expect("CloseEffect requires a ConnectionResource")
                .clone();
            resource.close(self.conn).await.map_err(|e| CloseError {
                conn: self.conn,
                error: format!("{e}"),
            })
        })
    }
}

impl ExternalEffectAPI for CloseEffect {
    type Response = Result<(), CloseError>;
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CloseError {
    conn: ConnectionId,
    error: String,
}

impl Display for CloseError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let CloseError { conn, error } = self;
        write!(f, "CloseError on {conn:?}: {error}")
    }
}

/// Create a connection to an upstream node, either specified in the PEER environment variable,
/// or to 127.0.0.1:3000
#[cfg(test)]
pub async fn create_connection(
    conn: &dyn amaru_ouroboros::ConnectionProvider,
) -> anyhow::Result<ConnectionId> {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        use amaru_network::socket_addr::resolve;

        let addr = ToSocketAddrs::String(
            std::env::var("PEER").unwrap_or_else(|_| "127.0.0.1:3000".to_string()),
        );
        let addr = resolve(addr).await?;
        Ok(conn.connect(addr, Duration::from_secs(5)).await?)
    })
    .await?
}
