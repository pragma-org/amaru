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

use bytes::{Buf, BufMut, Bytes, BytesMut, TryGetError};
use std::{marker::PhantomData, time::Duration};

mod check;
mod miniprotocol;

pub use check::ProtoSpec;
pub use miniprotocol::{
    Inputs, Miniprotocol, Outcome, ProtocolState, StageState, miniprotocol, outcome,
};

/// Input to a protocol step
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Input<L, R> {
    Local(L),
    Remote(R),
}

// TODO(network) find right value
pub const NETWORK_SEND_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(serde::Serialize, serde::Deserialize)]
pub struct ProtocolId<T: RoleT>(u16, PhantomData<T>);

impl<T: RoleT> ProtocolId<T> {
    pub fn encode(self, buffer: &mut BytesMut) {
        buffer.put_u16(self.0);
    }

    pub fn decode(buffer: &mut Bytes) -> Result<Self, TryGetError> {
        Ok(Self(buffer.try_get_u16()?, PhantomData))
    }
}

impl<T: RoleT> std::fmt::Display for ProtocolId<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl<T: RoleT> std::hash::Hash for ProtocolId<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl<T: RoleT> Ord for ProtocolId<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}

impl<T: RoleT> PartialOrd for ProtocolId<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: RoleT> Eq for ProtocolId<T> {}

impl<T: RoleT> PartialEq for ProtocolId<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0 && self.1 == other.1
    }
}

impl<T: RoleT> std::fmt::Debug for ProtocolId<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ProtocolId").field(&self.0).finish()
    }
}

impl<T: RoleT> Copy for ProtocolId<T> {}

impl<R: RoleT> Clone for ProtocolId<R> {
    fn clone(&self) -> Self {
        *self
    }
}

const RESPONDER: u16 = 0x8000;

#[derive(Debug, PartialEq, Eq, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub enum Role {
    Initiator,
    Responder,
}

impl Role {
    pub const fn opposite(self) -> Role {
        match self {
            Role::Initiator => Role::Responder,
            Role::Responder => Role::Initiator,
        }
    }
}

mod sealed {
    pub trait Sealed {}
}
pub trait RoleT:
    Clone
    + Copy
    + std::fmt::Debug
    + std::hash::Hash
    + std::cmp::Ord
    + std::cmp::PartialOrd
    + std::cmp::Eq
    + std::cmp::PartialEq
    + serde::Serialize
    + serde::de::DeserializeOwned
    + Send
    + Sync
    + 'static
    + sealed::Sealed
{
    type Opposite: RoleT;

    const ROLE: Option<Role>;
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct Initiator;
impl sealed::Sealed for Initiator {}
impl RoleT for Initiator {
    type Opposite = Responder;

    const ROLE: Option<Role> = Some(Role::Initiator);
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct Responder;
impl sealed::Sealed for Responder {}
impl RoleT for Responder {
    type Opposite = Initiator;

    const ROLE: Option<Role> = Some(Role::Responder);
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct Erased;
impl sealed::Sealed for Erased {}
impl RoleT for Erased {
    type Opposite = Erased;

    const ROLE: Option<Role> = None;
}

pub const PROTO_HANDSHAKE: ProtocolId<Initiator> = ProtocolId::<Initiator>(0, PhantomData);

pub const PROTO_N2N_CHAIN_SYNC: ProtocolId<Initiator> = ProtocolId::<Initiator>(2, PhantomData);
pub const PROTO_N2N_BLOCK_FETCH: ProtocolId<Initiator> = ProtocolId::<Initiator>(3, PhantomData);
pub const PROTO_N2N_TX_SUB: ProtocolId<Initiator> = ProtocolId::<Initiator>(4, PhantomData);
pub const PROTO_N2N_KEEP_ALIVE: ProtocolId<Initiator> = ProtocolId::<Initiator>(8, PhantomData);
pub const PROTO_N2N_PEER_SHARE: ProtocolId<Initiator> = ProtocolId::<Initiator>(10, PhantomData);

// The below are only for information regarding the allocated numbers, Amaru will not implement N2C protocols.

// pub const PROTO_N2C_CHAIN_SYNC: ProtocolId<Initiator> = ProtocolId::<Initiator>(5, PhantomData);
// pub const PROTO_N2C_TX_SUB: ProtocolId<Initiator> = ProtocolId::<Initiator>(6, PhantomData);
// pub const PROTO_N2C_STATE_QUERY: ProtocolId<Initiator> = ProtocolId::<Initiator>(7, PhantomData);
// pub const PROTO_N2C_TX_MON: ProtocolId<Initiator> = ProtocolId::<Initiator>(9, PhantomData);

#[cfg(test)]
pub const PROTO_TEST: ProtocolId<Initiator> = ProtocolId::<Initiator>(257, PhantomData);

impl<R: RoleT> ProtocolId<R> {
    pub const fn is_initiator(self) -> bool {
        self.0 & RESPONDER == 0
    }

    pub const fn is_responder(self) -> bool {
        !self.is_initiator()
    }

    pub const fn opposite(self) -> ProtocolId<R::Opposite> {
        ProtocolId(self.0 ^ RESPONDER, PhantomData)
    }

    pub const fn erase(self) -> ProtocolId<Erased> {
        ProtocolId(self.0, PhantomData)
    }

    pub const fn for_role(self, role: Role) -> ProtocolId<Erased> {
        match (role, self.role()) {
            (Role::Initiator, Role::Initiator) | (Role::Responder, Role::Responder) => self.erase(),
            (Role::Initiator, Role::Responder) | (Role::Responder, Role::Initiator) => {
                self.opposite().erase()
            }
        }
    }

    pub const fn role(self) -> Role {
        if let Some(role) = R::ROLE {
            role
        } else if self.is_initiator() {
            Role::Initiator
        } else {
            Role::Responder
        }
    }
}

impl ProtocolId<Initiator> {
    pub const fn responder(self) -> ProtocolId<Responder> {
        ProtocolId(self.0 | RESPONDER, PhantomData)
    }
}

impl ProtocolId<Responder> {
    pub const fn initiator(self) -> ProtocolId<Initiator> {
        ProtocolId(self.0 & !RESPONDER, PhantomData)
    }
}
