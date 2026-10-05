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
    marker::PhantomData,
    time::Duration,
};

use bytes::{Buf, BufMut, Bytes, BytesMut, TryGetError};

mod check;
mod limits;
mod miniprotocol;
mod pipeline;
mod want_next;

pub use check::ProtoSpec;
pub use limits::{
    BLOCK_FETCH_INGRESS, CHAIN_SYNC_INGRESS, HANDSHAKE_INGRESS, KEEP_ALIVE_INGRESS, PEER_SHARING_INGRESS,
    TX_SUBMISSION_INGRESS, ingress_limit,
};
pub use miniprotocol::{
    Inputs, Internal, Miniprotocol, Outcome, ProtocolState, Pull, StageState, Timeout, from_wire, miniprotocol, outcome,
};
pub(crate) use pipeline::{MuxClient, Pipelined, ToMux, WantNext, drive, pipelined};
pub use want_next::{WantNextError, check_want_next};

/// Input to a protocol step
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Input<L, R> {
    Local(L),
    Remote(R),
}

// TODO(network) find right value
pub const NETWORK_SEND_TIMEOUT: Duration = Duration::from_secs(1);

/// Slowest sustained rate a single lane is still only slow.
///
/// Per-lane bandwidth is assumed to be at least 500 kbps. A lane below that is
/// faulted, not scored as adversarial. Bits per second.
pub const MIN_PEER_BANDWIDTH_BPS: u64 = 500_000;

#[derive(serde::Serialize, serde::Deserialize, PartialEq)]
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

impl Display for Role {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Role::Initiator => write!(f, "initiator"),
            Role::Responder => write!(f, "responder"),
        }
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct Initiator;
impl sealed::Sealed for Initiator {}
impl RoleT for Initiator {
    type Opposite = Responder;

    const ROLE: Option<Role> = Some(Role::Initiator);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct Responder;
impl sealed::Sealed for Responder {}
impl RoleT for Responder {
    type Opposite = Initiator;

    const ROLE: Option<Role> = Some(Role::Responder);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
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

pub enum KnownProtocol {
    Handshake,
    ChainSync,
    BlockFetch,
    TxSubmission,
    KeepAlive,
    PeerShare,
}

impl KnownProtocol {
    pub fn protocol_id<R: RoleT>(&self) -> ProtocolId<R> {
        match self {
            KnownProtocol::Handshake => PROTO_HANDSHAKE.for_role_t(),
            KnownProtocol::ChainSync => PROTO_N2N_CHAIN_SYNC.for_role_t(),
            KnownProtocol::BlockFetch => PROTO_N2N_BLOCK_FETCH.for_role_t(),
            KnownProtocol::TxSubmission => PROTO_N2N_TX_SUB.for_role_t(),
            KnownProtocol::KeepAlive => PROTO_N2N_KEEP_ALIVE.for_role_t(),
            KnownProtocol::PeerShare => PROTO_N2N_PEER_SHARE.for_role_t(),
        }
    }
}

impl<R: RoleT> TryFrom<ProtocolId<R>> for KnownProtocol {
    type Error = ProtocolId<R>;

