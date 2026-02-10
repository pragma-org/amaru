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
use crate::tests::configuration::NodeConfig;
use crate::tests::setup::{create_nodes, run_nodes, setup_logging};

/// The purpose of this test is to create two nodes in memory.
/// It uses a connection provider implemented with in-memory data structures to connect the two nodes.
#[test]
fn test_connect_nodes_in_memory() -> anyhow::Result<()> {
    setup_logging(true);
    // Create responder first so it starts listening before the initiator tries to connect
    let mut nodes = create_nodes(vec![NodeConfig::responder(), NodeConfig::initiator()])?;
    run_nodes(&mut nodes);

    let (left, right) = nodes.split_at_mut(1);
    let (responder, initiator) = (&mut left[0], &mut right[0]);

    check_state(initiator.resources(), responder.resources())?;
    Ok(())
}
