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
/// - [`DurationDist::Zero`], [`DurationDist::Constant`], [`DurationDist::Uniform`],
///   [`DurationDist::Cdf`]: `δ` is known at issue time. The simulator enqueues a wakeup at
///   `now + δ` immediately. For [`Effects::external`](crate::Effects::external) the stage
///   resumes only once that time has been reached *and* the effect result is available. For
///   [`Effects::detach`](crate::Effects::detach) the airlock is acked immediately; `δ` delays
///   mailbox delivery of the mapped result.
/// - [`DurationDist::UntilResolved`]: there is no `δ`. A blocking external keeps the stage
///   suspended until the effect `Future` is resolved. A detached external leaves the stage
///   free; the result is enqueued when the future completes. This is how a later world runner
///   delivers a network receive at a time of its choosing.
///
/// Start every effect at [`DurationDist::Zero`]. Pick [`DurationDist::UntilResolved`] for
/// completions the simulation drives. Assign [`DurationDist::cdf`] for measured local work.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
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
    /// Inverse CDF of `(probability, nanoseconds)` knots.
    ///
    /// Build one with [`Self::cdf`]. The array length is that function's const parameter.
    /// Probabilities are `f32` values from `0.0` to `1.0`. Latencies are nanoseconds.
    /// Knots must start at `0.0`, end at `1.0`, strictly increase in probability, and not
    /// decrease in latency. The slice is `'static` because this enum is [`Copy`].
    /// Deserializing a CDF checks the knots, then leaks one copy. An invalid table is
    /// rejected and not leaked.
    Cdf(&'static [(f32, u64)]),
    /// Occupy time until the effect `Future` resolves.
    ///
    /// No wakeup is scheduled. A network receive uses this so the world runner can complete
    /// the future when that transmission should arrive.
    UntilResolved,
}

impl DurationDist {
    /// The default distribution: no simulated time.
    pub const ZERO: Self = Self::Zero;

    /// Inverse CDF whose length `N` is inferred from `knots`.
    ///
    /// Each knot is `(probability, nanoseconds)`. `probability` is an `f32` in `0.0..=1.0`.
    ///
    /// # Panics
    ///
    /// Panics if `N < 2`, if the first probability is not `0.0`, if the last is not `1.0`,
    /// if probabilities do not strictly increase, or if latencies decrease. In a const item
    /// that panic is a compile error.
    pub const fn cdf<const N: usize>(knots: &'static [(f32, u64); N]) -> Self {
        assert!(N >= 2, "DurationDist::Cdf needs a start knot and an end knot");
        validate_knots(knots);
        Self::Cdf(knots)
    }

    /// “use” a generated CDF const item when the effect is UntilResolved.
    pub const fn ignore(self, _other: Self) -> Self {
        self
    }

    /// Draw a finite `δ` if this distribution has one.
    ///
    /// Returns `None` for [`DurationDist::UntilResolved`] — that variant has no duration to sample.
    ///
    /// # Panics
    ///
    /// Panics if this is [`Uniform`](Self::Uniform) and `min > max`, if this is
    /// [`Cdf`](Self::Cdf) and the knots are empty or not a monotone inverse CDF, or if a
    /// bound exceeds `u64::MAX` nanoseconds (about 584 years).
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
            Self::Cdf(knots) => {
                let unit = rng.random_range(0.0f32..=1.0);
                Some(Duration::from_nanos(cdf_nanos(knots, unit)))
            }
            Self::UntilResolved => None,
        }
    }

    /// Wall-clock bound when forcing `run()` at a sampled deadline: `1.5 × max + 1s`.
    ///
    /// `max` is zero for [`DurationDist::Zero`], the constant for [`DurationDist::Constant`],
    /// the upper end for [`DurationDist::Uniform`], and the longest knot for
    /// [`DurationDist::Cdf`]. [`DurationDist::UntilResolved`] has no force bound (that
    /// variant is never forced).
    pub fn force_timeout(self) -> Option<Duration> {
        let max = match self {
            Self::Zero => Duration::ZERO,
            Self::Constant(duration) => duration,
            Self::Uniform { max, .. } => max,
            Self::Cdf(knots) => Duration::from_nanos(knots.iter().fold(0, |longest, knot| longest.max(knot.1))),
            Self::UntilResolved => return None,
        };
        Some(max.saturating_add(max / 2).saturating_add(Duration::from_secs(1)))
    }
}

