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
        Instant { inner: *EPOCH + Duration::from_nanos(self.load(Ordering::Relaxed)), global_epoch_offset }
    }

    fn advance_to(&self, instant: Instant) {
        let nanos = instant.inner.saturating_duration_since(*EPOCH).as_nanos();
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
#[derive(Clone, Copy, Eq)]
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
        let abs_diff = self.diff(*other).1;
        abs_diff <= tolerance
    }
}

impl PartialOrd for Instant {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Instant {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.diff(*other).0
    }
}

impl std::fmt::Debug for Instant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Instant").field(&self.inner.saturating_duration_since(*EPOCH)).finish()
    }
}

impl Display for Instant {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let duration = self.inner.saturating_duration_since(*EPOCH);
        write!(f, "{:.6?}", duration)
    }
}

impl<'de> serde::Deserialize<'de> for Instant {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let (duration, global_epoch_offset) = <(Duration, Duration)>::deserialize(deserializer)?;
        Ok(Self { inner: *EPOCH + duration, global_epoch_offset })
    }
}

impl serde::Serialize for Instant {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Wire format: `(sim_elapsed, global_epoch_offset)` as a 2-tuple of `Duration`s.
        //
        // - `sim_elapsed` is the inner time since [`EPOCH`] only (not folded with
        //   `global_epoch_offset`). That keeps schedule/trace times comparable to
        //   `Instant::at_offset(t, …)` after deserialize.
        // - `global_epoch_offset` is restored on deserialize so
        //   [`duration_since_global_epoch`](Self::duration_since_global_epoch) survives a roundtrip.
        (self.sim_elapsed(), self.global_epoch_offset).serialize(serializer)
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

    pub fn diff(&self, other: Self) -> (std::cmp::Ordering, Duration) {
        let left = self.duration_since_global_epoch();
        let right = other.duration_since_global_epoch();
        match left.cmp(&right) {
            std::cmp::Ordering::Less => (std::cmp::Ordering::Less, right - left),
            std::cmp::Ordering::Equal => (std::cmp::Ordering::Equal, Duration::ZERO),
            std::cmp::Ordering::Greater => (std::cmp::Ordering::Greater, left - right),
        }
    }

    pub fn saturating_since(&self, other: Self) -> Duration {
        if let (std::cmp::Ordering::Greater, dur) = self.diff(other) { dur } else { Duration::ZERO }
    }

    pub fn checked_since(&self, other: Self) -> Option<Duration> {
        match self.diff(other) {
            (std::cmp::Ordering::Less, _) => None,
            (_, dur) => Some(dur),
        }
    }

    pub fn at_offset(offset: Duration, global_epoch_offset: Duration) -> Self {
        Instant { inner: *EPOCH + offset, global_epoch_offset }
    }

    /// Simulation elapsed time since [`EPOCH`] (ignores [`Self::global_epoch_offset`]).
    ///
    /// This is what is stored as the first field of the serde wire format and what
    /// schedule / trace comparisons use for sim-relative time.
    pub fn sim_elapsed(&self) -> Duration {
        self.inner.saturating_duration_since(*EPOCH)
    }

