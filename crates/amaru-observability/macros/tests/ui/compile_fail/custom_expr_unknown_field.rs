//! Test that custom expression fields are validated strictly
//!
//! Unlike function parameters which are allowed to be "extra" (not in schema),
//! custom expression fields (field = expr) must exist in the schema.
//! This catches typos like `roots_constitutions` when the schema has `roots_constitution`.

use amaru_observability_macros::{define_local_schemas, trace};

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

// This should fail: `typo_field` doesn't exist in schema
#[trace(test::example::STRICT_TEST, typo_field = get_value())]
fn test_typo_in_custom_expr(actual_field: String) {
    let _ = actual_field;
}

fn main() {}
