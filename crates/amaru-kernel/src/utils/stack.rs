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

use std::{error::Error, thread};

pub const STACK_SIZE_2MIB: usize = 2 * 1024 * 1024;

/// Run a given action within in a thread with a given stack size, overriding any
/// higher-level setting. This allows, in particular, running tests in a controlled setting.
pub fn with_stack_size<A: Send + 'static>(
    stack_size: usize,
    run: impl FnOnce() -> A + Send + 'static,
) -> Result<A, Box<dyn Error>> {
    match thread::Builder::new().stack_size(stack_size).spawn(run)?.join() {
        Ok(result) => Ok(result),
        Err(_) => Err("the test thread panicked, see the failure reported above".into()),
    }
}
