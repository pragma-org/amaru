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
    sync::{
        LazyLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

/// A simulation clock that is driven explicitly by the simulation.
pub trait Clock {
    /// Get the current time.
    fn now(&self) -> Instant;

    /// Advance the clock to the given time.
    ///
    /// This method is expected to panic when attempting to go backwards in time.
    fn advance_to(&self, instant: Instant);
}

impl Clock for AtomicU64 {
    fn now(&self) -> Instant {
        *EPOCH + Duration::from_nanos(self.load(Ordering::Relaxed))
    }

    fn advance_to(&self, instant: Instant) {
        let nanos = instant.saturating_since(*EPOCH).as_nanos();
        assert!(
            nanos < u64::MAX as u128,
            "simulation is not supposed to run for more than 584 years"
        );
        let nanos = nanos as u64;
        let old = self.swap(nanos, Ordering::Relaxed);
        assert!(old <= nanos, "clock is not monotonic");
    }
}

/// A point in time in the simulation.
///
/// Note that this is an opaque type that serialises and prints as a duration since the [`EPOCH`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Instant(tokio::time::Instant);

impl std::fmt::Debug for Instant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Instant")
            .field(&self.saturating_since(*EPOCH))
            .finish()
    }
}

impl<'de> serde::Deserialize<'de> for Instant {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let duration = Duration::deserialize(deserializer)?;
        Ok(*EPOCH + duration)
    }
}

impl serde::Serialize for Instant {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let duration = self.saturating_since(*EPOCH);
        duration.serialize(serializer)
    }
}

impl Instant {
    pub(crate) fn from_tokio(instant: tokio::time::Instant) -> Self {
        Self(instant)
    }

    pub(crate) fn now() -> Self {
        Self(tokio::time::Instant::now())
    }

    pub fn pretty(self, now: Self) -> String {
        if let Some(duration) = self.checked_since(now) {
            format!("{:?} in the future", duration)
        } else if let Some(duration) = now.checked_since(self) {
            format!("{:?} ago", duration)
        } else {
            "(time bug)".to_string()
        }
    }

    pub fn saturating_since(&self, other: Self) -> Duration {
        self.0.duration_since(other.0)
    }

    pub fn checked_since(&self, other: Self) -> Option<Duration> {
        self.0.checked_duration_since(other.0)
    }

    pub fn at_offset(offset: Duration) -> Self {
        *EPOCH + offset
    }
}

impl std::ops::Add<Duration> for Instant {
    type Output = Instant;

    #[allow(clippy::expect_used)]
    fn add(self, duration: Duration) -> Self {
        Instant(
            self.0
                .checked_add(duration)
                .expect("simulation is not supposed to run for more than 290 billion years"),
        )
    }
}

impl std::ops::Sub<Duration> for Instant {
    type Output = Instant;

    #[allow(clippy::expect_used)]
    fn sub(self, duration: Duration) -> Self {
        Instant(
            self.0
                .checked_sub(duration)
                .expect("simulation is not supposed to run for more than 290 billion years"),
        )
    }
}

/// The concrete value of the epoch is completely opaque and irrelevant, we only persist
/// durations. The only guarantee needed is that the epoch stays constant for the duration of
/// the simulation.
pub static EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);

#[test]
fn instant() {
    let now = Instant::now();
    let later = now + Duration::from_secs(1);

    assert_eq!(later.checked_since(now).unwrap(), Duration::from_secs(1));
    assert_eq!(now.checked_since(later), None);

    assert_eq!(later.saturating_since(now), Duration::from_secs(1));
    assert_eq!(now.saturating_since(later), Duration::from_secs(0));

    assert_eq!(now + Duration::from_secs(1), later);
    assert_eq!(later - Duration::from_secs(1), now);
}
