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

//! Tests for the `private` keyword in trace macros and schema definitions.

use amaru_observability_macros::{define_local_schemas, trace, trace_record, trace_span};

define_local_schemas! {
    security {
        secrets {
            /// Public secret tracking
            PUBLIC_SECRET {
                required key_name: String
            }

            /// Private secret tracking (marked as private)
            private PRIVATE_SECRET {
                required key_id: String
            }
        }
    }
}

#[trace(private, security::secrets::PRIVATE_SECRET)]
fn trace_with_private(_key_id: String) {
    // Function with private trace
}

#[trace(security::secrets::PUBLIC_SECRET)]
fn trace_without_private(_key_name: String) {
    // Function with public trace
}

fn trace_record_with_private(_key_id: String) {
    trace_record!(private, security::secrets::PRIVATE_SECRET, key_id = _key_id);
}

fn trace_span_with_private() {
    let _span = trace_span!(private, security::secrets::PRIVATE_SECRET);
}

#[test]
fn test_private_keyword_in_schemas() {
    // Test that private schemas are defined correctly
    trace_with_private("secret_123".into());
    trace_without_private("public_key".into());
    trace_record_with_private("secret_456".into());
    trace_span_with_private();
}
