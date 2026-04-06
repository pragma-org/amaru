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

//! Test that custom expression fields are validated strictly
//!
//! Unlike function parameters which are allowed to be "extra" (not in schema),
//! custom expression fields (field = expr) must exist in the schema.
//! This catches typos like `roots_constitutions` when the schema has `roots_constitution`.

use amaru_observability_macros::{define_local_schemas, trace_span};

define_local_schemas! {
    test {
        example {
            /// Test schema for custom expression validation
            STRICT_TEST {
                required actual_field: String
                optional optional_field: u64
            }
        }
    }
}

/// Helper function
fn get_value() -> String {
    "test".to_string()
}

fn main() {
    let actual_field = String::from("ok");
    let _span = trace_span!(test::example::STRICT_TEST, actual_field = &actual_field, typo_field = get_value());
}
