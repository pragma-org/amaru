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

use std::time::Duration;

use rand::Rng;

/// How an [`ExternalEffect`](crate::ExternalEffect) occupies simulated time.
///
/// The Tokio runtime ignores this. The simulation samples (or declines to sample) when the
/// effect is *issued*, not when its `Future` later completes. Real CPU time is never used as `δ`.
///
/// - [`Zero`], [`Constant`], [`Uniform`]: `δ` is known at issue time. The simulator enqueues a
///   wakeup at `now + δ` immediately. The stage resumes only once that time has been reached
///   *and* the effect result is available.
/// - [`UntilResolved`]: there is no `δ`. The stage stays suspended until the effect `Future`
///   is resolved. This is how a later world runner delivers a network receive at a time of
///   its choosing.
///
/// Start every effect at [`DurationDist::Zero`]. Pick [`UntilResolved`] for completions the
/// simulation drives; pick a sampled variant for local work (store, validation, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub enum DurationDist {
    /// The effect occupies no simulated time. Resume as soon as the result is available.
    #[default]
    Zero,
    /// The effect always occupies exactly this duration, scheduled when the effect is issued.
    Constant(Duration),
    /// The effect occupies a duration drawn uniformly from `[min, max]` (inclusive).
    ///
    /// Sampled when the effect is issued. `min` must not exceed `max`;
    /// [`sample`](Self::sample) panics otherwise.
    Uniform { min: Duration, max: Duration },
    /// Occupy time until the effect `Future` resolves.
    ///
    /// No wakeup is scheduled. A network receive uses this so the world runner can complete
    /// the future when that transmission should arrive.
    UntilResolved,
}

impl DurationDist {
    /// The default distribution: no simulated time.
    pub const ZERO: Self = Self::Zero;

    /// Draw a finite `δ` if this distribution has one.
    ///
    /// Returns `None` for [`UntilResolved`] — that variant has no duration to sample.
    ///
    /// # Panics
    ///
    /// Panics if this is [`Uniform`](Self::Uniform) and `min > max`, or if a bound
    /// exceeds `u64::MAX` nanoseconds (about 584 years).
    pub fn sample(self, rng: &mut impl Rng) -> Option<Duration> {
        match self {
            Self::Zero => Some(Duration::ZERO),
            Self::Constant(duration) => Some(duration),
            Self::Uniform { min, max } => {
                let min_nanos = nanos_u64(min, "Uniform.min");
                let max_nanos = nanos_u64(max, "Uniform.max");
                assert!(min_nanos <= max_nanos, "DurationDist::Uniform min ({min:?}) exceeds max ({max:?})");
                Some(Duration::from_nanos(rng.random_range(min_nanos..=max_nanos)))
            }
            Self::UntilResolved => None,
        }
    }

    /// Wall-clock bound when forcing `run()` at a sampled deadline: `1.5 × max + 1s`.
    ///
    /// `max` is zero for [`Zero`], the constant for [`Constant`], and the upper end for
    /// [`Uniform`]. [`UntilResolved`] has no force bound (that variant is never forced).
    pub fn force_timeout(self) -> Option<Duration> {
        let max = match self {
            Self::Zero => Duration::ZERO,
            Self::Constant(duration) => duration,
            Self::Uniform { max, .. } => max,
            Self::UntilResolved => return None,
        };
        Some(max.saturating_add(max / 2).saturating_add(Duration::from_secs(1)))
    }
}

fn nanos_u64(duration: Duration, what: &str) -> u64 {
    let nanos = duration.as_nanos();
    assert!(nanos <= u64::MAX as u128, "{what} ({duration:?}) exceeds the 584-year simulation limit");
    nanos as u64
}

#[cfg(test)]
mod tests {
    use rand::{SeedableRng, rngs::StdRng};

    use super::*;

    #[test]
    fn zero_and_constant_are_deterministic() {
        let mut rng = StdRng::seed_from_u64(1);
        assert_eq!(DurationDist::Zero.sample(&mut rng), Some(Duration::ZERO));
        assert_eq!(DurationDist::Constant(Duration::from_millis(7)).sample(&mut rng), Some(Duration::from_millis(7)));
        assert_eq!(DurationDist::UntilResolved.sample(&mut rng), None);
    }

    #[test]
    fn uniform_equal_bounds_is_constant() {
        let mut rng = StdRng::seed_from_u64(1);
        let d = Duration::from_secs(3);
        assert_eq!(DurationDist::Uniform { min: d, max: d }.sample(&mut rng), Some(d));
    }

    #[test]
    fn uniform_stays_inside_inclusive_bounds() {
        let mut rng = StdRng::seed_from_u64(7);
        let min = Duration::from_millis(5);
        let max = Duration::from_millis(15);
        let dist = DurationDist::Uniform { min, max };
        for _ in 0..1000 {
            let sample = dist.sample(&mut rng).expect("Uniform has a finite δ");
            assert!(sample >= min && sample <= max, "{sample:?} not in [{min:?}, {max:?}]");
        }
    }

    #[test]
    fn uniform_is_deterministic_for_a_seed() {
        let dist = DurationDist::Uniform { min: Duration::from_millis(1), max: Duration::from_secs(1) };
        let samples = |seed| {
            let mut rng = StdRng::seed_from_u64(seed);
            (0..20).map(|_| dist.sample(&mut rng)).collect::<Vec<_>>()
        };
        assert_eq!(samples(99), samples(99));
    }

    #[test]
    fn force_timeout_is_one_and_a_half_max_plus_one_second() {
        assert_eq!(DurationDist::Zero.force_timeout(), Some(Duration::from_secs(1)));
        assert_eq!(DurationDist::Constant(Duration::from_secs(10)).force_timeout(), Some(Duration::from_secs(16)));
        assert_eq!(
            DurationDist::Uniform { min: Duration::from_secs(5), max: Duration::from_secs(10) }.force_timeout(),
            Some(Duration::from_secs(16))
        );
        assert_eq!(DurationDist::UntilResolved.force_timeout(), None);
    }

    #[test]
    #[should_panic(expected = "exceeds max")]
    fn uniform_rejects_inverted_bounds() {
        let mut rng = StdRng::seed_from_u64(1);
        DurationDist::Uniform { min: Duration::from_secs(2), max: Duration::from_secs(1) }.sample(&mut rng);
    }
}
