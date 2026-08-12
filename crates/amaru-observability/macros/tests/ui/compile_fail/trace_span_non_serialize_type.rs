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

//! Test that complex (non-primitive) trace_span field values must implement Serialize.

use amaru_observability_macros::{define_local_schemas, trace_span};

struct NoSerialize;

define_local_schemas! {
    test {
        example {
            /// Test schema for values that are typed correctly but not Serialize
            NON_SERIALIZE {
                required value: NoSerialize
            }
        }
    }
}

fn main() {
    let _span = trace_span!(crate::test::example::NON_SERIALIZE, value = NoSerialize);
}
