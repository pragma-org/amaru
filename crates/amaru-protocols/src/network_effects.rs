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

use std::{
    fmt::{Display, Formatter},
    io::ErrorKind,
    net::SocketAddr,
    num::NonZeroUsize,
    time::Duration,
};

use amaru_kernel::{NonEmptyBytes, Peer};
use amaru_ouroboros::{ConnectionId, ConnectionsResource};
use amaru_pure_stage::{BoxFuture, DurationDist, Effects, ExternalEffectAPI, Resources, SendData};

pub fn register_deserializers() -> amaru_pure_stage::DeserializerGuards {
    vec![
        amaru_pure_stage::register_data_deserializer::<ListenEffect>().boxed(),
        amaru_pure_stage::register_data_deserializer::<AcceptEffect>().boxed(),
        amaru_pure_stage::register_data_deserializer::<ConnectEffect>().boxed(),
        amaru_pure_stage::register_data_deserializer::<SendEffect>().boxed(),
        amaru_pure_stage::register_data_deserializer::<RecvEffect>().boxed(),
        amaru_pure_stage::register_data_deserializer::<CloseEffect>().boxed(),
    ]
}

pub trait NetworkOps {
    fn listen(&self, addr: SocketAddr) -> BoxFuture<'static, Result<SocketAddr, ListenError>>;

    fn accept(&self, listener_addr: SocketAddr) -> BoxFuture<'static, Result<(Peer, ConnectionId), AcceptError>>;

    fn connect(&self, peer: Peer, timeout: Duration) -> BoxFuture<'static, Result<ConnectionId, ConnectError>>;

    fn send(&self, conn: ConnectionId, data: NonEmptyBytes) -> BoxFuture<'static, Result<(), SendError>>;

    fn recv(&self, conn: ConnectionId, bytes: NonZeroUsize) -> BoxFuture<'static, Result<NonEmptyBytes, ReceiveError>>;

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

    fn accept(&self, listener_addr: SocketAddr) -> BoxFuture<'static, Result<(Peer, ConnectionId), AcceptError>> {
        self.0.external(AcceptEffect { listener_addr })
    }

    fn connect(&self, peer: Peer, timeout: Duration) -> BoxFuture<'static, Result<ConnectionId, ConnectError>> {
        self.0.external(ConnectEffect { peer, timeout })
    }

    fn send(&self, conn: ConnectionId, data: NonEmptyBytes) -> BoxFuture<'static, Result<(), SendError>> {
        self.0.external(SendEffect { conn, data })
    }

    fn recv(&self, conn: ConnectionId, bytes: NonZeroUsize) -> BoxFuture<'static, Result<NonEmptyBytes, ReceiveError>> {
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

impl ExternalEffectAPI for ListenEffect {
    type Response = Result<SocketAddr, ListenError>;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap(|this| async move {
            #[expect(clippy::expect_used)]
            let resource =
                resources.get::<ConnectionsResource>().expect("ListenEffect requires a ConnectionsResource").clone();
            resource.listen(this.addr).await.map_err(|e| ListenError(format!("{e}")))
        })
    }
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
pub struct AcceptEffect {
    pub listener_addr: SocketAddr,
}

impl ExternalEffectAPI for AcceptEffect {
    type Response = Result<(Peer, ConnectionId), AcceptError>;
    const SIMULATED_DURATION: DurationDist = DurationDist::UntilResolved;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap(|this| async move {
            #[expect(clippy::expect_used)]
            let resource =
                resources.get::<ConnectionsResource>().expect("AcceptEffect requires a ConnectionsResource").clone();
            #[expect(clippy::wildcard_enum_match_arm)]
            resource.accept(this.listener_addr).await.map_err(|e| match e.kind() {
                ErrorKind::ConnectionAborted => AcceptError::ConnectionAborted,
                other => AcceptError::Other(format!("{other}")),
            })
        })
    }
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
    pub peer: Peer,
    pub timeout: Duration,
}

impl ExternalEffectAPI for ConnectEffect {
    type Response = Result<ConnectionId, ConnectError>;
    const SIMULATED_DURATION: DurationDist = DurationDist::UntilResolved;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap(|this| async move {
            #[expect(clippy::expect_used)]
            let resource =
                resources.get::<ConnectionsResource>().expect("ConnectEffect requires a ConnectionsResource").clone();
            resource
                .connect(this.peer, this.timeout)
                .await
                .map_err(|e| ConnectError { peer: this.peer, error: format!("{e}") })
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ConnectError {
    peer: Peer,
    error: String,
}

impl Display for ConnectError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let ConnectError { peer, error } = self;
        write!(f, "ConnectError on {peer}: {error}")
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SendEffect {
    pub conn: ConnectionId,
    pub data: NonEmptyBytes,
}

impl ExternalEffectAPI for SendEffect {
    type Response = Result<(), SendError>;
    const SIMULATED_DURATION: DurationDist = DurationDist::UntilResolved;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap(|this| async move {
            #[expect(clippy::expect_used)]
            let resource =
                resources.get::<ConnectionsResource>().expect("SendEffect requires a ConnectionsResource").clone();
            resource.send(this.conn, this.data).await.map_err(|e| SendError { conn: this.conn, error: format!("{e}") })
        })
    }
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

impl ExternalEffectAPI for RecvEffect {
    type Response = Result<NonEmptyBytes, ReceiveError>;
    const SIMULATED_DURATION: DurationDist = DurationDist::UntilResolved;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap(|this| async move {
            #[expect(clippy::expect_used)]
            let resource =
                resources.get::<ConnectionsResource>().expect("RecvEffect requires a ConnectionsResource").clone();
            resource
                .recv(this.conn, this.bytes)
                .await
                .map_err(|e| ReceiveError { conn: this.conn, error: format!("{e}") })
        })
    }
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

impl ExternalEffectAPI for CloseEffect {
    type Response = Result<(), CloseError>;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap(|this| async move {
            #[expect(clippy::expect_used)]
            let resource =
                resources.get::<ConnectionsResource>().expect("CloseEffect requires a ConnectionsResource").clone();
            resource.close(this.conn).await.map_err(|e| CloseError { conn: this.conn, error: format!("{e}") })
        })
    }
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
pub async fn create_connection(conn: &dyn amaru_ouroboros::ConnectionProvider) -> anyhow::Result<ConnectionId> {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let peer: Peer = std::env::var("PEER").unwrap_or_else(|_| "127.0.0.1:3000".to_string()).parse()?;
        Ok(conn.connect(peer, Duration::from_secs(5)).await?)
    })
    .await?
}
