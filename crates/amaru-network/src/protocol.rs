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

use std::marker::PhantomData;

#[binrw::binrw]
#[brw(big)]
pub struct ProtocolId<T: Role>(u16, PhantomData<T>);

impl<T: Role> std::fmt::Display for ProtocolId<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl<T: Role> std::hash::Hash for ProtocolId<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl<T: Role> Ord for ProtocolId<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}

impl<T: Role> PartialOrd for ProtocolId<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: Role> Eq for ProtocolId<T> {}

impl<T: Role> PartialEq for ProtocolId<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0 && self.1 == other.1
    }
}

impl<T: Role> std::fmt::Debug for ProtocolId<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ProtocolId").field(&self.0).finish()
    }
}

impl<T: Role> Copy for ProtocolId<T> {}

impl<R: Role> Clone for ProtocolId<R> {
    fn clone(&self) -> Self {
        *self
    }
}

const RESPONDER: u16 = 0x8000;

mod sealed {
    pub trait Sealed {}
}
pub trait Role: sealed::Sealed {
    type Opposite: Role;
}

pub struct Initiator;
impl sealed::Sealed for Initiator {}
impl Role for Initiator {
    type Opposite = Responder;
}

pub struct Responder;
impl sealed::Sealed for Responder {}
impl Role for Responder {
    type Opposite = Initiator;
}

pub struct Erased;
impl sealed::Sealed for Erased {}
impl Role for Erased {
    type Opposite = Erased;
}

pub const PROTO_HANDSHAKE: ProtocolId<Initiator> = ProtocolId::<Initiator>(0, PhantomData);

pub const PROTO_N2N_CHAIN_SYNC: ProtocolId<Initiator> = ProtocolId::<Initiator>(2, PhantomData);
pub const PROTO_N2N_BLOCK_FETCH: ProtocolId<Initiator> = ProtocolId::<Initiator>(3, PhantomData);
pub const PROTO_N2N_TX_SUB: ProtocolId<Initiator> = ProtocolId::<Initiator>(4, PhantomData);
pub const PROTO_N2N_KEEP_ALIVE: ProtocolId<Initiator> = ProtocolId::<Initiator>(8, PhantomData);
pub const PROTO_N2N_PEER_SHARE: ProtocolId<Initiator> = ProtocolId::<Initiator>(10, PhantomData);

pub const PROTO_N2C_CHAIN_SYNC: ProtocolId<Initiator> = ProtocolId::<Initiator>(5, PhantomData);
pub const PROTO_N2C_TX_SUB: ProtocolId<Initiator> = ProtocolId::<Initiator>(6, PhantomData);
pub const PROTO_N2C_STATE_QUERY: ProtocolId<Initiator> = ProtocolId::<Initiator>(7, PhantomData);
pub const PROTO_N2C_TX_MON: ProtocolId<Initiator> = ProtocolId::<Initiator>(9, PhantomData);

impl<R: Role> ProtocolId<R> {
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
