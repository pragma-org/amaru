#![recursion_limit = "4096"]

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

use amaru::tests::configuration::NodeTestConfig;
use amaru::tests::setup::{create_nodes, run_nodes};
use amaru_consensus::headers_tree::data_generation::Action;
use amaru_kernel::utils::tests::run_strategy;
use amaru_kernel::{Peer, any_headers_chain};
use amaru_tracing_json::{TraceCollectConfig, assert_spans_trees};
use pure_stage::simulation::RandStdRng;
use serde_json::json;
use tracing::info_span;

/// For this test we initialize an initiator and a responder node, both set on the first header of a chain of 3 headers.
/// We then execute a RollForward action on the responder.
/// We assert that the resulting trace contains:
///  - Spans for the initialization of the connection, the handshake, the keepalive, the chainsync, and the txsubmission protocols.
///  - Spans for the roll forward action.
#[test]
fn run_simulator_with_traces() {
    let execute = || {
        let headers = run_strategy(any_headers_chain(3));
        let actions = vec![Action::RollForward {
            peer: Peer::new("1"),
            header: headers[1].clone(),
        }];
        let responder_config = NodeTestConfig::responder()
            .with_chain_length(3)
            .with_validated_blocks(headers.clone())
            .with_actions(actions);
        let initiator_config = NodeTestConfig::initiator()
            .with_chain_length(3)
            .with_validated_blocks(vec![headers[0].clone()]);

        let mut rng = RandStdRng::from_seed(42);
        info_span!(target: "amaru_consensus", "handle_msg").in_scope(|| {
            let mut nodes =
                create_nodes(&mut rng, vec![initiator_config, responder_config]).unwrap();
            run_nodes(&mut rng, &mut nodes, 10000);
        });
    };

    let config = TraceCollectConfig::default()
        .with_include_targets(&["amaru_consensus", "amaru_protocols"])
        .with_exclude_targets(&["amaru_protocols::mux"]);

    assert_spans_trees(
        execute,
        vec![json!(
          {
            "name": "handle_msg",
            "target": "amaru_consensus",
            "children": [
              { "name": "manager", "target": "amaru_protocols::manager", "message_type": "AddPeer" },
              { "name": "manager", "target": "amaru_protocols::manager", "message_type": "Listen" },
              { "name": "manager", "target": "amaru_protocols::manager", "message_type": "Listen" },
              { "name": "manager", "target": "amaru_protocols::manager", "message_type": "Connect" },
              { "name": "connection", "target": "amaru_protocols::connection", "message_type": "Initialize", "conn_id": "0", "peer": "127.0.0.1:3001", "role": "Initiator" },
              { "name": "manager", "target": "amaru_protocols::manager", "message_type": "Accepted" },
              { "name": "connection", "target": "amaru_protocols::connection", "message_type": "Initialize", "conn_id": "1", "peer": "127.0.0.1:5000", "role": "Responder" },
              { "name": "handshake.initiator.stage", "target": "amaru_protocols::handshake::initiator", "message_type": "Propose" },
              { "name": "handshake.responder.protocol", "target": "amaru_protocols::handshake::responder", "message_type": "Propose" },
              { "name": "handshake.responder.stage", "target": "amaru_protocols::handshake::responder", "version_table": "{11: { network_magic: testnet, initiator_only_diffusion_mode: true, peer_sharing: 0, query: false }, 12: { network_magic: testnet, initiator_only_diffusion_mode: true, peer_sharing: 0, query: false }, 13: { network_magic: testnet, initiator_only_diffusion_mode: true, peer_sharing: 0, query: false }, 14: { network_magic: testnet, initiator_only_diffusion_mode: true, peer_sharing: 0, query: false }}" },
              { "name": "connection", "target": "amaru_protocols::connection", "message_type": "Handshake", "conn_id": "1", "peer": "127.0.0.1:5000", "role": "Responder" },
              { "name": "handshake.initiator.protocol", "target": "amaru_protocols::handshake::initiator", "message_type": "Accept" },
              { "name": "handshake.initiator.stage", "target": "amaru_protocols::handshake::initiator", "message_type": "Conclusion" },
              { "name": "connection", "target": "amaru_protocols::connection", "message_type": "Handshake", "conn_id": "0", "peer": "127.0.0.1:3001", "role": "Initiator" },
              { "name": "keepalive.initiator.stage", "target": "amaru_protocols::keepalive::initiator", "cookie": 0 },
              { "name": "tx_submission.responder.protocol", "target": "amaru_protocols::tx_submission::responder", "message_type": "Init" },
              { "name": "tx_submission.responder.stage", "target": "amaru_protocols::tx_submission::responder", "message_type": "Init" },
              { "name": "chainsync.initiator.stage", "target": "amaru_protocols::chainsync::initiator", "message_type": "Initialize" },
              { "name": "diffusion.chain_sync", "target": "amaru_consensus::stages::pull", "message_type": "Initialize" },
              { "name": "chainsync.responder.protocol", "target": "amaru_protocols::chainsync::responder", "message_type": "FindIntersect" },
              { "name": "chainsync.responder.stage", "target": "amaru_protocols::chainsync::responder", "message_type": "FindIntersect" },
              { "name": "tx_submission.initiator.protocol", "target": "amaru_protocols::tx_submission::initiator", "message_type": "RequestTxIdsBlocking" },
              { "name": "txsubmission.initiator.stage", "target": "amaru_protocols::tx_submission::initiator", "message_type": "RequestTxIds" },
              { "name": "blockfetch.initiator.protocol", "target": "amaru_protocols::blockfetch::initiator", "message_type": "Initialize" },
              { "name": "tx_submission.responder.protocol", "target": "amaru_protocols::tx_submission::responder", "message_type": "ReplyTxIds" },
              { "name": "tx_submission.responder.stage", "target": "amaru_protocols::tx_submission::responder", "message_type": "ReplyTxIds" },
              { "name": "chainsync.initiator.protocol", "target": "amaru_protocols::chainsync::initiator", "message_type": "IntersectFound" },
              { "name": "chainsync.initiator.stage", "target": "amaru_protocols::chainsync::initiator", "message_type": "IntersectFound" },
              { "name": "diffusion.chain_sync", "target": "amaru_consensus::stages::pull", "message_type": "IntersectFound" },
              { "name": "tx_submission.initiator.protocol", "target": "amaru_protocols::tx_submission::initiator", "message_type": "RequestTxIdsBlocking" },
              { "name": "txsubmission.initiator.stage", "target": "amaru_protocols::tx_submission::initiator", "message_type": "RequestTxIds" },
              { "name": "chainsync.responder.protocol", "target": "amaru_protocols::chainsync::responder", "message_type": "RequestNext" },
              { "name": "chainsync.responder.stage", "target": "amaru_protocols::chainsync::responder", "message_type": "RequestNext" },
              { "name": "chainsync.responder.protocol", "target": "amaru_protocols::chainsync::responder", "message_type": "RequestNext" },
              { "name": "chainsync.responder.stage", "target": "amaru_protocols::chainsync::responder", "message_type": "RequestNext" },
              { "name": "chainsync.initiator.protocol", "target": "amaru_protocols::chainsync::initiator", "message_type": "RollBackward" },
              { "name": "chainsync.initiator.stage", "target": "amaru_protocols::chainsync::initiator", "message_type": "RollBackward" },
              { "name": "tx_submission.responder.protocol", "target": "amaru_protocols::tx_submission::responder", "message_type": "ReplyTxIds" },
              { "name": "tx_submission.responder.stage", "target": "amaru_protocols::tx_submission::responder", "message_type": "ReplyTxIds" },
              {
                "name": "diffusion.chain_sync",
                "target": "amaru_consensus::stages::pull",
                "message_type": "RollBackward",
                "children": [
                  { "name": "chain_sync.receive_header", "target": "amaru_consensus::stages::receive_header" },
                  { "name": "chain_sync.validate_header", "target": "amaru_consensus::stages::validate_header" },
                  { "name": "diffusion.fetch_block", "target": "amaru_consensus::stages::fetch_block" },
                  { "name": "chain_sync.validate_block", "target": "amaru_consensus::stages::validate_block" },
                  { "name": "chain_sync.select_chain", "target": "amaru_consensus::stages::select_chain" }
                ]
              },
              { "name": "chainsync.initiator.protocol", "target": "amaru_protocols::chainsync::initiator", "message_type": "AwaitReply" },
              { "name": "tx_submission.initiator.protocol", "target": "amaru_protocols::tx_submission::initiator", "message_type": "RequestTxIdsBlocking" },
              { "name": "txsubmission.initiator.stage", "target": "amaru_protocols::tx_submission::initiator", "message_type": "RequestTxIds" },
              { "name": "tx_submission.responder.protocol", "target": "amaru_protocols::tx_submission::responder", "message_type": "ReplyTxIds" },
              { "name": "tx_submission.responder.stage", "target": "amaru_protocols::tx_submission::responder", "message_type": "ReplyTxIds" },
              { "name": "tx_submission.initiator.protocol", "target": "amaru_protocols::tx_submission::initiator", "message_type": "RequestTxIdsBlocking" },
              { "name": "txsubmission.initiator.stage", "target": "amaru_protocols::tx_submission::initiator", "message_type": "RequestTxIds" },
              { "name": "tx_submission.responder.protocol", "target": "amaru_protocols::tx_submission::responder", "message_type": "ReplyTxIds" },
              { "name": "tx_submission.responder.stage", "target": "amaru_protocols::tx_submission::responder", "message_type": "ReplyTxIds" },
              { "name": "tx_submission.initiator.protocol", "target": "amaru_protocols::tx_submission::initiator", "message_type": "RequestTxIdsBlocking" },
              { "name": "txsubmission.initiator.stage", "target": "amaru_protocols::tx_submission::initiator", "message_type": "RequestTxIds" },
              { "name": "tx_submission.responder.protocol", "target": "amaru_protocols::tx_submission::responder", "message_type": "ReplyTxIds" },
              { "name": "tx_submission.responder.stage", "target": "amaru_protocols::tx_submission::responder", "message_type": "ReplyTxIds" },
              { "name": "tx_submission.initiator.protocol", "target": "amaru_protocols::tx_submission::initiator", "message_type": "RequestTxIdsBlocking" },
              { "name": "txsubmission.initiator.stage", "target": "amaru_protocols::tx_submission::initiator", "message_type": "RequestTxIds" },
              { "name": "manager", "target": "amaru_protocols::manager", "message_type": "NewTip" },
              { "name": "connection", "target": "amaru_protocols::connection", "message_type": "NewTip", "conn_id": "1", "peer": "127.0.0.1:5000", "role": "Responder" },
              { "name": "chainsync.responder.protocol", "target": "amaru_protocols::chainsync::responder", "message_type": "RequestNext" },
              { "name": "chainsync.responder.stage", "target": "amaru_protocols::chainsync::responder", "message_type": "RequestNext" },
              { "name": "chainsync.initiator.protocol", "target": "amaru_protocols::chainsync::initiator", "message_type": "RollForward" },
              { "name": "chainsync.initiator.stage", "target": "amaru_protocols::chainsync::initiator", "message_type": "RollForward" },
              {
                "name": "diffusion.chain_sync",
                "target": "amaru_consensus::stages::pull",
                "message_type": "RollForward",
                "children": [
                  {
                    "name": "chain_sync.receive_header",
                    "target": "amaru_consensus::stages::receive_header",
                    "children": [
                      { "name": "chain_sync.decode_header", "target": "amaru_consensus::stages::receive_header" }
                    ]
                  },
                  { "name": "chain_sync.validate_header", "target": "amaru_consensus::stages::validate_header" },
                  { "name": "diffusion.fetch_block", "target": "amaru_consensus::stages::fetch_block" }
                ]
              },
              { "name": "manager", "target": "amaru_protocols::manager", "message_type": "FetchBlocks" },
              { "name": "connection", "target": "amaru_protocols::connection", "message_type": "FetchBlocks", "conn_id": "0", "peer": "127.0.0.1:3001", "role": "Initiator" },
              { "name": "chainsync.initiator.protocol", "target": "amaru_protocols::chainsync::initiator", "message_type": "AwaitReply" },
              { "name": "blockfetch.responder.protocol", "target": "amaru_protocols::blockfetch::responder", "message_type": "RequestRange" },
              { "name": "blockfetch.responder.stage", "target": "amaru_protocols::blockfetch::responder", "message_type": "RequestRange" },
              { "name": "blockfetch.initiator.stage", "target": "amaru_protocols::blockfetch::initiator", "message_type": "StartBatch" },
              { "name": "blockfetch.initiator.stage", "target": "amaru_protocols::blockfetch::initiator", "message_type": "Block" },
              { "name": "blockfetch.initiator.protocol", "target": "amaru_protocols::blockfetch::initiator", "message_type": "Block" },
              { "name": "blockfetch.initiator.stage", "target": "amaru_protocols::blockfetch::initiator", "message_type": "BatchDone" },
              { "name": "blockfetch.initiator.protocol", "target": "amaru_protocols::blockfetch::initiator", "message_type": "Done" },
              { "name": "manager", "target": "amaru_protocols::manager", "message_type": "ConnectionDied" }
            ]
          }
        )],
        config,
    );
}
