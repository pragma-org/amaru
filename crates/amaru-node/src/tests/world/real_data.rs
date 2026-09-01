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

//! Recorded-chain world tests.
//!
//! Fragments taken from a live network. Production `build_node`; only the
//! connections resource is the simulated wire. See EDR-011
//! "World tests: generated vs recorded chains".

use std::{net::SocketAddr, sync::Arc, time::Duration};

use amaru_ouroboros::ConnectionsResource;
use amaru_pure_stage::trace_buffer::TraceBuffer;

use super::{
    HONEST_PAYLOAD_DELAY_MAX_NANOS, HeapLogKind, LONG_TAIL_PAYLOAD_EVERY, WorldConnectionProvider, WorldLoop,
    build_world_node,
    support::{
        SEED, entry_chainsync_initiator_kind, entry_chainsync_roll_forward_hash, entry_is_validate_header_of,
        fragment_trace_guards,
    },
};
use crate::tests::configuration::NodeTestConfig;

/// Five bootstrap nodes in a line, head dialing a primed store that already holds a
/// recorded preprod epoch. Same initial chain as generated P-join; the source is the
/// primed RocksDB, not an injector. Production validation; simulated wire only.
///
/// Primed startup keeps the persisted best chain. Catch-up nodes realign to the
/// bootstrap snapshot and use up to three upstreams (static chain + shared).
/// Populate `run_until` uses embedded big-ledger peers (up to 10).
/// The test installs [`crate::Telemetry`] so WorldLoop has a tracing subscriber
/// even when `primed/` already exists.
///
/// Missing stores are produced on first run (CDN bootstrap + live `run_until`).
/// Not `#[tokio::test]`: production graphs may `Handle::block_on` DurationDist::Zero effects.
const CATCH_UP_NODES: usize = 5;
#[test]
#[ignore = "first run downloads a preprod snapshot and syncs one epoch from the network"]
fn test_world_disseminates_preprod_fragment() {
    use std::cmp::Ordering;

    use amaru_consensus::stages::select_chain::cmp_tip;
    use amaru_kernel::{IsHeader, PREPROD_ERA_HISTORY, PREPROD_GLOBAL_PARAMETERS};
    use amaru_ouroboros::BaseReadChainStore;
    use amaru_protocols::store_effects::{ResourceHeaderStore, ResourceParameters};

    use super::fragment::{
        copy_dir, covers_following_epoch, ensure_fragment_stores, fixture_root, header_hash_from_snapshot_point,
        linear_fragment_to_head, load_committed_meta, open_chain_store, parse_slot_from_point,
    };
    use crate::Telemetry;

    let _guards = fragment_trace_guards();

    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let handle = runtime.handle().clone();
    let _telemetry = runtime.block_on(Telemetry::install(crate::LogFormat::Ansi)).expect("install tracing subscriber");

    let root = fixture_root();
    ensure_fragment_stores(&root).expect("produce preprod fragment stores");
    let meta = load_committed_meta(&root).expect("meta.json");

    let primed_tmp = tempfile::tempdir().expect("primed temp");
    copy_dir(&root.join("primed/chain"), &primed_tmp.path().join("chain")).expect("copy primed chain");
    copy_dir(&root.join("primed/ledger"), &primed_tmp.path().join("ledger")).expect("copy primed ledger");
    let node_tmps: Vec<_> = (0..CATCH_UP_NODES).map(|_| tempfile::tempdir().expect("node temp")).collect();
    for tmp in &node_tmps {
        copy_dir(&root.join("bootstrap/chain"), &tmp.path().join("chain")).expect("copy bootstrap chain");
        copy_dir(&root.join("bootstrap/ledger"), &tmp.path().join("ledger")).expect("copy bootstrap ledger");
    }

    let primed_chain_path = primed_tmp.path().join("chain");
    let snapshot_hash = header_hash_from_snapshot_point(&meta.latest_snapshot_point).expect("snapshot hash");
    let (served_head, served_fragment) = {
        let store = open_chain_store(&primed_chain_path).expect("open primed chain");
        let served_tip = store.get_best_chain_tip();
        assert_ne!(served_tip.hash(), snapshot_hash, "primed best tip must be after the bootstrap snapshot");
        assert!(store.has_block(&served_tip.hash()).expect("has_block"), "best tip must have a stored body");
        let head = store.load_header(&served_tip.hash()).expect("best tip header");
        let fragment = linear_fragment_to_head(&store, snapshot_hash, head.clone()).expect("parent walk to best tip");
        assert_eq!(fragment.last().map(|h| h.point()), Some(head.point()));
        assert_ne!(
            format!("{}", fragment[0].point()),
            format!("{}", head.point()),
            "best tip must not be the first header after the snapshot"
        );
        let snapshot_slot = parse_slot_from_point(&meta.latest_snapshot_point).expect("snapshot slot");
        assert!(
            covers_following_epoch(head.slot().as_u64(), snapshot_slot),
            "fragment should cover most of an epoch after the snapshot; got {} headers from {} to {}",
            fragment.len(),
            fragment[0].point(),
            head.point()
        );
        (head, fragment)
    };
    let served_tip = served_head.point();

    let offset = PREPROD_ERA_HISTORY
        .slot_to_relative_time_unchecked_horizon(served_head.slot())
        .expect("fragment slot in era history")
        + Duration::from_secs(30);

    let provider = Arc::new(WorldConnectionProvider::with_long_tail_payload_delay(SEED));

    let primed_addr: SocketAddr = "127.0.0.1:9330".parse().expect("primed addr");
    let listen_primed = primed_addr.to_string();
    let node_addrs: Vec<String> = (0..CATCH_UP_NODES).map(|i| format!("127.0.0.1:{}", 9341 + i as u16)).collect();

    let connections: ConnectionsResource = provider.clone();
    let node_primed = NodeTestConfig::default()
        .with_no_upstream_peers()
        .with_listen_address(&listen_primed)
        .with_seed(21)
        .with_target_upstream_peers(1)
        .with_peer_mix("static~1")
        .with_keep_persisted_best_chain()
        .with_trace_buffer(TraceBuffer::new_shared(50_000, 64_000_000))
        .with_store_dirs(primed_tmp.path().join("chain"), primed_tmp.path().join("ledger"))
        .with_global_epoch_offset(offset);
    let sim_primed = build_world_node(&node_primed, connections.clone(), &handle).expect("primed node");

    let mut graphs = vec![sim_primed];
    for (i, listen) in node_addrs.iter().enumerate() {
        let upstream = if i == 0 {
            listen_primed.parse().expect("primed listen is an IPv4 socket")
        } else {
            node_addrs[i - 1].parse().expect("node listen is an IPv4 socket")
        };
        let node = NodeTestConfig::default()
            .with_upstream_peer(upstream)
            .with_listen_address(listen)
            .with_seed(31 + i as u64)
            .with_target_upstream_peers(3)
            .with_peer_mix("static!1, shared~6")
            .with_trace_buffer(TraceBuffer::new_shared(50_000, 64_000_000))
            .with_store_dirs(node_tmps[i].path().join("chain"), node_tmps[i].path().join("ledger"))
            .with_global_epoch_offset(offset);
        graphs.push(build_world_node(&node, connections.clone(), &handle).expect("catch-up node"));
    }

    let primed_store = {
        let store = graphs[0].resources().get::<ResourceHeaderStore>().expect("primed chain store");
        Arc::clone(&*store)
    };
    let receiver_store = {
        let store = graphs[1].resources().get::<ResourceHeaderStore>().expect("first catch-up chain store");
        Arc::clone(&*store)
    };

    assert_eq!(
        primed_store.get_best_chain_tip(),
        served_tip,
        "world startup must keep the primed store's validated best tip"
    );
    for earlier in &served_fragment[..served_fragment.len() - 1] {
        assert_eq!(
            cmp_tip(Some(&served_head), Some(earlier)),
            Ordering::Greater,
            "served HEAD must win cmp_tip against earlier headers"
        );
    }
    assert!(receiver_store.load_header(&served_tip.hash()).is_none(), "receiver must start without the served HEAD");
    assert_ne!(
        receiver_store.get_best_chain_tip(),
        served_tip,
        "receiver best tip starts at bootstrap, not the served HEAD"
    );
    // Header + body payloads, five hops. One in LONG_TAIL_PAYLOAD_EVERY can sit at the hop cap.
    let hop_coverage = (served_fragment.len() as u64).saturating_mul(2).saturating_mul(CATCH_UP_NODES as u64);
    let long_tail_hops = hop_coverage.div_ceil(LONG_TAIL_PAYLOAD_EVERY);
    let horizon_nanos =
        long_tail_hops.saturating_add(1).saturating_mul(HONEST_PAYLOAD_DELAY_MAX_NANOS).saturating_add(2_000_000_000);
    eprintln!(
        "catch-up served HEAD {served_tip} snapshot={snapshot_hash} fragment_len={} nodes={CATCH_UP_NODES} horizon_nanos={horizon_nanos}",
        served_fragment.len()
    );

    let mut world = WorldLoop::new(provider, graphs);
    let wall_start = std::time::Instant::now();
    world.run_until_horizon_with(horizon_nanos, Duration::from_secs(2), |world| {
        let last =
            world.graphs().last().expect("catch-up node").resources().get::<ResourceHeaderStore>().expect("tail store");
        let have = served_fragment.iter().filter(|h| last.load_header(&h.hash()).is_some()).count();
        let tips: Vec<_> = world
            .graphs()
            .iter()
            .map(|g| format!("{}", g.resources().get::<ResourceHeaderStore>().expect("store").get_best_chain_tip()))
            .collect();
        eprintln!(
            "catch-up wall={:?} sim={:?} next={:?} events={} tail_have={have}/{} tips={tips:?}",
            wall_start.elapsed(),
            world.graphs()[0].now().sim_elapsed(),
            world.peek_next_event_time(),
            world.heap_len(),
            served_fragment.len(),
        );
    });
    let wall = wall_start.elapsed();

    for graph in world.graphs() {
        let params = graph.resources().get::<ResourceParameters>().expect("production GlobalParameters");
        assert_eq!(params.consensus_security_param, PREPROD_GLOBAL_PARAMETERS.consensus_security_param);
        assert_eq!(params.consensus_security_param, 2160, "production k, not chain_length");
    }

    let log = world.heap_log();
    let connects = log.iter().filter(|e| matches!(e.kind, HeapLogKind::ConnectAttempt { .. })).count();
    let primed_connects = log
        .iter()
        .filter(|e| matches!(e.kind, HeapLogKind::ConnectAttempt { target } if target == primed_addr))
        .count();
    assert!(
        connects > CATCH_UP_NODES,
        "peer sharing must add connections beyond the initial {CATCH_UP_NODES}-hop chain; connects={connects}"
    );
    assert!(
        primed_connects >= 2,
        "at least one node besides the chain head must dial the primed store; primed_connects={primed_connects}"
    );

    let primed_after = world.graphs()[0].resources().get::<ResourceHeaderStore>().expect("primed store");
    let primed_tip = primed_after.get_best_chain_tip();
    let primed_header = primed_after.load_header(&primed_tip.hash()).expect("primed tip header");
    let head_hash = served_head.hash();
    let tail = world.graphs().len() - 1;
    let tail_traces = world.graphs()[tail].trace_buffer().lock().hydrate_without_timestamps();
    let tail_store = world.graphs()[tail].resources().get::<ResourceHeaderStore>().expect("tail store");
    let tail_have = served_fragment.iter().filter(|h| tail_store.load_header(&h.hash()).is_some()).count();
    let rf_hashes: Vec<_> = tail_traces.iter().filter_map(entry_chainsync_roll_forward_hash).collect();
    let saw_head_rf = rf_hashes.contains(&head_hash);
    let saw_rollback = tail_traces
        .iter()
        .any(|entry| entry_chainsync_initiator_kind(entry).is_some_and(|kind| kind.starts_with("RollBackward")));
    eprintln!(
        "catch-up after WorldLoop wall={wall:?} sim={:?} next={:?} primed_tip={primed_tip} tail_have={tail_have}/{} connect={connects} primed_connects={primed_connects} head_rf={saw_head_rf} rollback={saw_rollback} dropped={}",
        world.graphs()[0].now().sim_elapsed(),
        world.peek_next_event_time(),
        served_fragment.len(),
        world.graphs()[tail].trace_buffer().lock().dropped_messages(),
    );

    for (i, graph) in world.graphs().iter().enumerate().skip(1) {
        let store = graph.resources().get::<ResourceHeaderStore>().expect("node chain store");
        assert!(store.load_header(&head_hash).is_some(), "node {i} must have the served HEAD in store");
        assert!(store.has_block(&head_hash).expect("has_block"), "node {i} must have the served HEAD block body");
        let tip = store.get_best_chain_tip();
        let got = store.load_header(&tip.hash()).expect("node tip header");
        assert_ne!(
            cmp_tip(Some(&got), Some(&served_head)),
            Ordering::Less,
            "node {i} tip {tip} must reach at least the served HEAD"
        );
        assert_eq!(
            cmp_tip(Some(&got), Some(&primed_header)),
            Ordering::Equal,
            "node {i} tip {tip} must be cmp_tip-equal to the primed best tip {primed_tip}"
        );
        assert_eq!(tip, primed_tip, "node {i} best-chain pointer must match primed");
    }

    assert!(
        tail_traces.iter().any(|entry| entry_chainsync_roll_forward_hash(entry) == Some(head_hash)),
        "tail node must see a typed chainsync RollForward of the served HEAD; head={head_hash}"
    );
    let any_validated = world.graphs().iter().skip(1).any(|graph| {
        graph
            .trace_buffer()
            .lock()
            .hydrate_without_timestamps()
            .iter()
            .any(|entry| entry_is_validate_header_of(entry, &head_hash))
    });
    assert!(
        any_validated,
        "at least one catch-up node must run production ValidateHeaderEffect on the served HEAD; head={head_hash}"
    );
}
