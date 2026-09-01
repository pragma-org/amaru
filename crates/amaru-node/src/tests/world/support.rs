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

//! Shared trace matchers for `generated` and `real_data` world tests.

use amaru_kernel::IsHeader;
use amaru_pure_stage::{
    Effect, TraceMatch, register_data_deserializer, register_effect_deserializer, tm_external_effect_any,
    trace_buffer::TraceEntry,
};

use super::WorldLoop;

pub(super) const SEED: u64 = 0xA11CE;

/// Same env var as `amaru-sim`. Empty or unset draws a fresh seed; a value replays that run.
pub(super) const TEST_SEED_ENV: &str = "AMARU_TEST_SEED";

pub(super) fn env_test_seed() -> Option<u64> {
    let raw = std::env::var(TEST_SEED_ENV).ok()?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    Some(parse_test_seed(raw).unwrap_or_else(|| {
        panic!("{TEST_SEED_ENV}={raw:?} is not a decimal or 0x-hex u64");
    }))
}

fn parse_test_seed(raw: &str) -> Option<u64> {
    if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok()
    } else {
        raw.parse().ok()
    }
}

pub(super) fn draw_test_seed() -> u64 {
    env_test_seed().unwrap_or_else(amaru_kernel::utils::tests::random_u64)
}

/// `n` fresh seeds, or a single env seed when replaying.
pub(super) fn test_seeds(n: u32) -> Vec<u64> {
    match env_test_seed() {
        Some(seed) => vec![seed],
        None => (0..n).map(|_| amaru_kernel::utils::tests::random_u64()).collect(),
    }
}

