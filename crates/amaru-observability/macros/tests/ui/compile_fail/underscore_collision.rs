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

//! Test: Underscore collision should fail
//! Expected error: Parameter names '_second' and '__second' collide after underscore stripping

use amaru_observability_macros::{define_local_schemas, trace};

define_local_schemas! {
    test {
        sub {
            /// Test schema for underscore collision test
            SCHEMA {
                required first: String
                required second: u64
                required third: u64
            }
        }
    }
}

// Both _second and __second become "second" after stripping underscores
#[trace(test::sub::SCHEMA)]
fn validate_header(_first: String, _second: u64, __second: u64, _third: u64) {
    println!("Underscore collision!");
}

fn main() {}
