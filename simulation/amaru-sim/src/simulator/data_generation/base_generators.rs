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

use pure_stage::Instant;
use rand::Rng;
use rand::prelude::StdRng;
use rand_distr::Distribution;
use rand_distr::Exp;
use std::time::Duration;

/// Given a generator for type A, produce a generator for Vec<A> of a given size.
pub fn generate_vec<A>(
    generator: impl Fn(&mut StdRng) -> A,
) -> impl Fn(usize, &mut StdRng) -> Vec<A> {
    move |size, rng| {
        let mut result = Vec::<A>::with_capacity(size);
        for _ in 0..size {
            result.push(generator(rng));
        }
        result
    }
}

/// Generate a u8 in the range [low, high], then pass it to the `then` function to produce a value of type A.
pub fn generate_u8_then<A>(low: u8, high: u8, then: impl Fn(u8) -> A) -> impl Fn(&mut StdRng) -> A {
    move |rng| {
        let x = rng.random_range(low..=high);
        then(x)
    }
}

/// Generate a u8 in the range [low, high].
pub fn generate_u8(low: u8, high: u8) -> impl Fn(&mut StdRng) -> u8 {
    generate_u8_then(low, high, |x| x)
}

/// Given two generators for vectors of values and a function f
/// generate 2 vectors and combine them using f.
/// The two vectors are generated with the same length.
pub fn generate_zip_with<A: Copy, B: Copy, C>(
    size: usize,
    generator1: impl Fn(usize, &mut StdRng) -> Vec<A>,
    generator2: impl Fn(usize, &mut StdRng) -> Vec<B>,
    f: impl Fn(A, B) -> C,
) -> impl Fn(&mut StdRng) -> Vec<C> {
    move |rng| {
        let xs = generator1(size, rng);
        let ys = generator2(size, rng);
        assert_eq!(xs.len(), ys.len());
        xs.into_iter().zip(ys).map(|(x, y)| f(x, y)).collect()
    }
}

/// Generate a sequence of arrival times starting from `start_time`
/// where the delay between arrivals is exponentially distributed with mean `mean_millis`.
pub fn generate_arrival_times<R: Rng>(
    start_time: Instant,
    mean_millis: f64,
) -> impl Fn(usize, &mut R) -> Vec<Instant> {
    move |size, rng| {
        let mut time: Instant = start_time;
        let mut arrival_times = Vec::new();

        let exp = Exp::new(1.0 / mean_millis).unwrap_or_else(|err| panic!("{}", err));
        for _ in 0..size {
            arrival_times.push(time);
            let delay = exp.sample(rng);
            let delay_ms = delay.ceil().min(u64::MAX as f64) as u64;
            time = time + Duration::from_millis(delay_ms);
        }
        arrival_times
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pure_stage::Instant;
    use rand::SeedableRng;
    use rand_distr::Exp;
    use std::collections::BTreeMap;
    use std::time::Duration;

    #[test]
    fn test_generate_u8() {
        let seed = 1234;
        let mut rng = StdRng::seed_from_u64(seed);
        let mut counts = BTreeMap::new();

        for _i in 0..1000 {
            let x = generate_u8(1, 6)(&mut rng);
            *counts.entry(x).or_insert(0) += 1;
        }

        assert_eq!(counts.len(), 6);
        for i in 1..=6 {
            assert!(counts.contains_key(&i), "value {} was never generated", i);
        }
    }

    #[test]
    fn test_exponential() {
        let seed = 1;
        let mut rng = StdRng::seed_from_u64(seed);

        let exp = Exp::new(1.0 / 200.0).unwrap();

        let count = 100_000_000;
        let mut sum: f64 = 0.0;
        for _i in 0..count {
            sum += exp.sample(&mut rng);
        }
        let expected = 200.0;
        let actual = sum / count as f64;
        let difference = (expected - actual).abs();
        assert!(
            difference < 0.01,
            "expected: {}, actual: {}, difference: {}",
            expected,
            actual,
            difference
        )
    }

    #[test]
    fn test_generate_arrival_times() {
        let seed = 1;
        let mut rng = StdRng::seed_from_u64(seed);
        let result =
            generate_arrival_times(Instant::at_offset(Duration::new(0, 0)), 200.0)(5, &mut rng);
        assert_eq!(
            result,
            vec![
                Instant::at_offset(Duration::from_millis(0)),
                Instant::at_offset(Duration::from_millis(409)),
                Instant::at_offset(Duration::from_millis(753)),
                Instant::at_offset(Duration::from_millis(810)),
                Instant::at_offset(Duration::from_millis(837)),
            ]
        )
    }
}
