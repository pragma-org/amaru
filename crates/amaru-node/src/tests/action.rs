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

//! Scenario events used by the node test harness and `amaru-sim`.
//!
//! These model chainsync-style roll-forward / rollback steps with simplified data so a generated
//! walk can be injected into an upstream test node.

use std::fmt::{Display, Formatter};

use amaru_kernel::{Hash, Header, HeaderHash, IsHeader, NetworkPoint, Peer, Slot, make_header, size::HEADER};
use hex::FromHexError;

/// A single roll-forward or rollback step from one peer.
///
/// JSON is kept compact so a failing simulation can dump a list of actions as a unit-test fixture.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Action {
    RollForward { peer: Peer, header: Header },
    Rollback { peer: Peer, rollback_point: NetworkPoint },
}

impl Action {
    pub fn hash(&self) -> HeaderHash {
        match self {
            Action::RollForward { header, .. } => header.hash(),
            Action::Rollback { rollback_point, .. } => rollback_point.hash(),
        }
    }

    pub fn parent_hash(&self) -> Option<HeaderHash> {
        match self {
            Action::RollForward { header, .. } => header.parent(),
            Action::Rollback { .. } => None,
        }
    }

    pub fn slot(&self) -> Slot {
        match self {
            Action::RollForward { header, .. } => header.slot(),
            Action::Rollback { rollback_point, .. } => rollback_point.slot_or_default(),
        }
    }

    pub fn pretty_print(&self) -> String {
        format!("r#\"{}\"#", serde_json::to_string(self).unwrap_or_else(|_| "<unserializable action>".into()))
    }

    pub fn set_peer(mut self, peer: &Peer) -> Self {
        match &mut self {
            Action::RollForward { peer: p, .. } => *p = peer.clone(),
            Action::Rollback { peer: p, .. } => *p = peer.clone(),
        }
        self
    }

    pub fn peer(&self) -> &Peer {
        match self {
            Action::RollForward { peer, .. } => peer,
            Action::Rollback { peer, .. } => peer,
        }
    }

    pub fn is_rollback(&self) -> bool {
        matches!(self, Action::Rollback { .. })
    }
}

struct SimplifiedHeader(Header);

impl serde::Serialize for SimplifiedHeader {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Header", 4)?;
        state.serialize_field("hash", &self.0.hash())?;
        state.serialize_field("block", &self.0.block_height())?;
        state.serialize_field("slot", &self.0.slot())?;
        state.serialize_field("parent", &self.0.parent().as_ref().map(|h| hex::encode(h.as_ref())))?;
        state.end()
    }
}

impl<'de> serde::Deserialize<'de> for SimplifiedHeader {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct SimplifiedHeaderHelper {
            hash: String,
            block: u64,
            slot: u64,
            parent: Option<String>,
        }

        let helper = SimplifiedHeaderHelper::deserialize(deserializer)?;

        let parent_hash = if let Some(parent_str) = helper.parent {
            Some(decode_hash(parent_str.as_str()).map_err(serde::de::Error::custom)?)
        } else {
            None
        };
        let header = make_header(helper.block, helper.slot, parent_hash);
        Ok(SimplifiedHeader(header.with_hash(decode_hash(&helper.hash).map_err(serde::de::Error::custom)?)))
    }
}

fn decode_hash(s: &str) -> Result<HeaderHash, FromHexError> {
    let bytes = hex::decode(s)?;
    let mut arr = [0u8; HEADER];
    arr.copy_from_slice(&bytes);
    Ok(Hash::from(arr))
}

impl serde::Serialize for Action {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Action::RollForward { peer, header } => {
                ActionHelper::RollForward { peer: peer.to_string(), header: SimplifiedHeader(header.clone()) }
                    .serialize(serializer)
            }
            Action::Rollback { peer, rollback_point } => {
                ActionHelper::Rollback { peer: peer.to_string(), rollback_point: *rollback_point }.serialize(serializer)
            }
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
enum ActionHelper {
    RollForward {
        peer: String,
        header: SimplifiedHeader,
    },
    Rollback {
        peer: String,
        #[serde(serialize_with = "serialize_point", deserialize_with = "deserialize_point")]
        rollback_point: NetworkPoint,
    },
}

impl<'de> serde::Deserialize<'de> for Action {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let helper = ActionHelper::deserialize(deserializer)?;
        match helper {
            ActionHelper::RollForward { peer, header } => {
                Ok(Action::RollForward { peer: Peer::new(&peer), header: header.0 })
            }
            ActionHelper::Rollback { peer, rollback_point } => {
                Ok(Action::Rollback { peer: Peer::new(&peer), rollback_point })
            }
        }
    }
}

impl Display for Action {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Action::RollForward { peer, header, .. } => {
                write!(f, "Forward peer {peer} to {}", header.hash())
            }
            Action::Rollback { peer, rollback_point } => write!(f, "Rollback peer {peer} to {}", rollback_point.hash()),
        }
    }
}

fn serialize_point<S: serde::Serializer>(point: &NetworkPoint, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&point.to_string())
}

fn deserialize_point<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<NetworkPoint, D::Error> {
    let bytes: &str = serde::Deserialize::deserialize(deserializer)?;
    NetworkPoint::try_from(bytes).map_err(serde::de::Error::custom)
}
