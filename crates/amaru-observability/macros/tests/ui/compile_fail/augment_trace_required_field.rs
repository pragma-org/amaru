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
