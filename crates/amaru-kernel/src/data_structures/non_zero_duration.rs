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

use std::{fmt, num::NonZeroU64, time::Duration};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, thiserror::Error)]
#[error("duration must be non-zero")]
pub struct ZeroDurationError;

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
