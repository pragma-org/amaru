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

use amaru_kernel::cbor;

/// Parameters used to control the behavior of the transaction submission responder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ResponderParams {
    /// How many tx ids we can keep unacknowledged at any moment.
    pub max_window: u16,
    /// Per-`RequestTxs` byte budget for the cumulative advertised size of tx bodies fetched in
    /// a single round. Greedy: the responder stops adding tx_ids to the request once including
    /// the next one would exceed this budget (always serving at least one to avoid starving on
    /// an unusually large advertisement).
    pub fetch_batch_bytes: u64,
}

/// Default outstanding tx-id window, i.e. how many tx ids we can keep unacknowledged at any moment.
pub const DEFAULT_MAX_OUTSTANDING_TX_IDS: u16 = 10;

/// Default per-`RequestTxs` byte budget. Matches Haskell V2's `txsSizeInflightPerPeer`
/// (`6 × max_TX_SIZE = 6 × 65_540 = 393_240`) in
/// `ouroboros-network/lib/Ouroboros/Network/TxSubmission/Inbound/V2/Policy.hs`.
pub const DEFAULT_FETCH_BATCH_BYTES: u64 = 393_240;

impl Default for ResponderParams {
    fn default() -> Self {
        Self { max_window: DEFAULT_MAX_OUTSTANDING_TX_IDS, fetch_batch_bytes: DEFAULT_FETCH_BATCH_BYTES }
    }
}

impl ResponderParams {
    pub fn new(max_window: u16, fetch_batch_bytes: u64) -> Self {
        Self { max_window, fetch_batch_bytes }
    }
}

/// Whether the transaction submission initiator should block until it can fulfill a server request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Blocking {
    Yes,
    No,
}

impl cbor::Encode<()> for Blocking {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        match self {
            Blocking::Yes => e.encode(true)?,
            Blocking::No => e.encode(false)?,
        };
        Ok(())
    }
}

impl<'b> cbor::Decode<'b, ()> for Blocking {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut ()) -> Result<Self, cbor::decode::Error> {
        let value = bool::decode(d, ctx)?;
        match value {
            true => Ok(Blocking::Yes),
            false => Ok(Blocking::No),
        }
    }
}

impl From<Blocking> for bool {
    fn from(value: Blocking) -> Self {
        match value {
            Blocking::Yes => true,
            Blocking::No => false,
        }
    }
}