const fn validate_knots(knots: &[(f32, u64)]) {
    assert!(!knots.is_empty(), "DurationDist::Cdf has no knots");
    let last = knots.len() - 1;
    assert!(knots[0].0 == 0.0, "DurationDist::Cdf must start at probability 0");
    assert!(knots[last].0 == 1.0, "DurationDist::Cdf must end at probability 1");
    let mut i = 1;
    while i < knots.len() {
        assert!(knots[i].0 > knots[i - 1].0, "DurationDist::Cdf probabilities must strictly increase");
        assert!(knots[i].1 >= knots[i - 1].1, "DurationDist::Cdf durations must not decrease");
        i += 1;
    }
}

fn cdf_nanos(knots: &[(f32, u64)], sample: f32) -> u64 {
    assert!(!knots.is_empty(), "DurationDist::Cdf has no knots");
    let last = knots.len() - 1;
    // A draw of 1.0 lands on the final knot. `partition_point` would then return `knots.len()`.
    if sample >= knots[last].0 {
        return knots[last].1;
    }
    let right_idx = knots.partition_point(|knot| knot.0 <= sample);
    let left = knots[right_idx - 1];
    let right = knots[right_idx];
    let t = ((sample - left.0) / (right.0 - left.0)).min(1.0);
    (left.1 as f32 + (right.1 - left.1) as f32 * t).round() as u64
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

    const LINE_10_40: DurationDist = DurationDist::cdf(&[(0.0, 10), (1.0, 40)]);

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

    fn line(start: u64, end: u64) -> DurationDist {
        DurationDist::cdf(Box::leak(Box::new([(0.0, start), (1.0, end)])))
    }

    #[test]
    fn cdf_interpolates_between_knots() {
        let knots = [(0.0, 0), (0.5, 0), (1.0, 1_000)];
        assert_eq!(cdf_nanos(&knots, 0.0), 0);
        assert_eq!(cdf_nanos(&knots, 0.5), 0);
        assert_eq!(cdf_nanos(&knots, 0.75), 500);
        assert_eq!(cdf_nanos(&knots, 1.0), 1_000);
    }

    #[test]
    fn cdf_sample_stays_inside_the_recorded_range() {
        let dist = line(10, 40);
        let mut rng = StdRng::seed_from_u64(3);
        for _ in 0..200 {
            let sample = dist.sample(&mut rng).expect("Cdf has a finite δ");
            assert!(sample >= Duration::from_nanos(10) && sample <= Duration::from_nanos(40));
        }
    }

    #[test]
    fn cdf_sample_is_deterministic_for_a_seed() {
        let dist = line(1, 1_000);
        let samples = |seed| {
            let mut rng = StdRng::seed_from_u64(seed);
            (0..20).map(|_| dist.sample(&mut rng)).collect::<Vec<_>>()
        };
        assert_eq!(samples(4), samples(4));
    }

    #[test]
    fn cdf_const_item_uses_the_inferred_length() {
        assert_eq!(LINE_10_40.force_timeout(), Some(Duration::from_secs(1) + Duration::from_nanos(60)));
        assert_eq!(line(0, 10_000_000_000).force_timeout(), Some(Duration::from_secs(16)));
    }

    #[test]
    #[should_panic(expected = "must end at probability")]
    fn cdf_rejects_a_table_that_does_not_reach_one() {
        let _ = DurationDist::cdf(Box::leak(Box::new([(0.0, 0), (0.5, 10)])));
    }
}
