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

//! Call-site `%` / `?` formatters are rejected; they belong on the schema type.

use amaru_observability_macros::{define_local_schemas, trace_span};

define_local_schemas! {
    test {
        example {
            /// Test schema
            HASH {
                required hash: String
            }
        }
    }
}

fn main() {
    let hash = "abc".to_string();
    let _span = trace_span!(crate::test::example::HASH, hash = %hash);
}
