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

use amaru_observability::{RecordFields, amaru};

struct StubRecord;

impl RecordFields for StubRecord {
    fn bool(&self, name: &str) -> Option<bool> {
        match name {
            "full_duplex_capable" => Some(true),
            "full_duplex" => Some(false),
            _ => None,
        }
    }

    fn f64(&self, _: &str) -> Option<f64> {
        None
    }

    fn i64(&self, _: &str) -> Option<i64> {
        None
    }

    fn str(&self, name: &str) -> Option<&str> {
        match name {
            "peer" => Some("1.2.3.4:3001"),
            "direction" => Some("Outbound"),
            _ => None,
        }
    }

    fn u64(&self, name: &str) -> Option<u64> {
        match name {
            "conn_id" => Some(7),
            "consecutive_dormant_epochs" => Some(3),
            _ => None,
        }
    }
}

#[test]
fn generated_schema_identity_and_accessors_are_typed() {
    let record = StubRecord;

    assert_eq!(amaru::protocols::peer_selection::peer::CONNECTED::TARGET, "amaru::protocols");
    assert_eq!(amaru::protocols::peer_selection::peer::CONNECTED::NAME, "peer_selection.peer.connected");
    assert_eq!(amaru::protocols::peer_selection::peer::CONNECTED::FIELD_PEER, "peer");
    assert_eq!(amaru::protocols::peer_selection::peer::CONNECTED::FIELD_DIRECTION, "direction");
    assert!(amaru::protocols::peer_selection::peer::CONNECTED::matches(
        "amaru::protocols",
        "peer_selection.peer.connected"
    ));

    assert_eq!(amaru::protocols::peer_selection::peer::CONNECTED::peer(&record), "1.2.3.4:3001");
    assert_eq!(amaru::protocols::peer_selection::peer::CONNECTED::direction(&record), "Outbound");
    assert_eq!(amaru::protocols::peer_selection::peer::CONNECTED::conn_id(&record), 7);
    assert!(amaru::protocols::peer_selection::peer::CONNECTED::full_duplex_capable(&record));
    assert!(!amaru::protocols::peer_selection::peer::CONNECTED::full_duplex(&record));
    assert_eq!(amaru::ledger::governance_activity::UPDATE::consecutive_dormant_epochs(&record), 3);
}
