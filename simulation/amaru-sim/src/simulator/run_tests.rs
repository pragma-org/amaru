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

use crate::simulator::checks::{TestResult, check_chain_property};
use crate::simulator::report::{
    create_symlink_dir, display_actions_statistics, persist_args,
    persist_generated_actions_as_json, persist_generated_data, persist_traces,
};
use crate::simulator::{Args, RunConfig, generate_entries};
use amaru::tests::configuration::NodeTestConfig;
use amaru::tests::configuration::NodeType::{DownstreamNode, NodeUnderTest, UpstreamNode};
use amaru::tests::setup::{create_nodes, run_nodes};
use amaru_consensus::headers_tree::data_generation::{Action, GeneratedActions, shrink};
use amaru_kernel::{BlockHeader, Peer};
use pure_stage::Instant;
use pure_stage::trace_buffer::TraceBuffer;
use std::fs::create_dir_all;
use std::iter::once;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

pub fn run_tests(args: Args) -> anyhow::Result<()> {
    let run_config = RunConfig::from(args.clone());
    tracing::info!(?run_config, "simulate.start");

    let tests_dir = run_config.persist_directory.as_path();
    if !tests_dir.exists() {
        create_dir_all(tests_dir)?;
    }

    let now = SystemTime::now();
    let test_run_name = format!("{}", now.duration_since(SystemTime::UNIX_EPOCH)?.as_secs());
    let test_run_dir = tests_dir.join(test_run_name);
    create_dir_all(&test_run_dir)?;
    create_symlink_dir(test_run_dir.as_path(), tests_dir.join("latest").as_path());
    persist_args(test_run_dir.as_path(), &args, args.persist_on_success)?;

    for test_number in 1..=run_config.number_of_tests {
        run_test_nb(&run_config, test_run_dir.as_path(), test_number)?;
    }

    Ok(())
}

fn run_test_nb(
    run_config: &RunConfig,
    test_run_dir: &Path,
    test_number: u32,
) -> anyhow::Result<()> {
    let test_run_dir_n = test_run_dir.join(format!("test-{}", test_number));
    create_dir_all(&test_run_dir_n)?;
    create_symlink_dir(
        test_run_dir_n.as_path(),
        test_run_dir_n.parent().unwrap().join("latest").as_path(),
    );

    tracing::info!(
        test_number, total=%run_config.number_of_tests,
        "simulate.generate_test_data"
    );

    let actions = generate_actions(run_config);
    display_actions_statistics(&actions);

    let test = |actions: &GeneratedActions| {
        let mut rng = run_config.rng();
        let mut nodes = create_nodes(&mut rng, node_configs(run_config, actions))
            .expect("failed to create nodes");
        run_nodes(&mut rng, &mut nodes, 10000);
        check_chain_property(nodes, actions)
    };

    let mut test_result = test(&actions);
    shrink_test(run_config, &test, test_number, &mut test_result);

    let persist = run_config.persist_on_success || test_result.is_err();
    persist_generated_data(&test_run_dir_n, &actions, persist)?;
    persist_generated_actions_as_json(&test_run_dir_n, &actions.actions())?;
    if let Some(node_under_test) = test_result
        .nodes()
        .iter()
        .find(|node| node.node_type() == NodeUnderTest)
    {
        persist_traces(
            test_run_dir_n.as_path(),
            node_under_test.config.trace_buffer.clone(),
            persist,
        )?;
    };
    Ok(())
}

pub fn shrink_test(
    run_config: &RunConfig,
    run_test: &dyn Fn(&GeneratedActions) -> TestResult,
    test_number: u32,
    test_result: &mut TestResult,
) {
    if run_config.enable_shrinking {
        let (_, _shrunk_actions, number_of_shrinks) = shrink(
            &run_test,
            &test_result.generated_actions(),
            |test_result: &TestResult| test_result.is_err(),
        );
        test_result.set_test_failure(run_config, test_number, number_of_shrinks)
    } else {
        test_result.set_test_failure(run_config, test_number, 0)
    }
}

pub fn node_configs(run_config: &RunConfig, actions: &GeneratedActions) -> Vec<NodeTestConfig> {
    let upstream_peers = run_config.upstream_peers();

    let upstream_nodes = upstream_peers
        .iter()
        .map(|peer| {
            NodeTestConfig::default()
                .with_no_upstream_peers()
                .with_listen_address(&peer.name)
                .with_chain_length(run_config.generated_chain_depth)
                .with_actions(get_peer_actions(actions, peer))
                .with_validated_blocks(get_headers(actions, peer))
                .with_node_type(UpstreamNode)
        })
        .collect::<Vec<_>>();

    let listen_address = "127.0.0.1:3000";
    let trace_buffer = Arc::new(parking_lot::Mutex::new(TraceBuffer::new(42, 1_000_000_000)));
    let node_under_test = NodeTestConfig::default()
        .with_listen_address(listen_address)
        .with_chain_length(run_config.generated_chain_depth)
        .with_upstream_peers(upstream_peers)
        .with_trace_buffer(trace_buffer)
        .with_validated_blocks(vec![get_anchor(actions)])
        .with_node_type(NodeUnderTest);

    let downstream_nodes = run_config
        .downstream_peers()
        .iter()
        .map(|peer| {
            NodeTestConfig::default()
                .with_listen_address(&peer.name)
                .with_chain_length(run_config.generated_chain_depth)
                .with_upstream_peer(Peer::new(listen_address))
                .with_validated_blocks(vec![get_anchor(actions)])
                .with_node_type(DownstreamNode)
        })
        .collect::<Vec<_>>();

    upstream_nodes
        .into_iter()
        .chain(once(node_under_test))
        .chain(downstream_nodes)
        .collect()
}

fn get_peer_actions(actions: &GeneratedActions, peer: &Peer) -> Vec<Action> {
    actions
        .actions_per_peer()
        .get(peer)
        .into_iter()
        .flatten()
        .cloned()
        .collect::<Vec<_>>()
}

fn get_headers(actions: &GeneratedActions, peer: &Peer) -> Vec<BlockHeader> {
    get_peer_actions(actions, peer)
        .into_iter()
        .filter_map(|action| match action {
            Action::RollForward { header, .. } => Some(header),
            Action::Rollback { .. } => None,
        })
        .collect::<Vec<_>>()
}

fn get_anchor(actions: &GeneratedActions) -> BlockHeader {
    actions.generated_tree().tree().value.clone()
}

fn generate_actions(run_config: &RunConfig) -> GeneratedActions {
    let rng = run_config.rng();
    generate_entries(
        run_config.generated_chain_depth,
        &run_config.upstream_peers(),
        Instant::at_offset(Duration::from_secs(0)),
        200.0,
    )(rng)
    .generation_context()
    .clone()
}
