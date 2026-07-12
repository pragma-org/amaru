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
    cell::RefCell,
    fmt::{Display, Formatter},
    sync::{
        LazyLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use parking_lot::Mutex;

use crate::drop_guard::DropGuard;

/// A simulation clock that is driven explicitly by the simulation.
pub trait Clock {
    /// Get the current time, associating the returned `Instant` with the given global epoch offset
    /// (from e.g. `GlobalParameters::system_start`).
    fn now(&self, global_epoch_offset: Duration) -> Instant;

    /// Advance the clock to the given time.
    ///
    /// This method is expected to panic when attempting to go backwards in time.
    fn advance_to(&self, instant: Instant);
}

impl Clock for AtomicU64 {
    fn now(&self, global_epoch_offset: Duration) -> Instant {
        Instant { inner: EPOCH.inner + Duration::from_nanos(self.load(Ordering::Relaxed)), global_epoch_offset }
    }

    fn advance_to(&self, instant: Instant) {
        let nanos = instant.saturating_since(*EPOCH).as_nanos();
        assert!(nanos < u64::MAX as u128, "simulation is not supposed to run for more than 584 years");
        let nanos = nanos as u64;
        let old = self.swap(nanos, Ordering::Relaxed);
        assert!(old <= nanos, "clock is not monotonic");
    }
}

impl Clock for Mutex<Instant> {
    fn now(&self, global_epoch_offset: Duration) -> Instant {
        let mut inst = *self.lock();
        inst.global_epoch_offset = global_epoch_offset;
        inst
    }

    fn advance_to(&self, instant: Instant) {
        *self.lock() = instant;
    }
}

/// A point in time in the simulation.
///
/// Note that this is an opaque type that serialises and prints as a duration since the [`EPOCH`].
#[derive(Clone, Copy, Eq, PartialOrd, Ord)]
pub struct Instant {
    pub(crate) inner: tokio::time::Instant,
    /// Offset of the simulation EPOCH relative to a global reference (e.g. Cardano system_start).
    /// This is baked into Instants created for a particular simulation configuration.
    pub(crate) global_epoch_offset: Duration,
}

thread_local! {
    static TOLERANCE: RefCell<Duration> = const { RefCell::new(Duration::from_nanos(0)) };
}

impl PartialEq for Instant {
    fn eq(&self, other: &Self) -> bool {
        let tolerance = TOLERANCE.with(|tolerance| *tolerance.borrow());
        if tolerance.is_zero() {
            self.inner == other.inner
        } else if self > other {
            self.inner - other.inner <= tolerance
        } else {
            other.inner - self.inner <= tolerance
        }
    }
}

impl std::fmt::Debug for Instant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Instant").field(&self.saturating_since(*EPOCH)).finish()
    }
}

impl Display for Instant {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let duration = self.saturating_since(*EPOCH);
        write!(f, "{:.6?}", duration)
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
    pub fn with_tolerance_for_test(tolerance: Duration) -> DropGuard<Duration, fn(Duration)> {
        fn restore(tolerance: Duration) {
            TOLERANCE.with_borrow_mut(|t2| *t2 = tolerance)
        }
        TOLERANCE.with_borrow_mut(|t| DropGuard::new(std::mem::replace(t, tolerance), restore as fn(Duration)))
    }

    pub fn from_tokio(instant: tokio::time::Instant, global_epoch_offset: Duration) -> Self {
        Instant { inner: instant, global_epoch_offset }
    }

    pub(crate) fn to_tokio(self) -> tokio::time::Instant {
        self.inner
    }

    pub(crate) fn now() -> Self {
        Instant { inner: tokio::time::Instant::now(), global_epoch_offset: Duration::ZERO }
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
        let since = self.inner.duration_since(other.inner);
        if self.global_epoch_offset == other.global_epoch_offset {
            since
        } else if self.global_epoch_offset > other.global_epoch_offset {
            since + (self.global_epoch_offset - other.global_epoch_offset)
        } else {
            since.saturating_sub(other.global_epoch_offset - self.global_epoch_offset)
        }
    }

    pub fn checked_since(&self, other: Self) -> Option<Duration> {
        let since = self.inner.checked_duration_since(other.inner)?;
        if self.global_epoch_offset == other.global_epoch_offset {
            Some(since)
        } else if self.global_epoch_offset > other.global_epoch_offset {
            since.checked_add(self.global_epoch_offset.checked_sub(other.global_epoch_offset)?)
        } else {
            since.checked_sub(other.global_epoch_offset.checked_sub(self.global_epoch_offset)?)
        }
    }

    pub fn at_offset(offset: Duration) -> Self {
        Instant { inner: EPOCH.inner + offset, global_epoch_offset: Duration::ZERO }
    }

    /// Returns the duration since the configured global epoch (see
    /// [`SimulationBuilder::with_global_epoch_offset`]).
    ///
    /// This is `sim_elapsed + global_offset`, providing a useful input for `EraHistory` slot/time
    /// conversions.
    pub fn duration_since_global_epoch(&self) -> Duration {
        // sim elapsed relative to this Instant's EPOCH (which has zero offset) + the baked offset
        let sim_elapsed = self.saturating_since(*EPOCH);
        sim_elapsed + self.global_epoch_offset
    }
}

impl std::ops::Add<Duration> for Instant {
    type Output = Instant;

    #[expect(clippy::expect_used)]
    fn add(self, duration: Duration) -> Self {
        Instant {
            inner: self
                .inner
                .checked_add(duration)
                .expect("simulation is not supposed to run for more than 290 billion years"),
            global_epoch_offset: self.global_epoch_offset,
        }
    }
}

impl std::ops::Sub<Duration> for Instant {
    type Output = Instant;

    #[expect(clippy::expect_used)]
    fn sub(self, duration: Duration) -> Self {
        Instant {
            inner: self
                .inner
                .checked_sub(duration)
                .expect("simulation is not supposed to run for more than 290 billion years"),
            global_epoch_offset: self.global_epoch_offset,
        }
    }
}

/// The concrete value of the epoch is completely opaque and irrelevant, we only persist
/// durations. The only guarantee needed is that the epoch stays constant for the duration of
/// the simulation.
pub static EPOCH: LazyLock<Instant> =
    LazyLock::new(|| Instant { inner: tokio::time::Instant::now(), global_epoch_offset: Duration::ZERO });

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
