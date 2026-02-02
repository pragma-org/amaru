//! This test demonstrates the error message when a field has the wrong type
//!
//! Expected error: "Wrong type for field 'second': expected 'u64', found 'i32'"

use amaru_observability_macros::{define_local_schemas, trace};

define_local_schemas! {
    test {
        sub {
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
