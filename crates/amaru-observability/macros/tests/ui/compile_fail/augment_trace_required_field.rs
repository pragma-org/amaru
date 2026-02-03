//! Test that augment_trace rejects required fields
//! 
//! augment_trace can only use optional fields, not required fields.

use amaru_observability_macros::{augment_trace, define_local_schemas};

define_local_schemas! {
    test {
        sub {
            /// Test schema for augment_trace required field test
            SCHEMA {
                required id: u64
                optional count: u64
            }
        }
    }
}

// This should fail - using a required field 'id' in augment_trace
#[augment_trace(test::sub::SCHEMA)]
fn try_required_field(_id: u64) {
    println!("This should not compile");
}

fn main() {}
