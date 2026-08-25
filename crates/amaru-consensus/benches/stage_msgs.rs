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

use std::{hint::black_box, time::Duration};

use amaru_consensus::stages::{
    adopt_chain::AdoptChainMsg, block_source::BlockSourceMsg, fetch_blocks::FetchBlocksMsg,
    select_chain::SelectChainMsg, track_peers::TrackPeersMsg, validate_block::ValidateBlockMsg,
};
use amaru_kernel::{
    BlockHeight, EraHistory, EraName, Peer, Point, any_header_hash,
    cardano::network_block::{NetworkBlock, make_block},
    make_header,
    utils::tests::run_strategy,
};
use amaru_ouroboros::ConnectionId;
use amaru_protocols::chainsync::{ChainSyncInitiatorMsg, HeaderContent, InitiatorMessage, InitiatorResult};
use amaru_pure_stage::{SendData, serde::to_cbor};
use criterion::{Criterion, criterion_group, criterion_main};

fn stage_msgs(c: &mut Criterion) {
    let mut group = c.benchmark_group("Stage Messages");
    group.measurement_time(Duration::from_secs(5));

    let bh = BlockHeight::from(123_456_789);
    let point = Point::Specific(1_234_567_890.into(), run_strategy(any_header_hash()), bh);

    let msg = ValidateBlockMsg::new(point, point, bh);
    let msg: Box<dyn SendData> = Box::new(msg);
    group.bench_function("ValidateBlockMsg", |b| b.iter(|| black_box(to_cbor(black_box(&msg)))));

    let msg = AdoptChainMsg::new(point, bh);
    let msg: Box<dyn SendData> = Box::new(msg);
    group.bench_function("AdoptChainMsg", |b| b.iter(|| black_box(to_cbor(black_box(&msg)))));

    let msg = BlockSourceMsg::Validation { valid: true, point };
    let msg: Box<dyn SendData> = Box::new(msg);
    group.bench_function("BlockSourceMsg::Validation", |b| b.iter(|| black_box(to_cbor(black_box(&msg)))));

    let msg = SelectChainMsg::tip_from_upstream(point, point);
    let msg: Box<dyn SendData> = Box::new(msg);
    group.bench_function("SelectChainMsg::TipFromUpstream", |b| b.iter(|| black_box(to_cbor(black_box(&msg)))));

    let msg = SelectChainMsg::block_validation_result(point, true, BlockHeight::from(0));
    let msg: Box<dyn SendData> = Box::new(msg);
    group.bench_function("SelectChainMsg::BlockValidationResult", |b| b.iter(|| black_box(to_cbor(black_box(&msg)))));

    let msg = SelectChainMsg::fetch_next_from(point);
    let msg: Box<dyn SendData> = Box::new(msg);
    group.bench_function("SelectChainMsg::FetchNextFrom", |b| b.iter(|| black_box(to_cbor(black_box(&msg)))));

    let msg = FetchBlocksMsg::new_tip(point, point);
    let msg: Box<dyn SendData> = Box::new(msg);
    group.bench_function("FetchBlocksMsg::NewTip", |b| b.iter(|| black_box(to_cbor(black_box(&msg)))));

    let msg = FetchBlocksMsg::recover_stored_blocks(point, point.hash());
    let msg: Box<dyn SendData> = Box::new(msg);
    group.bench_function("FetchBlocksMsg::RecoverStoredBlocks", |b| b.iter(|| black_box(to_cbor(black_box(&msg)))));

    let msg = FetchBlocksMsg::Timeout(1000);
    let msg: Box<dyn SendData> = Box::new(msg);
    group.bench_function("FetchBlocksMsg::Timeout", |b| b.iter(|| black_box(to_cbor(black_box(&msg)))));

    let block = make_block();
    #[allow(clippy::expect_used)]
    let nb = NetworkBlock::new(&EraHistory::default(), &block).expect("minimal network block");
    let header = make_header(1234, 12345, None);
    let header_content = HeaderContent::new(&header, EraName::Conway);

    let msg = FetchBlocksMsg::Block(Peer::for_test(3013), nb.clone());
    let msg: Box<dyn SendData> = Box::new(msg);
    group.bench_function("FetchBlocksMsg::Block", |b| b.iter(|| black_box(to_cbor(black_box(&msg)))));

    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: Peer::for_test(3001),
        conn_id: ConnectionId::initial(),
        handler: amaru_pure_stage::StageRef::<InitiatorMessage>::named_for_tests("test"),
        msg: InitiatorResult::Initialize,
    });
    let msg: Box<dyn SendData> = Box::new(msg);
    group.bench_function("TrackPeersMsg::FromUpstream", |b| b.iter(|| black_box(to_cbor(black_box(&msg)))));

    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: Peer::for_test(3001),
        conn_id: ConnectionId::initial(),
        handler: amaru_pure_stage::StageRef::<InitiatorMessage>::named_for_tests("test"),
        msg: InitiatorResult::RollForward(header_content.clone(), point),
    });
    let msg: Box<dyn SendData> = Box::new(msg);
    group
        .bench_function("TrackPeersMsg::FromUpstream(RollForward)", |b| b.iter(|| black_box(to_cbor(black_box(&msg)))));

    group.finish();
}

criterion_group!(
    name = benches;
    config = Criterion::default().measurement_time(Duration::from_secs(10));
    targets = stage_msgs
);
criterion_main!(benches);
