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

#![feature(type_alias_impl_trait)]

pub mod blockfetch;
pub mod chainsync;
pub mod connection;
pub mod handshake;
pub mod keepalive;
pub mod manager;
pub mod mempool_effects;
pub mod mux;
pub mod network_effects;
pub mod protocol;
pub mod protocol_messages;
pub mod store_effects;
pub mod tx_submission;

#[cfg(test)]
mod tests;
