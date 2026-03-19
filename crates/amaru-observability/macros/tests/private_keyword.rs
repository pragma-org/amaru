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

//! Tests for schema visibility in trace macros and schema definitions.

use std::sync::atomic::{AtomicUsize, Ordering};

use amaru_observability_macros::{define_local_schemas, trace_record, trace_span};

static PRIVATE_FIELD_EVALUATIONS: AtomicUsize = AtomicUsize::new(0);

define_local_schemas! {
    security {
        secrets {
            /// Public secret tracking
            public PUBLIC_SECRET {
                required key_name: String
            }

            /// Private secret tracking (private by default)
            PRIVATE_SECRET {
                required key_id: String
            }
        }
    }
}

fn trace_private_schema(key_id: String) {
    let _span = trace_span!(security::secrets::PRIVATE_SECRET, key_id = &key_id);
    let _guard = _span.enter();
}

fn trace_public_schema(key_name: String) {
    let _span = trace_span!(security::secrets::PUBLIC_SECRET, key_name = &key_name);
    let _guard = _span.enter();
}

fn trace_record_private_schema(key_id: String) {
    trace_record!(security::secrets::PRIVATE_SECRET, key_id = key_id);
}

fn trace_span_private_schema(key_id: String) {
    let _span = trace_span!(security::secrets::PRIVATE_SECRET, key_id = &key_id);
    let _guard = _span.enter();
}

fn count_private_field_evaluation() -> String {
    PRIVATE_FIELD_EVALUATIONS.fetch_add(1, Ordering::SeqCst);
    "side_effect".to_string()
}

#[test]
fn test_schema_visibility_in_schemas() {
    trace_private_schema("secret_123".into());
    trace_public_schema("public_key".into());
    trace_record_private_schema("secret_456".into());
    trace_span_private_schema("secret_789".into());
}

#[test]
fn test_disabled_private_span_skips_field_evaluation() {
    PRIVATE_FIELD_EVALUATIONS.store(0, Ordering::SeqCst);

    let _span = trace_span!(security::secrets::PRIVATE_SECRET, key_id = count_private_field_evaluation());

    assert_eq!(PRIVATE_FIELD_EVALUATIONS.load(Ordering::SeqCst), 0);
}
