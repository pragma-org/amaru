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

//! This test demonstrates the error message when a field has the wrong type
//!
//! Expected error: "Wrong type for field 'second': expected 'u64', found 'i32'"

use amaru_observability_macros::{define_local_schemas, trace};

define_local_schemas! {
    test {
        sub {
            /// Test schema for wrong field type test
            SCHEMA {
                required first: String
                required second: u64
                required third: u64
            }
        }
    }
}

// Wrong type for 'second' field (i32 instead of u64)
#[trace(test::sub::SCHEMA)]
fn wrong_field_type(_first: String, _second: i32, _third: u64) {
    println!("This has the wrong type for second field");
}

fn main() {}
