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
pub fn shrink<A: Debug + Clone, B: Debug>(
    test: impl Fn(&[A]) -> B,
    mut input: Vec<A>,
    error_predicate: impl Fn(&B) -> bool,
) -> (Vec<A>, B, u32) {
    let mut number_of_shrinks = 0;
    let mut last_error: B;
    let result = test(&input);
    if error_predicate(&result) {
        last_error = result;
    } else {
        panic!(
            "shrink, error predicate doesn't hold for initial input: '{:?}'",
            input
        )
    }
    let mut n = 2;
    while input.len() >= 2 {
        let mut start = 0;
        let subset_length = input.len() / n;
        let mut some_complement_is_failing = false;
        while start < input.len() {
            let mut complement: Vec<A> = Vec::new();
            complement.extend_from_slice(&input[..start]);
            if start + subset_length < input.len() {
                complement.extend_from_slice(&input[start + subset_length..]);
            }
            // NOTE: that if we get a different error than the expected one, we treat it as a
            // passing test.
            let result = test(&complement);
            if error_predicate(&result) {
                number_of_shrinks += 1;
                last_error = result;
                input = complement;
                n = n.max(2) - 1;
                some_complement_is_failing = true;
                break;
            }

            start += subset_length;
        }

        if !some_complement_is_failing {
            if n == input.len() {
                break;
            }
            n = (n * 2).min(input.len())
        }
    }
    (input, last_error, number_of_shrinks)
}

#[cfg(test)]
mod test {

    use super::*;

    #[test]
    fn test_shrink_failing() {
        let failing_input = vec![1, 2, 3, 42, 5, 6];

        let test = |input: &[u8]| {
            // println!("input: {:?}", input);
            // input: [1, 2, 3, 42, 5, 6]
            // input: [42, 5, 6]
            // input: [5, 6]
            // input: [42, 6]
            // input: [6]
            // input: [42]

            if input.contains(&42) {
                Err("Found 42".to_string())
            } else {
                Ok(())
            }
        };

        assert_eq!(
            shrink(test, failing_input, |err| *err
                == Err("Found 42".to_string())),
            (vec![42], Err("Found 42".to_string()), 3)
        );
    }

    #[test]
    fn test_shrink_unresolved() {
        let failing_input = vec![1, 2, 3, 42, 5, 6];

        let test = |input: &[u8]| {
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
                assert_eq!(input, vec![42, 5, 6]);
                return Err("Found 5".to_string());
            };
            if input.contains(&42) {
                Err("Found 42".to_string())
            } else {
                Ok(())
            }
        };

        assert_eq!(
            shrink(test, failing_input, |err| *err
                == Err("Found 42".to_string())),
            (vec![42], Err("Found 42".to_string()), 4)
        )
    }

    #[test]
    #[should_panic(
        expected = "shrink, error predicate doesn't hold for initial input: '[1, 2, 3]'"
    )]
    fn test_shrink_passing() {
        let failing_input = vec![1, 2, 3];

        let test = |input: &[u8]| {
            if input.contains(&4) {
                Err("Found 4".to_string())
            } else {
                Ok(())
            }
        };
        assert_eq!(
            shrink(test, failing_input, |err| *err
                == Err("Found 4".to_string())),
            (vec![4], Err("Found 4".to_string()), 0)
        )
    }
}
