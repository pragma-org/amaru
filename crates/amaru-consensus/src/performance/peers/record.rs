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

//! Per-peer row: tips, scores, share flags, and connection malus.

use std::collections::BTreeMap;

use amaru_kernel::HeaderHash;
use amaru_pure_stage::Instant;

use super::{claims::ClaimMeta, quality::PeerScores, reputation::PeerShareFlags};

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub(super) struct PeerState {
    pub(super) tips: BTreeMap<HeaderHash, ClaimMeta>,
    pub(super) scores: PeerScores,
    /// Sticky once a successful handshake is observed; not set by connection-failure upserts.
    pub(super) ever_connected: bool,
    /// Latest handshake peer-sharing willingness; latest successful handshake wins.
    pub(super) advertisable: bool,
    /// Connection / protocol failures (distinct from blockfetch `fetch_timeouts`); telemetry.
    pub(super) failure_count: u32,
    /// Sticky once set by [`PeerPerformance::mark_adversarial`]; not cleared by clear/availability.
    /// Permanent for peer-sharing filters only.
    pub(super) adversarial: bool,
    /// Connection-reputation malus at [`Self::malus_as_of`] (lazy exponential decay).
    pub(super) malus: f64,
    /// Instant when [`Self::malus`] was last evolved for storage.
    pub(super) malus_as_of: Option<Instant>,
    /// Keep this reputation stub until then. Set by an adversarial mark and extended while the
    /// peer stays protected.
    pub(super) stub_until: Option<Instant>,
}

impl PeerState {
    pub(super) fn share_flags(&self) -> PeerShareFlags {
        PeerShareFlags {
            ever_connected: self.ever_connected,
            advertisable: self.advertisable,
            failure_count: self.failure_count,
            adversarial: self.adversarial,
        }
    }
}
