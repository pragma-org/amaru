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
use std::fmt::Debug;

// Andreas Zeller's delta debugging (`ddmin`) algorithm from the paper
// "Simplifying and Isolating Failure-Inducing Input" (2002).
//
// Basically tries to bisect the input (git's bisect algorithm uses the same technique). Will first
// try throwing away half of the input, but if that fails it will throw away smaller and smaller
// parts until it finds the smallest counter example.
pub fn shrink<A: Shrinkable + Debug + Clone, B: Debug>(
    test: &dyn Fn(&A) -> B,
    input: &A,
    error_predicate: impl Fn(&B) -> bool,
) -> (B, A, u32) {
    let mut number_of_shrinks = 0;
    let mut last_error: B;
    let mut input = input.clone();

    let result = test(&input);
    if !error_predicate(&result) {
        return (result, input, number_of_shrinks);
    }
    last_error = result;

    let mut n = 2;
    while input.len() >= 2 {
        let mut current = 0;
        let subset_length = input.len() / n;
        let mut some_reduced_input_is_failing = false;
        while current < input.len() {
            let removed_start = current;
            let removed_end = current + subset_length;
            let reduced_input = input.remove_range(removed_start, removed_end);
            // NOTE: that if we get a different error than the expected one, we treat it as a
            // passing test.
            let result = test(&reduced_input);
            if error_predicate(&result) {
                number_of_shrinks += 1;
                last_error = result;
                input = reduced_input;
                n = (n - 1).max(2);
                some_reduced_input_is_failing = true;
                break;
            }

            current += subset_length;
        }

        if !some_reduced_input_is_failing {
            if n == input.len() {
                break;
            }
            n = (n * 2).min(input.len())
        }
    }
    (last_error, input, number_of_shrinks)
}

/// Trait for data types which can be shrunk to a smaller number of elements via the `shrink` function.
pub trait Shrinkable {
    /// Return a value with a portion of elements removed.
    ///
    /// The removed portion is the half-open range `[removed_start, removed_end)`, so
    /// `removed_start` is the index of the first removed element and `removed_end` is the index
    /// just past the last removed element.
    ///
    /// For example, removing `(1, 3)` from `[10, 20, 30, 40]` produces `[10, 40]`.
    fn remove_range(&self, removed_start: usize, removed_end: usize) -> Self
    where
        Self: Sized;

    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<A: Debug + Clone> Shrinkable for Vec<A> {
    fn remove_range(&self, removed_start: usize, removed_end: usize) -> Self
    where
        Self: Sized,
    {
        let mut remaining_elements: Vec<A> = Vec::new();
        remaining_elements.extend_from_slice(&self[..removed_start]);
        if removed_end < self.len() {
            remaining_elements.extend_from_slice(&self[removed_end..]);
        };
        remaining_elements
    }

    fn len(&self) -> usize {
        self.len()
    }
}

impl<A: Shrinkable, B: Shrinkable> Shrinkable for (A, B) {
    fn remove_range(&self, removed_start: usize, removed_end: usize) -> Self
    where
        Self: Sized,
    {
        let left_len = self.0.len();
        let left_removed_start = removed_start.min(left_len);
        let left_removed_end = removed_end.min(left_len);
        let right_removed_start = removed_start.saturating_sub(left_len);
        let right_removed_end = removed_end.saturating_sub(left_len);

        (
            self.0.remove_range(left_removed_start, left_removed_end),
            self.1.remove_range(right_removed_start, right_removed_end),
        )
    }

    fn len(&self) -> usize {
        self.0.len() + self.1.len()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_tuple_remove_range_splits_removed_range_across_both_halves() {
        let input = (vec![1, 2, 3], vec![4, 5]);

        assert_eq!(input.remove_range(2, 4), (vec![1, 2], vec![5]));
    }

    #[test]
    fn test_tuple_remove_range_handles_range_entirely_in_second_half() {
        let input = (vec![1, 2, 3], vec![4, 5]);

        assert_eq!(input.remove_range(4, 5), (vec![1, 2, 3], vec![4]));
    }

    #[test]
    fn test_shrink_tuple_minimizes_across_both_halves() {
        let failing_input = (vec![1, 2, 3], vec![4, 5]);

        let test = |input: &(Vec<u8>, Vec<u8>)| {
            if input.0.contains(&3) && input.1.contains(&4) { Err("Found tuple pair".to_string()) } else { Ok(()) }
        };

        assert_eq!(
            shrink(&test, &failing_input, |err| { *err == Err("Found tuple pair".to_string()) }),
            (Err("Found tuple pair".to_string()), (vec![3], vec![4]), 2)
        );
    }

    #[test]
    fn test_shrink_failing() {
        let failing_input = vec![1, 2, 3, 42, 5, 6];

        let test = |input: &Vec<u8>| {
            // println!("input: {:?}", input);
            // input: [1, 2, 3, 42, 5, 6]
            // input: [42, 5, 6]
            // input: [5, 6]
            // input: [42, 6]
            // input: [6]
            // input: [42]

            if input.contains(&42) { Err("Found 42".to_string()) } else { Ok(()) }
        };

        assert_eq!(
            shrink(&test, &failing_input, |err| { *err == Err("Found 42".to_string()) }),
            (Err("Found 42".to_string()), vec![42], 3)
        );
    }

    #[test]
    fn test_shrink_unresolved() {
        let failing_input = vec![1, 2, 3, 42, 5, 6];

        let test = |input: &Vec<u8>| {
            // println!("input: {:?}", input);
            // input: [1, 2, 3, 42, 5, 6]
            // input: [42, 5, 6]  <-- NOTE: This will return a different error message than the one
            //                        we expect, which ddmin treats as a passing test.
            // input: [1, 2, 3]
            // input: [2, 3, 42, 5, 6]
            // input: [3, 42, 5, 6]
            // input: [5, 6]
            // input: [3, 42]
            // input: [42]

            if input.len() == 3 && input.contains(&5) {
                assert_eq!(input, &vec![42, 5, 6]);
                return Err("Found 5".to_string());
            };
            if input.contains(&42) { Err("Found 42".to_string()) } else { Ok(()) }
        };

        assert_eq!(
            shrink(&test, &failing_input, |err| { *err == Err("Found 42".to_string()) }),
            (Err("Found 42".to_string()), vec![42], 4)
        )
    }

    #[test]
    fn test_shrink_passing() {
        let successful_input = vec![1, 2, 3];

        let test = |input: &Vec<u8>| {
            if input.contains(&4) { Err("Found 4".to_string()) } else { Ok(()) }
        };
        assert_eq!(
            shrink(&test, &successful_input, |err| { *err == Err("Found 4".to_string()) }),
            (Ok(()), vec![1, 2, 3], 0)
        )
    }
}
