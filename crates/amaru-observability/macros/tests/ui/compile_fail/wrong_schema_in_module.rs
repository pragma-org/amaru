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

//! This test demonstrates the error message when an invalid schema name is used
//!
//! Expected error: "Invalid trace in module test::sub : NON_EXISTENT. Expected one of: SCHEMA"

use amaru_observability_macros::{define_local_schemas, trace_span};

define_local_schemas! {
    test {
        sub {
            /// Test schema for wrong schema in module test
            SCHEMA {
                required first: String
                required second: u64
            }
        }
    }
}

fn main() {
    let first = String::from("first");
    let _span = trace_span!(test::sub::NON_EXISTENT, first = &first, second = 2_u64);
}
