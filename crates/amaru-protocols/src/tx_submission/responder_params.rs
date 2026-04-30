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
    fmt,
    num::{NonZeroU16, NonZeroU64},
    time::Duration,
};

use amaru_kernel::cbor;

#[derive(Debug)]
pub struct ZeroDurationError;

impl fmt::Display for ZeroDurationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("duration must be non-zero")
    }
}

impl std::error::Error for ZeroDurationError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "Duration", into = "Duration")]
pub struct NonZeroDuration(Duration);

impl NonZeroDuration {
    pub const fn try_new(d: Duration) -> Option<Self> {
        if d.is_zero() { None } else { Some(Self(d)) }
    }

    pub const fn from_millis(ms: u64) -> Option<Self> {
        Self::try_new(Duration::from_millis(ms))
    }

    pub const fn from_secs(s: u64) -> Option<Self> {
        Self::try_new(Duration::from_secs(s))
    }

    pub const fn from_nonzero_millis(ms: NonZeroU64) -> Self {
        Self(Duration::from_millis(ms.get()))
    }

    pub const fn as_duration(self) -> Duration {
        self.0
    }
}

impl TryFrom<Duration> for NonZeroDuration {
    type Error = ZeroDurationError;

    fn try_from(value: Duration) -> Result<Self, Self::Error> {
        Self::try_new(value).ok_or(ZeroDurationError)
    }
}

impl From<NonZeroDuration> for Duration {
    fn from(value: NonZeroDuration) -> Self {
        value.0
    }
}

impl fmt::Display for NonZeroDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ResponderParams {
    pub max_window: NonZeroU16,
    pub fetch_batch_bytes: NonZeroU64,
    pub inflight_fetch_timeout: NonZeroDuration,
    pub back_pressure_recheck_interval: NonZeroDuration,
    pub mempool_insert_timeout: NonZeroDuration,
}

pub const DEFAULT_MAX_OUTSTANDING_TX_IDS: NonZeroU16 =
    NonZeroU16::new(10).expect("DEFAULT_MAX_OUTSTANDING_TX_IDS must be non-zero");

pub const DEFAULT_FETCH_BATCH_BYTES: NonZeroU64 =
    NonZeroU64::new(393_240).expect("DEFAULT_FETCH_BATCH_BYTES must be non-zero");

pub const DEFAULT_INFLIGHT_FETCH_TIMEOUT: NonZeroDuration =
    NonZeroDuration::try_new(Duration::from_secs(10)).expect("DEFAULT_INFLIGHT_FETCH_TIMEOUT must be non-zero");

pub const DEFAULT_BACK_PRESSURE_RECHECK_INTERVAL: NonZeroDuration =
    NonZeroDuration::try_new(Duration::from_millis(500))
        .expect("DEFAULT_BACK_PRESSURE_RECHECK_INTERVAL must be non-zero");

pub const DEFAULT_MEMPOOL_INSERT_TIMEOUT: NonZeroDuration =
    NonZeroDuration::try_new(Duration::from_secs(5)).expect("DEFAULT_MEMPOOL_INSERT_TIMEOUT must be non-zero");

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
    pub fn new(max_window: NonZeroU16, fetch_batch_bytes: NonZeroU64) -> Self {
        Self { max_window, fetch_batch_bytes, ..Self::default() }
    }

    pub fn with_inflight_fetch_timeout(mut self, d: NonZeroDuration) -> Self {
        self.inflight_fetch_timeout = d;
        self
    }

    pub fn with_back_pressure_recheck_interval(mut self, d: NonZeroDuration) -> Self {
        self.back_pressure_recheck_interval = d;
        self
    }

    pub fn with_mempool_insert_timeout(mut self, d: NonZeroDuration) -> Self {
        self.mempool_insert_timeout = d;
        self
    }
}

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
