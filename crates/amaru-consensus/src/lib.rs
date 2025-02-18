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
// limitations under the License

use amaru_ouroboros::protocol::peer::Peer;
use thiserror::Error;

/// Consensus interface
///
/// The consensus interface is responsible for validating block headers.
pub mod consensus;

/// Chain forward stage
pub mod chain_forward;

#[derive(Error, Debug)]
pub enum ConsensusError {
    #[error("cannot build a chain selector without a tip")]
    MissingTip,
    #[error("Unknown peer {0:?}, bailing out")]
    UnknownPeer(Peer),
}