    /// Returns the duration since the configured global epoch (see
    /// [`SimulationBuilder::with_global_epoch_offset`]).
    ///
    /// This is `sim_elapsed + global_offset`, providing a useful input for `EraHistory` slot/time
    /// conversions.
    pub fn duration_since_global_epoch(&self) -> Duration {
        self.sim_elapsed() + self.global_epoch_offset
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
pub static EPOCH: LazyLock<tokio::time::Instant> = LazyLock::new(tokio::time::Instant::now);

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use cbor4ii::serde::from_slice;
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn instant_arithmetic() {
        // Use at_offset (not wall Instant::now) so differences are exact and independent of EPOCH age.
        let now = Instant::at_offset(Duration::from_secs(100), Duration::from_secs(50));
        let later = now + Duration::from_secs(1);

        assert_eq!(later.checked_since(now).unwrap(), Duration::from_secs(1));
        assert_eq!(now.checked_since(later), None);

        assert_eq!(later.saturating_since(now), Duration::from_secs(1));
        assert_eq!(now.saturating_since(later), Duration::from_secs(0));

        assert_eq!(now + Duration::from_secs(1), later);
        assert_eq!(later - Duration::from_secs(1), now);
    }

    /// `Instant` serializes as `(sim_elapsed, global_epoch_offset)` — a 2-tuple of [`Duration`].
    ///
    /// JSON shape (serde's default `Duration` encoding):
    /// ```json
    /// [{"secs":1,"nanos":500},{"secs":3600,"nanos":0}]
    /// ```
    #[test]
    fn instant_serde_json_format() {
        let sim_elapsed = Duration::from_secs(1) + Duration::from_nanos(500);
        let global_epoch_offset = Duration::from_secs(3600);
        let instant = Instant::at_offset(sim_elapsed, global_epoch_offset);

        let json = serde_json::to_value(instant).expect("serialize Instant");
        assert_eq!(
            json,
            serde_json::json!([
                { "secs": 1, "nanos": 500 },
                { "secs": 3600, "nanos": 0 },
            ]),
            "wire format must be a 2-array of Duration objects (sim_elapsed, global_epoch_offset)"
        );
    }

    #[test]
    fn instant_serde_json_roundtrip() {
        let original =
            Instant::at_offset(Duration::from_secs(11) + Duration::from_millis(250), Duration::from_secs(70_419_600));

        let json = serde_json::to_string(&original).expect("serialize");
        let restored: Instant = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(restored, original);
        assert_eq!(restored.duration_since_global_epoch(), original.duration_since_global_epoch());
        assert_eq!(restored.global_epoch_offset, original.global_epoch_offset);
        assert_eq!(restored.inner.saturating_duration_since(*EPOCH), original.inner.saturating_duration_since(*EPOCH));
    }

    #[test]
    fn instant_cbor_roundtrip() {
        // Traces use cbor4ii; schedule IDs and clock entries must survive that path.
        let cases = [
            Instant::at_offset(Duration::ZERO, Duration::ZERO),
            Instant::at_offset(Duration::from_secs(5), Duration::ZERO),
            Instant::at_offset(Duration::from_secs(11), Duration::from_secs(70_419_600)),
            Instant::at_offset(Duration::from_millis(1), Duration::from_nanos(1)),
        ];

        for original in cases {
            let bytes = crate::serde::to_cbor(&original);
            let restored: Instant = from_slice(&bytes).expect("cbor deserialize");
            assert_eq!(restored, original, "cbor roundtrip for {original:?}");
            assert_eq!(restored.duration_since_global_epoch(), original.duration_since_global_epoch());
            assert_eq!(restored.global_epoch_offset, original.global_epoch_offset);
        }
    }

    /// Regression: sim_elapsed must not absorb `global_epoch_offset` on serialize.
    /// Otherwise traces deserialize as `Instant(R+t)` while tests build `Instant(t)`.
    #[test]
    fn instant_serialize_keeps_sim_elapsed_separate_from_global_offset() {
        let sim_elapsed = Duration::from_secs(11);
        let global_epoch_offset = Duration::from_secs(70_419_600);
        let instant = Instant::at_offset(sim_elapsed, global_epoch_offset);

        let (ser_elapsed, ser_offset): (Duration, Duration) =
            serde_json::from_value(serde_json::to_value(instant).unwrap()).unwrap();

        assert_eq!(ser_elapsed, sim_elapsed);
        assert_eq!(ser_offset, global_epoch_offset);
        assert_ne!(ser_elapsed, sim_elapsed + global_epoch_offset, "must not fold offset into sim_elapsed");
    }

    #[test]
    fn instant_eq_uses_duration_since_global_epoch() {
        // Same absolute Cardano time via different (sim_elapsed, offset) pairs.
        let a = Instant::at_offset(Duration::from_secs(5), Duration::from_secs(10));
        let b = Instant::at_offset(Duration::from_secs(15), Duration::ZERO);
        assert_eq!(a.duration_since_global_epoch(), Duration::from_secs(15));
        assert_eq!(b.duration_since_global_epoch(), Duration::from_secs(15));
        assert_eq!(a, b);
    }
}
