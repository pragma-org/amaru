// Copyright 2026 PRAGMA
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

//! Call sites may pass owned values, references, slices, `String`, and `&str`
//! without moving or cloning the original.

use amaru_observability_macros::{define_local_schemas, trace_event, trace_span};

#[derive(Clone, serde::Serialize, schemars::JsonSchema)]
struct SamplePeer {
    name: String,
}

impl SamplePeer {
    fn new(name: &str) -> Self {
        Self { name: name.to_owned() }
    }
}

define_local_schemas! {
    test {
        borrow {
            /// Owned / borrowed peer
            public PEER {
                required peer: SamplePeer
            }
            /// Slice of peers
            public PEERS {
                required peers: [SamplePeer]
            }
            /// Display peer
            public PEER_DISPLAY {
                required peer: %SamplePeer
            }
            /// String label
            public LABEL {
                required label: String
            }
        }
    }
}

impl std::fmt::Display for SamplePeer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}

fn use_owned_peer(peer: SamplePeer) {
    let _span = trace_span!(crate::test::borrow::PEER, peer);
    let _ = peer.name;
}

fn use_peer_ref(peer: &SamplePeer) {
    let _span = trace_span!(crate::test::borrow::PEER, peer);
    let _ = &peer.name;
}

fn use_peer_display(peer: SamplePeer) {
    let _span = trace_span!(crate::test::borrow::PEER_DISPLAY, peer);
    let _ = peer.name;
}

fn use_peers_vec(peers: Vec<SamplePeer>) {
    let _span = trace_span!(crate::test::borrow::PEERS, peers);
    let _ = peers.len();
}

fn use_peers_slice(peers: &[SamplePeer]) {
    let _span = trace_span!(crate::test::borrow::PEERS, peers);
    let _ = peers.len();
}

fn use_string_owned(label: String) {
    let _span = trace_span!(crate::test::borrow::LABEL, label);
    let _ = label.len();
}

fn use_str(label: &str) {
    let _span = trace_span!(crate::test::borrow::LABEL, label);
    let _ = label.len();
}

#[test]
fn owned_and_borrowed_values_compile_and_keep_the_original() {
    let peer = SamplePeer::new("a:1");
    use_owned_peer(peer.clone());
    use_peer_ref(&peer);
    use_peer_display(peer.clone());

    let peers = vec![peer.clone()];
    use_peers_vec(peers.clone());
    use_peers_slice(&peers);

    let label = String::from("tip");
    use_string_owned(label.clone());
    use_str(&label);
    use_str("static");

    trace_event!(INFO, crate::test::borrow::PEER, peer);
    trace_event!(INFO, crate::test::borrow::PEERS, peers);
    trace_event!(INFO, crate::test::borrow::LABEL, label = "event");
}
