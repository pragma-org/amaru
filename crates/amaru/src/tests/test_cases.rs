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

use crate::tests::assertions::check_state;
use crate::tests::configuration::NodeTestConfig;
use crate::tests::setup::{create_nodes, run_nodes, setup_logging};
use amaru_kernel::utils::tests::run_strategy;
use amaru_kernel::{Hash, Point, Slot, any_headers_chain_with_root};
use pure_stage::simulation::RandStdRng;

/// The purpose of this test is to create two nodes in memory.
/// It uses a connection provider implemented with in-memory data structures to connect the two nodes.
#[test]
fn test_connect_nodes_in_memory() -> anyhow::Result<()> {
    setup_logging(true);

    // Use a slot in the Conway era (era tag 7) for Preprod which starts at slot 68774400
    // This ensures blocks created with era tag 7 are valid according to the era history
    let conway_start_slot = Slot::from(68774400);
    let root_point = Point::Specific(conway_start_slot, Hash::new([0u8; 32]));
    let headers = run_strategy(any_headers_chain_with_root(5, root_point));
    let mut rng = RandStdRng::from_seed(42);
    let mut nodes = create_nodes(
        &mut rng,
        vec![
            NodeTestConfig::initiator()
                .with_chain_length(5)
                .with_validated_blocks(vec![headers[0].clone()]),
            NodeTestConfig::responder()
                .with_chain_length(5)
                .with_validated_blocks(headers),
        ],
    )?;
    run_nodes(&mut rng, &mut nodes, 10000);

    let (initiator, responder) = nodes.split_at_mut(1);
    let (initiator, responder) = (&mut initiator[0], &mut responder[0]);

    check_state(initiator.running.resources(), responder.running.resources())?;
    Ok(())
}
