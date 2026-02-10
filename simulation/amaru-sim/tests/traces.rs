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

use amaru::tests::configuration::NodeConfig;
use amaru::tests::setup::{create_nodes, run_nodes};
use amaru_tracing_json::assert_spans_trees;
use serde_json::json;
use tracing::info_span;

#[test]
fn run_simulator_with_traces() {
    let execute = || {
        let responder_config = NodeConfig::responder().with_chain_length(1);
        let mut nodes = create_nodes(vec![NodeConfig::initiator(), responder_config]).unwrap();
        info_span!(target: "amaru_consensus", "handle_msg").in_scope(|| {
            run_nodes(&mut nodes);
        });
    };

    assert_spans_trees(
        execute,
        vec![json!(
            {
              "name": "handle_msg",
              "target": "amaru_consensus",
              "children": [
                // Protocol manager
                { "name": "manager", "target": "amaru_protocols::manager", "message_type": "AddPeer" },
                { "name": "manager", "target": "amaru_protocols::manager", "message_type": "Listen" },
                { "name": "manager", "target": "amaru_protocols::manager", "message_type": "Listen" },
                { "name": "manager", "target": "amaru_protocols::manager", "message_type": "Connect" },
                // Connection initialization
                { "name": "connection", "target": "amaru_protocols::connection", "conn_id": "0", "peer": "127.0.0.1:3000", "role": "Initiator", "message_type": "Initialize" },
                { "name": "manager", "target": "amaru_protocols::manager", "message_type": "Accepted" },
                { "name": "connection", "target": "amaru_protocols::connection", "conn_id": "1", "peer": "127.0.0.1:0", "role": "Responder", "message_type": "Initialize" },
                // Handshake
                { "name": "handshake.responder", "target": "amaru_protocols::handshake::responder", "message_type": "Propose" },
                { "name": "connection", "target": "amaru_protocols::connection", "conn_id": "1", "peer": "127.0.0.1:0", "role": "Responder", "message_type": "Handshake" },
                { "name": "handshake.initiator", "target": "amaru_protocols::handshake::initiator", "message_type": "Accept" },
                { "name": "connection", "target": "amaru_protocols::connection", "conn_id": "0", "peer": "127.0.0.1:3000", "role": "Initiator", "message_type": "Handshake" },
                // Chainsync + Tx submission
                { "name": "diffusion.chain_sync", "target": "amaru_consensus::stages::pull", "message_type": "Initialize" },
                { "name": "tx_submission.responder", "target": "amaru_protocols::tx_submission::responder", "message_type": "Init" },
                { "name": "chainsync.responder", "target": "amaru_protocols::chainsync::responder", "message_type": "FindIntersect" },
                { "name": "tx_submission.initiator", "target": "amaru_protocols::tx_submission::initiator", "message_type": "RequestTxIdsBlocking" },
                { "name": "chainsync.initiator", "target": "amaru_protocols::chainsync::initiator", "message_type": "IntersectFound" },
                { "name": "diffusion.chain_sync", "target": "amaru_consensus::stages::pull", "message_type": "IntersectFound" },
                { "name": "tx_submission.responder", "target": "amaru_protocols::tx_submission::responder", "message_type": "ReplyTxIds" },
                { "name": "chainsync.responder", "target": "amaru_protocols::chainsync::responder", "message_type": "RequestNext" },
                { "name": "chainsync.responder", "target": "amaru_protocols::chainsync::responder", "message_type": "RequestNext" },
                { "name": "tx_submission.initiator", "target": "amaru_protocols::tx_submission::initiator", "message_type": "RequestTxIdsBlocking" },
                { "name": "chainsync.initiator", "target": "amaru_protocols::chainsync::initiator", "message_type": "RollBackward" },
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
                { "name": "tx_submission.responder", "target": "amaru_protocols::tx_submission::responder", "message_type": "ReplyTxIds" },
                { "name": "chainsync.initiator", "target": "amaru_protocols::chainsync::initiator", "message_type": "AwaitReply" },
                { "name": "tx_submission.initiator", "target": "amaru_protocols::tx_submission::initiator", "message_type": "RequestTxIdsBlocking" },
                { "name": "tx_submission.responder", "target": "amaru_protocols::tx_submission::responder", "message_type": "ReplyTxIds" },
                { "name": "tx_submission.initiator", "target": "amaru_protocols::tx_submission::initiator", "message_type": "RequestTxIdsBlocking" },
                { "name": "tx_submission.responder", "target": "amaru_protocols::tx_submission::responder", "message_type": "ReplyTxIds" },
                { "name": "tx_submission.initiator", "target": "amaru_protocols::tx_submission::initiator", "message_type": "RequestTxIdsBlocking" },
                { "name": "tx_submission.responder", "target": "amaru_protocols::tx_submission::responder", "message_type": "ReplyTxIds" },
                { "name": "tx_submission.initiator", "target": "amaru_protocols::tx_submission::initiator", "message_type": "RequestTxIdsBlocking" }
              ]
            }
        )],
        vec!["amaru_consensus", "amaru_protocols"],
        vec!["amaru_protocols::mux"],
    );
}
