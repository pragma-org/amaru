//! This test demonstrates the error message when an invalid schema name is used
//!
//! Expected error: "Invalid trace in module test::sub : NON_EXISTENT. Expected one of: SCHEMA"

use amaru_observability_macros::{define_local_schemas, trace};

define_local_schemas! {
    test {
        sub {
            SCHEMA {
                required first: String
                required second: u64
            }
        }
    }
}

#[trace(test::sub::NON_EXISTENT)]
fn wrong_schema_name(_first: String, _second: u64) {
    println!("This schema name doesn't exist");
}

fn main() {}
