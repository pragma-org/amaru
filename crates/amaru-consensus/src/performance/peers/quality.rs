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

//! Smoothed peer quality (response, lag, bandwidth, fetch counters) and the ranking functions.

use std::time::Duration;

use amaru_kernel::Peer;
use amaru_pure_stage::Instant;

use super::PeerPerformance;

const EWMA_ALPHA: f64 = 0.2;

/// Quality aggregates used for ranking.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PeerScores {
    pub header_lag_ewma: Option<Duration>,
    pub block_response_ewma: Option<Duration>,
    pub bandwidth_ewma_bps: Option<f64>,
    pub keepalive_rtt_latest: Option<Duration>,
    pub keepalive_rtt_ewma: Option<Duration>,
    pub fetch_timeouts: u32,
    pub fetch_successes: u32,
    pub last_change: Option<Instant>,
}

fn ewma_duration(prev: Option<Duration>, sample: Duration) -> Duration {
    match prev {
        None => sample,
        Some(p) => {
            let prev_secs = p.as_secs_f64();
            let sample_secs = sample.as_secs_f64();
            let next = (1.0 - EWMA_ALPHA) * prev_secs + EWMA_ALPHA * sample_secs;
            Duration::from_secs_f64(next.max(0.0))
        }
    }
}

fn ewma_f64(prev: Option<f64>, sample: f64) -> f64 {
    match prev {
        None => sample,
        Some(p) => (1.0 - EWMA_ALPHA) * p + EWMA_ALPHA * sample,
    }
}

/// Compute a "goodness" score for fetch ranking: higher is better.
///
/// NOTE: **these are made-up heuristics for now, and may be tuned or replaced with a more
/// principled approach later.**
pub(super) fn rank_score(scores: Option<&PeerScores>, need_len: usize) -> f64 {
    let scores = scores.cloned().unwrap_or_default();
    let mut score = 0.0;

    let response_penalty = scores.block_response_ewma.map(|d| (d.as_secs_f64() + 1.0).ln()).unwrap_or(0.0);
    score -= 2.0 * response_penalty;

    let lag_penalty = scores.header_lag_ewma.map(|d| (d.as_secs_f64() + 1.0).ln()).unwrap_or(0.0);
    score -= if need_len <= 1 { 3.0 } else { 1.0 } * lag_penalty;

    if need_len >= 5
        && let Some(bps) = scores.bandwidth_ewma_bps
    {
        score += (bps.max(1.0)).ln();
    }

    let attempts = scores.fetch_successes.saturating_add(scores.fetch_timeouts);
    if attempts > 0 {
        let rate = scores.fetch_timeouts as f64 / attempts as f64;
        score -= 5.0 * rate;
    }

    score
}

/// Compute a "badness" score for churn ranking: higher is worse.
///
/// NOTE: **these are made-up heuristics for now, and may be tuned or replaced with a more
/// principled approach later.**
fn churn_badness(scores: &PeerScores) -> f64 {
    let mut bad = 0.0;
    let attempts = scores.fetch_successes.saturating_add(scores.fetch_timeouts);
    if attempts > 0 {
        bad += 10.0 * (scores.fetch_timeouts as f64 / attempts as f64);
    }
    if let Some(lag) = scores.header_lag_ewma {
        bad += (lag.as_secs_f64() + 1.0).ln();
    }
    if let Some(resp) = scores.block_response_ewma {
        bad += (resp.as_secs_f64() + 1.0).ln();
    }
    if let Some(bps) = scores.bandwidth_ewma_bps {
        bad -= (bps.max(1.0)).ln() * 0.1;
    }
    bad
}

impl PeerPerformance {
    pub fn record_fetch_failure(&mut self, peers: &[Peer], at: Instant) {
        for &peer in peers {
            {
                let state = self.peers.entry(peer).or_default();
                state.scores.fetch_timeouts = state.scores.fetch_timeouts.saturating_add(1);
                state.scores.last_change = Some(at);
            }
            self.note_activity(peer, at);
        }
    }

    pub fn record_keepalive_rtt(&mut self, peer: Peer, rtt: Duration, at: Instant) {
        {
            let state = self.peers.entry(peer).or_default();
            state.scores.keepalive_rtt_latest = Some(rtt);
            state.scores.keepalive_rtt_ewma = Some(ewma_duration(state.scores.keepalive_rtt_ewma, rtt));
            state.scores.last_change = Some(at);
        }
        self.note_activity(peer, at);
    }

    /// Copy the rows churn ranking needs. Ordering is the caller's.
    pub fn churn_inputs(&self, candidates: &[Peer]) -> Vec<ChurnInput> {
        candidates
            .iter()
            .map(|peer| ChurnInput { peer: *peer, scores: self.scores(peer), is_static: self.is_static_peer(peer) })
            .collect()
    }

    /// Rank candidates for churn (worst first). `now` is reserved for future staleness ranking.
    pub fn rank_peers_for_churn(&self, candidates: &[Peer], _now: Instant) -> Vec<ChurnRank> {
        rank_churn(self.churn_inputs(candidates))
    }

    pub fn scores(&self, peer: &Peer) -> PeerScores {
        self.peers.get(peer).map(|s| s.scores.clone()).unwrap_or_default()
    }

    pub(super) fn update_header_lag(&mut self, peer: &Peer, lag: Duration, at: Instant) {
        {
            let state = self.peers.entry(*peer).or_default();
            state.scores.header_lag_ewma = Some(ewma_duration(state.scores.header_lag_ewma, lag));
            state.scores.last_change = Some(at);
        }
        self.note_activity(*peer, at);
    }

    pub(super) fn update_block_delivery(&mut self, peer: &Peer, response: Duration, bytes: u64, at: Instant) {
        {
            let state = self.peers.entry(*peer).or_default();
            state.scores.block_response_ewma = Some(ewma_duration(state.scores.block_response_ewma, response));
            let secs = response.as_secs_f64().max(1e-6);
            let bps = bytes as f64 / secs;
            state.scores.bandwidth_ewma_bps = Some(ewma_f64(state.scores.bandwidth_ewma_bps, bps));
            state.scores.fetch_successes = state.scores.fetch_successes.saturating_add(1);
            state.scores.last_change = Some(at);
        }
        self.note_activity(*peer, at);
    }
}

/// Scores and static-ness copied off the worker before churn ranking.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ChurnInput {
    pub peer: Peer,
    pub scores: PeerScores,
    pub is_static: bool,
}

/// One peer after churn ranking (worst first).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ChurnRank {
    pub peer: Peer,
    pub scores: PeerScores,
    pub is_static: bool,
}

/// Rank copied rows, worst first. Ties break toward the smaller peer address.
pub fn rank_churn(inputs: Vec<ChurnInput>) -> Vec<ChurnRank> {
    let mut ranked: Vec<(f64, ChurnInput)> = inputs
        .into_iter()
        .map(|input| {
            let badness = churn_badness(&input.scores);
            (badness, input)
        })
        .collect();
    ranked.sort_by(|left, right| {
        right.0.partial_cmp(&left.0).unwrap_or(std::cmp::Ordering::Equal).then_with(|| left.1.peer.cmp(&right.1.peer))
    });
    ranked
        .into_iter()
        .map(|(_, input)| ChurnRank { peer: input.peer, scores: input.scores, is_static: input.is_static })
        .collect()
}
