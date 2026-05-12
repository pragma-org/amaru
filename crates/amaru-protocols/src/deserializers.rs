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

use pure_stage::DeserializerGuards;

use crate::{
    accept, blockfetch, chainsync, connection, handshake, keepalive, manager, mux, network_effects, store_effects,
    tx_submission,
};

pub fn register_deserializers() -> DeserializerGuards {
    let mut guards: DeserializerGuards = Vec::new();
    guards.extend(accept::register_deserializers());
    guards.extend(blockfetch::register_deserializers());
    guards.extend(chainsync::register_deserializers());
    guards.extend(connection::register_deserializers());
    guards.extend(handshake::register_deserializers());
    guards.extend(keepalive::register_deserializers());
    guards.extend(manager::register_deserializers());
    guards.extend(mux::register_deserializers());
    guards.extend(network_effects::register_deserializers());
    guards.extend(store_effects::register_deserializers());
    guards.extend(tx_submission::register_deserializers());
    guards
}
