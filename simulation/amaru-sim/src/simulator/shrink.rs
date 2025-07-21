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

// Andreas Zeller's delta debugging (`ddmin`) algorithm from the paper
// "Simplifying and Isolating Failure-Inducing Input" (2002).
//
// Basically tries to bisect the input (git's bisect algorithm uses the same technique). Will first
// try throwing away half of the input, but if that fails it will throw away smaller and smaller
// parts until it finds the smallest counter example.
pub fn shrink<T: Clone>(
    test: impl Fn(&[T]) -> Result<(), String>,
    mut input: Vec<T>,
    expected_error: &str,
) -> Vec<T> {
    assert_eq!(
        test(&input),
        Err(expected_error.to_string()),
        "shrink, initial input doesn't fail with '{}'",
        expected_error
    );

    let mut n = 2;
    while input.len() >= 2 {
        let mut start = 0;
        let subset_length = input.len() / n;
        let mut some_complement_is_failing = false;

        while start < input.len() {
            let mut complement: Vec<T> = Vec::new();
            complement.extend_from_slice(&input[..start]);
            complement.extend_from_slice(&input[start + subset_length..]);

            // NOTE: that if we get a different error than the expected one, we treat it as a
            // passing test.
            if test(&complement) == Err(expected_error.to_string()) {
                input = complement;
                n = n.saturating_sub(1).max(2);
                some_complement_is_failing = true;
                break;
            }

            start += subset_length;
        }

        if !some_complement_is_failing {
            if n == input.len() {
                break;
            }
            n *= 2.min(input.len());
        }
    }

    input
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

        assert_eq!(shrink(test, failing_input, "Found 42"), vec![42]);
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

        assert_eq!(shrink(test, failing_input, "Found 42"), vec![42])
    }

    #[test]
    #[should_panic(expected = "shrink, initial input doesn't fail with 'Found 4'")]
    fn test_shrink_passing() {
        let failing_input = vec![1, 2, 3];

        let test = |input: &[u8]| {
            if input.contains(&4) {
                Err("Found 4".to_string())
            } else {
                Ok(())
            }
        };
        assert_eq!(shrink(test, failing_input, "Found 4"), vec![4])
    }
}
