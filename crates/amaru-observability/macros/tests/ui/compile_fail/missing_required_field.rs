//! Test: Missing a required field
//!
//! Expected error: Missing required field 'third' for schema SCHEMA

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

// This function is missing the required 'third' field
#[trace(test::sub::SCHEMA)]
fn process_block(_first: String, _second: u64) {
    println!("Processing block");
}

fn main() {}
