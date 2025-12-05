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

use crate::{
    bytes::DebugBytes,
    socket::{ConnectionId, ConnectionResource},
    socket_addr::ToSocketAddrs,
};
use pure_stage::{BoxFuture, Effects, ExternalEffect, ExternalEffectAPI, Resources, SendData};

pub trait NetworkOps {
    fn connect(&self, addr: ToSocketAddrs) -> BoxFuture<'static, Result<ConnectionId, String>>;
    fn send(&self, conn: ConnectionId, data: DebugBytes) -> BoxFuture<'static, Result<(), String>>;
    fn recv(
        &self,
        conn: ConnectionId,
        bytes: usize,
    ) -> BoxFuture<'static, Result<DebugBytes, String>>;
    fn close(&self, conn: ConnectionId) -> BoxFuture<'static, Result<(), String>>;
}

pub struct Network<'a, T>(&'a Effects<T>);

impl<'a, T> Network<'a, T> {
    pub fn new(eff: &'a Effects<T>) -> Self {
        Network(eff)
    }
}

impl<T> NetworkOps for Network<'_, T> {
    fn connect(&self, addr: ToSocketAddrs) -> BoxFuture<'static, Result<ConnectionId, String>> {
        self.0.external(ConnectEffect { addr })
    }

    fn send(&self, conn: ConnectionId, data: DebugBytes) -> BoxFuture<'static, Result<(), String>> {
        self.0.external(SendEffect { conn, data })
    }

    fn recv(
        &self,
        conn: ConnectionId,
        bytes: usize,
    ) -> BoxFuture<'static, Result<DebugBytes, String>> {
        self.0.external(RecvEffect { conn, bytes })
    }

    fn close(&self, conn: ConnectionId) -> BoxFuture<'static, Result<(), String>> {
        self.0.external(CloseEffect { conn })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ConnectEffect {
    pub addr: ToSocketAddrs,
}

impl ExternalEffect for ConnectEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap(async move {
            #[expect(clippy::expect_used)]
            let resource = resources
                .get::<ConnectionResource>()
                .expect("ConnectEffect requires a ConnectionResource")
                .clone();
            let addr = self
                .addr
                .resolve()
                .await
                .map_err(|e| format!("failed to resolve address {:?}: {:#}", self.addr, e))?;
            resource
                .connect(addr)
                .await
                .map_err(|e| format!("failed to connect to {:?}: {:#}", self.addr, e))
        })
    }
}

impl ExternalEffectAPI for ConnectEffect {
    type Response = Result<ConnectionId, String>;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SendEffect {
    pub conn: ConnectionId,
    pub data: DebugBytes,
}

impl ExternalEffect for SendEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap(async move {
            #[expect(clippy::expect_used)]
            let resource = resources
                .get::<ConnectionResource>()
                .expect("SendEffect requires a ConnectionResource")
                .clone();
            resource
                .send(self.conn, self.data.into())
                .await
                .map_err(|e| format!("failed to send data on connection {}: {:#}", self.conn, e))
        })
    }
}

impl ExternalEffectAPI for SendEffect {
    type Response = Result<(), String>;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecvEffect {
    pub conn: ConnectionId,
    pub bytes: usize,
}

impl ExternalEffect for RecvEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap(async move {
            #[expect(clippy::expect_used)]
            let resource = resources
                .get::<ConnectionResource>()
                .expect("RecvEffect requires a ConnectionResource")
                .clone();
            resource
                .recv(self.conn, self.bytes)
                .await
                .map(|data| data.into())
                .map_err(|e| format!("failed to recv data on connection {}: {:#}", self.conn, e))
        })
    }
}

impl ExternalEffectAPI for RecvEffect {
    type Response = Result<DebugBytes, String>;
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
                .get::<ConnectionResource>()
                .expect("CloseEffect requires a ConnectionResource")
                .clone();
            resource
                .close(self.conn)
                .await
                .map_err(|e| format!("failed to close connection {}: {:#}", self.conn, e))
        })
    }
}

impl ExternalEffectAPI for CloseEffect {
    type Response = Result<(), String>;
}
