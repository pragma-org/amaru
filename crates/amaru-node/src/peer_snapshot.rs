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

//! Cardano ledger peer snapshot (big ledger pools) JSON loading.
//!
//! Compatible with the `mainnet-peer-snapshot.json` format shipped by cardano-node.
//! Known-network snapshots may be embedded at build time (see `build/peer_snapshot.rs`).

use std::{
    collections::BTreeSet,
    fs,
    num::NonZeroU16,
    path::{Path, PathBuf},
};

use amaru_kernel::{NetworkMagic, NetworkName, NetworkPoint, PEER_SNAPSHOT_NETWORKS, Peer, Slot, size::HEADER};
use serde::Deserialize;
use thiserror::Error;

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/embedded_peer_snapshots.rs"));
}

/// Default N2N relay port when a snapshot relay omits `port`.
pub const DEFAULT_RELAY_PORT: NonZeroU16 = NonZeroU16::new(3001).unwrap();

/// Loaded peer snapshot after validation, ready for peer selection.
#[derive(Debug, Clone, PartialEq)]
pub struct PeerSnapshot {
    pub network_magic: NetworkMagic,
    pub node_to_client_version: u64,
    pub point: NetworkPoint,
    pub peers: BTreeSet<Peer>,
    pub pool_count: usize,
}

#[derive(Debug, Error)]
pub enum PeerSnapshotError {
    #[error("failed to read peer snapshot {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse peer snapshot {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("peer snapshot network magic mismatch for {path}: file has {file_magic}, expected {expected}")]
    NetworkMagicMismatch { path: PathBuf, file_magic: u64, expected: NetworkMagic },
    #[error("failed to convert snapshot point to NetworkPoint: {0}")]
    PointConversion(#[from] PointConversionError),
}

#[derive(Debug, Error)]
pub enum PointConversionError {
    #[error("blockPointHash is not valid hex: {0}")]
    InvalidHex(#[from] hex::FromHexError),
    #[error("blockPointHash is not the expected length of {HEADER} bytes")]
    InvalidLength(#[from] std::array::TryFromSliceError),
}

#[derive(Debug, Deserialize)]
struct SnapshotFile {
    #[serde(rename = "NetworkMagic")]
    network_magic: u64,
    #[serde(rename = "NodeToClientVersion")]
    node_to_client_version: u64,
    #[serde(rename = "Point")]
    point: SnapshotPoint,
    // Official files use camelCase for this key (not PascalCase).
    #[serde(rename = "bigLedgerPools")]
    big_ledger_pools: Vec<BigLedgerPool>,
}

#[derive(Debug, Deserialize)]
struct SnapshotPoint {
    #[serde(rename = "blockPointHash")]
    block_point_hash: String,
    #[serde(rename = "blockPointSlot")]
    block_point_slot: u64,
}

#[derive(Debug, Deserialize)]
struct BigLedgerPool {
    relays: Vec<SnapshotRelay>,
}

#[derive(Debug, Deserialize)]
struct SnapshotRelay {
    address: String,
    port: Option<NonZeroU16>,
}

/// Load and validate a peer snapshot file against the expected network magic.
pub fn load_peer_snapshot(path: &Path, expected_magic: NetworkMagic) -> Result<PeerSnapshot, PeerSnapshotError> {
    let bytes = fs::read(path).map_err(|source| PeerSnapshotError::Io { path: path.to_path_buf(), source })?;
    parse_peer_snapshot_bytes(&bytes, path, expected_magic)
}

/// Load the peer snapshot embedded in this binary for a network in
/// [`PEER_SNAPSHOT_NETWORKS`] (for example mainnet, preprod, preview), if any.
///
/// Returns `None` when the network is not in that list, or when no snapshot was staged
/// at build time for it (e.g. offline build).
pub fn load_embedded_peer_snapshot(network: NetworkName) -> Result<Option<PeerSnapshot>, PeerSnapshotError> {
    if !PEER_SNAPSHOT_NETWORKS.contains(&network) {
        return Ok(None);
    }
    let network_key = network.to_string();
    let Some(bytes) = embedded::embedded_peer_snapshot(&network_key) else {
        return Ok(None);
    };
    let path = Path::new("embedded").join(&network_key).join("peer-snapshot.json");
    Ok(Some(parse_peer_snapshot_bytes(bytes, &path, network.to_network_magic())?))
}

/// Configs repo commit used when the embedded snapshots were last refreshed at build time.
pub fn embedded_configs_commit() -> Option<&'static str> {
    embedded::CONFIGS_COMMIT
}

/// Parse peer snapshot JSON bytes (used by tests and [`load_peer_snapshot`]).
pub fn parse_peer_snapshot_bytes(
    bytes: &[u8],
    path: &Path,
    expected_magic: NetworkMagic,
) -> Result<PeerSnapshot, PeerSnapshotError> {
    let file: SnapshotFile =
        serde_json::from_slice(bytes).map_err(|source| PeerSnapshotError::Json { path: path.to_path_buf(), source })?;

    if file.network_magic != expected_magic.as_u64() {
        return Err(PeerSnapshotError::NetworkMagicMismatch {
            path: path.to_path_buf(),
            file_magic: file.network_magic,
            expected: expected_magic,
        });
    }

    let pool_count = file.big_ledger_pools.len();
    let mut peers = BTreeSet::new();
    for pool in &file.big_ledger_pools {
        for relay in &pool.relays {
            let port = relay.port.unwrap_or(DEFAULT_RELAY_PORT).get();
            peers.insert(Peer::new(&format!("{}:{}", relay.address, port)));
        }
    }

    Ok(PeerSnapshot {
        network_magic: NetworkMagic::new(file.network_magic),
        node_to_client_version: file.node_to_client_version,
        point: NetworkPoint::try_from(file.point)?,
        peers,
        pool_count,
    })
}

impl TryFrom<SnapshotPoint> for NetworkPoint {
    type Error = PointConversionError;

