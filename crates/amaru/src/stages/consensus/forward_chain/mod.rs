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

pub mod chain_follower;
pub mod client_protocol;
pub mod tcp_forward_chain_server;

#[cfg(test)]
mod test_infra;
#[cfg(test)]
mod tests;

pub fn to_pallas_tip(
    tip: amaru_kernel::protocol_messages::tip::Tip,
) -> pallas_network::miniprotocols::chainsync::Tip {
    pallas_network::miniprotocols::chainsync::Tip(
        amaru_network::point::to_network_point(tip.0),
        tip.1.as_u64(),
    )
}
