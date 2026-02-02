//! Test: Duplicate field names should fail
//! Expected error: Duplicate field 'name' in schema VALIDATE_HEADER

use amaru_observability_macros::define_local_schemas;

define_local_schemas! {
    consensus {
        chain_sync {
            VALIDATE_HEADER {
                required name: String
                required name: u64
            }
        }
    }
}

fn main() {}
