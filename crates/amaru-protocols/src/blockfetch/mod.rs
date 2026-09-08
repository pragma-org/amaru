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

mod initiator;
pub(crate) mod messages;
mod responder;

use amaru_pure_stage::DeserializerGuards;
pub use initiator::{BLOCKFETCH_PIPELINE_N, BlockFetchMessage, Blocks, register_blockfetch_initiator};
pub use messages::{BatchDone, Block, ClientDone, Message, NoBlocks, RequestRange, StartBatch};
pub use responder::register_blockfetch_responder;

pub fn register_deserializers() -> DeserializerGuards {
    vec![initiator::register_deserializers(), responder::register_deserializers()].into_iter().flatten().collect()
}