    fn try_from(protocol_id: ProtocolId<R>) -> Result<Self, Self::Error> {
        match protocol_id.for_role_t::<Initiator>() {
            PROTO_HANDSHAKE => Ok(KnownProtocol::Handshake),
            PROTO_N2N_CHAIN_SYNC => Ok(KnownProtocol::ChainSync),
            PROTO_N2N_BLOCK_FETCH => Ok(KnownProtocol::BlockFetch),
            PROTO_N2N_TX_SUB => Ok(KnownProtocol::TxSubmission),
            PROTO_N2N_KEEP_ALIVE => Ok(KnownProtocol::KeepAlive),
            PROTO_N2N_PEER_SHARE => Ok(KnownProtocol::PeerShare),
            _ => Err(protocol_id),
        }
    }
}

/// Bytes on the wire for one message of `payload` bytes.
///
/// Each segment carries [`crate::mux::SEGMENT_HEADER_LEN`] extra bytes. An empty
/// payload contributes nothing. Other lanes are not included.
fn wire_bytes(payload: usize) -> u64 {
    if payload == 0 {
        return 0;
    }
    let payload = u64::try_from(payload).unwrap_or(u64::MAX);
    let segment = u64::try_from(crate::mux::MAX_SEGMENT_SIZE).unwrap_or(u64::MAX);
    let header = u64::try_from(crate::mux::SEGMENT_HEADER_LEN).unwrap_or(u64::MAX);
    let segments = payload.div_ceil(segment);
    payload.saturating_add(segments.saturating_mul(header))
}

/// Time to drain one full egress buffer at [`MIN_PEER_BANDWIDTH_BPS`].
///
/// The buffer holds [`crate::mux::MAX_SEGMENT_SIZE`] bytes. One handler runs
/// on a lane and submits the next message only after the previous message's
/// last byte is already in that buffer, so a call waits on at most one
/// in-flight segment and one full buffer.
pub(crate) fn egress_buffer_drain() -> Duration {
    let bytes = u128::from(u64::try_from(crate::mux::MAX_SEGMENT_SIZE).unwrap_or(u64::MAX));
    let nanos = bytes.saturating_mul(8).saturating_mul(1_000_000_000) / u128::from(MIN_PEER_BANDWIDTH_BPS);
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
}

/// How long the mux may take to accept one message of `payload_len` bytes.
///
/// A lane is assumed to have at least [`MIN_PEER_BANDWIDTH_BPS`]. A lane below
/// that rate is faulted and is not recorded as adversarial. Other lanes are
/// not part of this budget.
///
/// The slack is one full egress buffer, not a fixed second: one sequential
/// handler per lane means the previous message's last byte is already in the
/// one-segment buffer. The rest is this message's own wire time at 500 kbps,
/// including segment headers.
pub fn egress_admission_deadline(payload_len: usize) -> Duration {
    let millis = wire_bytes(payload_len).saturating_mul(8).saturating_mul(1000).div_ceil(MIN_PEER_BANDWIDTH_BPS);
    Duration::from_millis(millis) + egress_buffer_drain()
}

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
            (Role::Initiator, Role::Responder) | (Role::Responder, Role::Initiator) => self.opposite().erase(),
        }
    }

    pub const fn for_role_t<R2: RoleT>(self) -> ProtocolId<R2> {
        match R2::ROLE {
            Some(Role::Initiator) => ProtocolId(self.for_role(Role::Initiator).0, PhantomData),
            Some(Role::Responder) => ProtocolId(self.for_role(Role::Responder).0, PhantomData),
            None => ProtocolId(self.0, PhantomData),
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

#[cfg(test)]
mod egress_deadline_tests {
    use super::*;

    #[test]
    fn block_deadline_is_one_buffer_drain_plus_its_own_wire_time() {
        let block = crate::blockfetch::BLOCKFETCH_MAX_BLOCK_WIRE_BYTES;
        assert_eq!(block, 96 * 1024);
        // wire(98304) = 98304 + 2 * 8 = 98320
        // ceil(98320 * 8 * 1000 / 500_000) = 1574 ms
        // drain(65535) = 65535 * 8 * 1e9 / 500_000 = 1_048_560_000 ns
        assert_eq!(wire_bytes(block), 98_320);
        assert_eq!(egress_buffer_drain(), Duration::from_nanos(1_048_560_000));
        let deadline = egress_admission_deadline(block);
        assert_eq!(deadline, Duration::from_millis(1_574) + Duration::from_nanos(1_048_560_000));
        assert_eq!(deadline, Duration::from_nanos(2_622_560_000));
    }

    #[test]
    fn just_over_one_segment_is_longer_than_a_one_second_slack() {
        // wire(65537) = 65537 + 2 * 8 = 65553
        // ceil(65553 * 8 * 1000 / 500_000) = 1049 ms
        // A fixed 1 s slack made the deadline 2049 ms. One full buffer is 1048.56 ms.
        let payload = crate::mux::MAX_SEGMENT_SIZE + 2;
        assert_eq!(payload, 65_537);
        assert_eq!(wire_bytes(payload), 65_553);
        let deadline = egress_admission_deadline(payload);
        assert_eq!(deadline, Duration::from_millis(1_049) + egress_buffer_drain());
        assert_eq!(deadline, Duration::from_nanos(2_097_560_000));
        assert!(deadline > Duration::from_millis(2_049));
    }
}
