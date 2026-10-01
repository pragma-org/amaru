// Copyright 2026 PRAGMA
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

//! Node-to-node mux ingress limits.
//!
//! A peer may pipeline until the receiver's ingress buffer is full, and an overrun closes the
//! connection. The sizes are the `maximumIngressQueue` values from ouroboros-network
//! (`Cardano.Network.NodeToNode`), which match network-spec table 3.15 except block-fetch: the
//! spec prints 230_686_940 while cardano-node evaluates
//! `addSafetyMargin (max (10 * 2_097_154) (100 * 90_112))`. Handshake is the 4×1440-byte
//! transmission unit from `byteLimitsHandshake` (the spec leaves that row blank).
//!
//! The responder bit (`0x8000`) selects the direction on the wire. It does not select a
//! different size: both directions of a mini-protocol share one limit.

use super::{ProtocolId, RoleT};
use crate::protocol::KnownProtocol;

const fn safety(bytes: usize) -> usize {
    bytes + bytes / 10
}

const fn larger(left: usize, right: usize) -> usize {
    if left > right { left } else { right }
}

/// Handshake messages must fit in one TCP initial window (4 × 1440).
pub const HANDSHAKE_INGRESS: usize = 4 * 1440;

/// Chain-sync: `addSafetyMargin (chainSyncPipeliningHighMark * 1400)` with high mark 300.
pub const CHAIN_SYNC_INGRESS: usize = safety(300 * 1400);

/// Block-fetch: cardano-node's `blockFetchProtocolLimits` at the default pipeline depth.
pub const BLOCK_FETCH_INGRESS: usize = safety(larger(10 * 2_097_154, 100 * 90_112));

/// Tx-submission v2: `addSafetyMargin (10 * (44 + 65_540))`.
pub const TX_SUBMISSION_INGRESS: usize = safety(10 * (44 + 65_540));

/// Keep-alive: `addSafetyMargin 1280`.
pub const KEEP_ALIVE_INGRESS: usize = safety(1280);

/// Peer-sharing: one TCP initial window, enough for 255 addresses plus CBOR overhead.
pub const PEER_SHARING_INGRESS: usize = 4 * 1440;

const _: () = {
    assert!(HANDSHAKE_INGRESS == 5_760);
    assert!(CHAIN_SYNC_INGRESS == 462_000);
    assert!(BLOCK_FETCH_INGRESS == 23_068_694);
    assert!(TX_SUBMISSION_INGRESS == 721_424);
    assert!(KEEP_ALIVE_INGRESS == 1_408);
    assert!(PEER_SHARING_INGRESS == 5_760);
};

/// Ingress buffer for `protocol`, ignoring the responder bit.
///
/// Unknown mini-protocol numbers return 0. Callers only pass the node-to-node protocols this
/// node registers; a 0 limit makes the mux drop bytes for that id.
pub fn ingress_limit<R: RoleT>(protocol: ProtocolId<R>) -> usize {
    use KnownProtocol::*;
    let Ok(proto) = KnownProtocol::try_from(protocol) else {
        return 0;
    };
    match proto {
        Handshake => HANDSHAKE_INGRESS,
        ChainSync => CHAIN_SYNC_INGRESS,
        BlockFetch => BLOCK_FETCH_INGRESS,
        TxSubmission => TX_SUBMISSION_INGRESS,
        KeepAlive => KEEP_ALIVE_INGRESS,
        PeerShare => PEER_SHARING_INGRESS,
    }
}