pub(super) fn derive_seed(master: u64, tag: u64) -> u64 {
    let mut z = master.wrapping_add(tag).wrapping_add(0x9E3779B97F4A7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

pub(super) fn seed_bytes(seed: u64) -> [u8; 32] {
    let mut out = [0u8; 32];
    let mut x = seed;
    for (i, chunk) in out.as_chunks_mut::<8>().0.iter_mut().enumerate() {
        x = derive_seed(x, i as u64 + 1);
        *chunk = x.to_le_bytes();
    }
    out
}

/// Hydrate production-graph traces as typed chainsync / header-validation values.
pub(super) fn fragment_trace_guards() -> amaru_pure_stage::DeserializerGuards {
    let mut guards = amaru_protocols::deserializers::register_deserializers();
    guards.push(register_data_deserializer::<amaru_consensus::stages::track_peers::TrackPeersMsg>().boxed());
    guards.push(register_data_deserializer::<amaru_protocols::chainsync::ChainSyncInitiatorMsg>().boxed());
    guards.push(register_effect_deserializer::<amaru_consensus::effects::ValidateHeaderEffect>().boxed());
    guards
}

pub(super) fn peer_trace(world: &WorldLoop, graph: usize) -> Vec<TraceEntry> {
    world.graphs()[graph].trace_buffer().lock().hydrate_without_timestamps()
}

pub(super) fn peer_saw_roll_forward(world: &WorldLoop, graph: usize, hash: &amaru_kernel::HeaderHash) -> bool {
    peer_trace(world, graph).iter().any(|entry| entry_chainsync_roll_forward_hash(entry) == Some(*hash))
}

pub(super) fn tm_chainsync_roll_forward_of(hash: amaru_kernel::HeaderHash) -> TraceMatch<'static> {
    TraceMatch::Property(
        Box::new(move |entry| entry_chainsync_roll_forward_hash(entry) == Some(hash)),
        format!("chainsync RollForward {hash}"),
    )
}

fn header_from_content(content: &amaru_protocols::chainsync::HeaderContent) -> Option<amaru_kernel::Header> {
    amaru_kernel::from_cbor(&content.cbor)
}

fn roll_forward_hash_from_result(
    msg: &amaru_protocols::chainsync::InitiatorResult,
) -> Option<amaru_kernel::HeaderHash> {
    match msg {
        amaru_protocols::chainsync::InitiatorResult::RollForward(content, _) => {
            header_from_content(content).map(|h| amaru_kernel::IsHeader::hash(&h))
        }
        amaru_protocols::chainsync::InitiatorResult::Initialize
        | amaru_protocols::chainsync::InitiatorResult::IntersectFound(_, _)
        | amaru_protocols::chainsync::InitiatorResult::IntersectNotFound(_)
        | amaru_protocols::chainsync::InitiatorResult::RollBackward(_, _)
        | amaru_protocols::chainsync::InitiatorResult::Terminated => None,
    }
}

pub(super) fn send_data_chainsync_initiator_kind(data: &dyn amaru_pure_stage::SendData) -> Option<String> {
    use amaru_consensus::stages::track_peers::TrackPeersMsg;
    use amaru_protocols::chainsync::{ChainSyncInitiatorMsg, InitiatorResult};

    let msg = if let Ok(msg) = data.cast_ref::<ChainSyncInitiatorMsg>() {
        &msg.msg
    } else if let Ok(TrackPeersMsg::FromUpstream(msg)) = data.cast_ref::<TrackPeersMsg>() {
        &msg.msg
    } else {
        return None;
    };
    Some(match msg {
        InitiatorResult::Initialize => "Initialize".to_string(),
        InitiatorResult::IntersectFound(current, tip) => format!("IntersectFound current={current} tip={tip}"),
        InitiatorResult::IntersectNotFound(tip) => format!("IntersectNotFound tip={tip}"),
        InitiatorResult::RollForward(content, tip) => {
            let hash = header_from_content(content).map(|h| h.hash()).map(|h| h.to_string()).unwrap_or_default();
            format!("RollForward {hash} tip={tip}")
        }
        InitiatorResult::RollBackward(current, tip) => format!("RollBackward current={current} tip={tip}"),
        InitiatorResult::Terminated => "Terminated".to_string(),
    })
}

pub(super) fn entry_chainsync_initiator_kind(entry: &TraceEntry) -> Option<String> {
    match entry {
        TraceEntry::Suspend(Effect::Send { msg, .. }) => send_data_chainsync_initiator_kind(msg.as_ref()),
        TraceEntry::Input { input, .. } => send_data_chainsync_initiator_kind(input.as_ref()),
        TraceEntry::Suspend(_)
        | TraceEntry::Resume { .. }
        | TraceEntry::Clock(_)
        | TraceEntry::State { .. }
        | TraceEntry::Terminated { .. }
        | TraceEntry::InvalidBytes(..) => None,
    }
}

fn send_data_chainsync_roll_forward_hash(data: &dyn amaru_pure_stage::SendData) -> Option<amaru_kernel::HeaderHash> {
    use amaru_consensus::stages::track_peers::TrackPeersMsg;
    use amaru_protocols::chainsync::ChainSyncInitiatorMsg;

    if let Ok(msg) = data.cast_ref::<ChainSyncInitiatorMsg>() {
        return roll_forward_hash_from_result(&msg.msg);
    }
    if let Ok(TrackPeersMsg::FromUpstream(msg)) = data.cast_ref::<TrackPeersMsg>() {
        return roll_forward_hash_from_result(&msg.msg);
    }
    None
}

pub(super) fn entry_chainsync_roll_forward_hash(entry: &TraceEntry) -> Option<amaru_kernel::HeaderHash> {
    match entry {
        TraceEntry::Suspend(Effect::Send { msg, .. }) => send_data_chainsync_roll_forward_hash(msg.as_ref()),
        TraceEntry::Input { input, .. } => send_data_chainsync_roll_forward_hash(input.as_ref()),
        TraceEntry::Suspend(_)
        | TraceEntry::Resume { .. }
        | TraceEntry::Clock(_)
        | TraceEntry::State { .. }
        | TraceEntry::Terminated { .. }
        | TraceEntry::InvalidBytes(..) => None,
    }
}

pub(super) fn tm_chainsync_roll_forward() -> TraceMatch<'static> {
    TraceMatch::Property(
        Box::new(|entry| entry_chainsync_roll_forward_hash(entry).is_some()),
        "chainsync RollForward".to_string(),
    )
}

pub(super) fn tm_validate_header() -> TraceMatch<'static> {
    tm_external_effect_any::<amaru_consensus::effects::ValidateHeaderEffect>()
}

pub(super) fn entry_is_validate_header_of(entry: &TraceEntry, hash: &amaru_kernel::HeaderHash) -> bool {
    let TraceEntry::Suspend(Effect::External { effect, .. }) = entry else {
        return false;
    };
    effect
        .cast_ref::<amaru_consensus::effects::ValidateHeaderEffect>()
        .is_some_and(|typed| typed.header().hash() == *hash)
}
