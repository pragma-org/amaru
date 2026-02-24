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

//! Test: Duplicate field names should fail
//! Expected error: Duplicate field 'name' in schema VALIDATE_HEADER

use amaru_observability_macros::define_local_schemas;

define_local_schemas! {
    consensus {
        chain_sync {
            /// Test schema with duplicate fields
            VALIDATE_HEADER {
                required name: String
                required name: u64
            }
        }
    }
}

fn main() {}
