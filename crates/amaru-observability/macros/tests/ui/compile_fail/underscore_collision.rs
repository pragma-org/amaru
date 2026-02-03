//! Test: Underscore collision should fail
//! Expected error: Parameter names '_second' and '__second' collide after underscore stripping

use amaru_observability_macros::{define_local_schemas, trace};

define_local_schemas! {
    test {
        sub {
            /// Test schema for underscore collision test
            SCHEMA {
                required first: String
                required second: u64
                required third: u64
            }
        }
    }
}

// Both _second and __second become "second" after stripping underscores
#[trace(test::sub::SCHEMA)]
fn validate_header(_first: String, _second: u64, __second: u64, _third: u64) {
    println!("Underscore collision!");
}

fn main() {}
