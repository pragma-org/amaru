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

//! Test: Missing a required field
//!
//! Expected error: Missing required field 'third' for schema SCHEMA

use amaru_observability_macros::{define_local_schemas, trace};

define_local_schemas! {
    test {
        sub {
            /// Test schema for missing required field test
            SCHEMA {
                required first: String
                required second: u64
                required third: u64
            }
        }
    }
}

// This function is missing the required 'third' field
#[trace(test::sub::SCHEMA)]
fn process_block(_first: String, _second: u64) {
    println!("Processing block");
}

fn main() {}
