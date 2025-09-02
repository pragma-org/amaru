// Copyright 2025 PRAGMA
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

mod io;
/// Implementation of maelstrom's [echo protocol](https://github.com/jepsen-io/maelstrom/blob/main/doc/workloads.md#workload-echo).
///
/// This is intended as a simple example of how to implement a node in Rust that
/// can be tested with maelstrom, using gasket as the underlying async runtime
/// over tokio.
mod message;
mod run;
mod service;
mod simulate;

pub use message::*;
pub use run::*;
pub use service::*;
pub use simulate::*;