    fn try_from(value: SnapshotPoint) -> Result<Self, Self::Error> {
        let slot = Slot::new(value.block_point_slot);
        let hash = hex::decode(value.block_point_hash)?;
        let hash = <[u8; HEADER]>::try_from(&*hash)?;
        let hash = amaru_kernel::HeaderHash::new(hash);
        Ok(NetworkPoint::Specific(slot, hash))
    }
}

#[cfg(test)]
mod tests {
    use amaru_kernel::HeaderHash;

    use super::*;

    const SAMPLE: &str = r#"{
  "NetworkMagic": 764824073,
  "NodeToClientVersion": 23,
  "Point": {
    "blockPointHash": "1a7f1af3e52ba8810247f7c82431113a61c2efc5435a8fe6f76c5ae6618cc92a",
    "blockPointSlot": 185831188
  },
  "bigLedgerPools": [
    {
      "accumulatedStake": 0.01,
      "relativeStake": 0.01,
      "relays": [
        {"address": "relay-a.example", "port": 3001},
        {"address": "relay-b.example"}
      ]
    },
    {
      "accumulatedStake": 0.02,
      "relativeStake": 0.01,
      "relays": [
        {"address": "10.0.0.1", "port": 6000},
        {"address": "relay-a.example", "port": 3001}
      ]
    }
  ]
}"#;

    #[test]
    fn parses_relays_with_default_port_and_dedup() {
        let snap = parse_peer_snapshot_bytes(SAMPLE.as_bytes(), Path::new("sample.json"), NetworkMagic::MAINNET)
            .expect("parse");
        assert_eq!(
            snap.point,
            NetworkPoint::Specific(
                185831188.into(),
                HeaderHash::new([
                    0x1a, 0x7f, 0x1a, 0xf3, 0xe5, 0x2b, 0xa8, 0x81, 0x02, 0x47, 0xf7, 0xc8, 0x24, 0x31, 0x11, 0x3a,
                    0x61, 0xc2, 0xef, 0xc5, 0x43, 0x5a, 0x8f, 0xe6, 0xf7, 0x6c, 0x5a, 0xe6, 0x61, 0x8c, 0xc9, 0x2a,
                ])
            )
        );
        assert_eq!(snap.node_to_client_version, 23);
        assert_eq!(snap.pool_count, 2);
        assert_eq!(
            snap.peers,
            BTreeSet::from([
                Peer::new("10.0.0.1:6000"),
                Peer::new("relay-a.example:3001"),
                Peer::new("relay-b.example:3001"),
            ])
        );
    }

    #[test]
    fn rejects_network_magic_mismatch() {
        let err = parse_peer_snapshot_bytes(SAMPLE.as_bytes(), Path::new("sample.json"), NetworkMagic::PREPROD)
            .expect_err("magic");
        assert!(matches!(err, PeerSnapshotError::NetworkMagicMismatch { .. }));
    }

    #[test]
    fn rejects_invalid_json() {
        let err =
            parse_peer_snapshot_bytes(b"{not json", Path::new("bad.json"), NetworkMagic::MAINNET).expect_err("json");
        assert!(matches!(err, PeerSnapshotError::Json { .. }));
    }

    #[test]
    fn empty_pools_yield_empty_peers() {
        let json = r#"{
              "NetworkMagic": 1,
              "NodeToClientVersion": 23,
              "Point": {"blockPointHash": "1a7f1af3e52ba8810247f7c82431113a61c2efc5435a8fe6f76c5ae6618cc92a", "blockPointSlot": 1},
              "bigLedgerPools": []
            }"#;
        let snap =
            parse_peer_snapshot_bytes(json.as_bytes(), Path::new("empty.json"), NetworkMagic::PREPROD).expect("parse");
        assert!(snap.peers.is_empty());
        assert_eq!(snap.pool_count, 0);
    }

    #[test]
    fn rejects_zero_port() {
        let json = r#"{
              "NetworkMagic": 764824073,
              "NodeToClientVersion": 23,
              "Point": {"blockPointHash": "1a7f1af3e52ba8810247f7c82431113a61c2efc5435a8fe6f76c5ae6618cc92a", "blockPointSlot": 1},
              "bigLedgerPools": [
                {"relays": [{"address": "relay.example", "port": 0}]}
              ]
            }"#;
        let err = parse_peer_snapshot_bytes(json.as_bytes(), Path::new("zero-port.json"), NetworkMagic::MAINNET)
            .expect_err("port");
        assert!(matches!(err, PeerSnapshotError::Json { .. }));
    }

    #[test]
    fn embedded_mainnet_parses_when_staged_at_build() {
        // Best-effort: when the build could not fetch/stage snapshots, this is a no-op pass.
        let Some(snap) = load_embedded_peer_snapshot(NetworkName::Mainnet).unwrap() else {
            eprintln!("skipping embedded snapshot test");
            return;
        };
        assert_eq!(snap.network_magic, NetworkMagic::MAINNET);
        assert!(!snap.peers.is_empty());
    }
}
