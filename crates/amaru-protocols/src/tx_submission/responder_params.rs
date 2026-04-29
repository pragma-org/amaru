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

use std::time::Duration;

use amaru_kernel::cbor;

/// Parameters used to control the behavior of the transaction submission responder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ResponderParams {
    /// How many tx ids we can keep unacknowledged at any moment.
    pub max_window: u16,
    /// Per-`RequestTxs` byte budget for the cumulative size of tx bodies fetched in a single round.
    pub fetch_batch_bytes: u64,
    /// How long to wait for a peer to deliver `ReplyTxs` after we sent `RequestTxs`. On expiry
    /// the responder treats the peer as misbehaving and terminates the connection.
    pub inflight_fetch_timeout: Duration,
    /// How long to wait between mempool capacity rechecks while back-pressured.
    pub back_pressure_recheck_interval: Duration,
    /// How long to wait for the mempool stage to respond to a batch insert.
    pub mempool_insert_timeout: Duration,
}

/// Default outstanding tx-id window, i.e. how many tx ids we can keep unacknowledged at any moment.
pub const DEFAULT_MAX_OUTSTANDING_TX_IDS: u16 = 10;

/// Default per-`RequestTxs` byte budget.
/// This value is `6 × max_TX_SIZE = 6 × 65_540 = 393_240`
pub const DEFAULT_FETCH_BATCH_BYTES: u64 = 393_240;

/// Default in-flight body fetch timeout.
pub const DEFAULT_INFLIGHT_FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// Default back-pressure recheck interval.
pub const DEFAULT_BACK_PRESSURE_RECHECK_INTERVAL: Duration = Duration::from_millis(500);

/// Default timeout for the mempool stage to respond to a batch insert call.
pub const DEFAULT_MEMPOOL_INSERT_TIMEOUT: Duration = Duration::from_secs(5);

impl Default for ResponderParams {
    fn default() -> Self {
        Self {
            max_window: DEFAULT_MAX_OUTSTANDING_TX_IDS,
            fetch_batch_bytes: DEFAULT_FETCH_BATCH_BYTES,
            inflight_fetch_timeout: DEFAULT_INFLIGHT_FETCH_TIMEOUT,
            back_pressure_recheck_interval: DEFAULT_BACK_PRESSURE_RECHECK_INTERVAL,
            mempool_insert_timeout: DEFAULT_MEMPOOL_INSERT_TIMEOUT,
        }
    }
}

impl ResponderParams {
    pub fn new(max_window: u16, fetch_batch_bytes: u64) -> Self {
        Self { max_window, fetch_batch_bytes, ..Self::default() }
    }

    pub fn with_inflight_fetch_timeout(mut self, d: Duration) -> Self {
        self.inflight_fetch_timeout = d;
        self
    }

    pub fn with_back_pressure_recheck_interval(mut self, d: Duration) -> Self {
        self.back_pressure_recheck_interval = d;
        self
    }

    pub fn with_mempool_insert_timeout(mut self, d: Duration) -> Self {
        self.mempool_insert_timeout = d;
        self
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
