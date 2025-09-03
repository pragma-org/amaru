// Copyright 2024 PRAGMA
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

use std::env;

#[expect(clippy::expect_used)]
fn main() {
    built::write_built_file().expect("Failed to acquire build-time information");

    // Set the NETWORK environment variable based on the value from the environment or default to "preprod"
    // This is necessary for the tests to run correctly, as they rely on the NETWORK variable to be set at build time
    let network = env::var("NETWORK").unwrap_or_else(|_| "preprod".into());
    println!("cargo:rerun-if-env-changed=NETWORK");
    println!("cargo:rustc-env=NETWORK={}", network);
}
