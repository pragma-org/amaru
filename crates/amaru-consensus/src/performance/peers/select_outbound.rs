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

//! Mix allotment and quality-weighted outbound picks.

use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use amaru_kernel::PeerCandidate;
use amaru_pure_stage::Instant;
use rand::{Rng, SeedableRng, rngs::StdRng};

use super::{
    PeerPerformance,
    peer_mix::{PeerMix, PeerSource},
    quality::{PeerScores, rank_score},
    reputation::malus_at,
};

/// Scale of malus in outbound score: `goodness - λ * malus`.
pub const OUTBOUND_MALUS_LAMBDA: f64 = 1.0;

/// Softmax temperature for weighted outbound sampling.
pub const OUTBOUND_PICK_TEMPERATURE: f64 = 1.0;

/// Bonus for peers with no Performance observation yet (never connected / unknown).
pub const NEVER_CONNECTED_BONUS: f64 = 0.5;

/// Parameters for mix + quality Using selection (outbound dials and inbound promotions).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SelectOutboundParams {
    /// How many new Using slots to fill (typically `target − occupancy`).
    pub open: usize,
    /// Candidates that must not be picked (already outbound, cooling down, or resolving).
    pub excluded: BTreeSet<PeerCandidate>,
    /// Duplex inbound connections that could be promoted to Using.
    pub eligible_inbound: usize,
    /// Deterministic RNG seed from peer selection’s random effect.
    pub seed: [u8; 32],
    pub now: Instant,
}

/// Mix result: inbound Using promotions plus outbound dials.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SelectUsing {
    /// How many duplex inbound connections to promote to Using.
    pub inbound: usize,
    pub outbound: Vec<OutboundPick>,
}

/// One mix pick: a [`PeerCandidate`] from a named source, not yet resolved to a dial address.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OutboundPick {
    pub candidate: PeerCandidate,
    pub origin: PeerSource,
}

/// Rows copied off the worker before mix sampling.
#[derive(Clone, Debug, PartialEq)]
pub struct OutboundInputs {
    pub mix: PeerMix,
    pub sources: Vec<OutboundSourceInputs>,
}

/// Eligible candidates of one source, in pool order, with the fields sampling needs.
#[derive(Clone, Debug, PartialEq)]
pub struct OutboundSourceInputs {
    pub source: PeerSource,
    pub candidates: Vec<OutboundCandidateInput>,
}

/// Score inputs for one candidate. The weight is computed by the caller, not the worker.
#[derive(Clone, Debug, PartialEq)]
pub struct OutboundCandidateInput {
    pub candidate: PeerCandidate,
    pub score: OutboundScoreInput,
}

/// How a candidate's outbound weight is derived. Mirrors the three branches of the worker lookup.
#[derive(Clone, Debug, PartialEq)]
pub enum OutboundScoreInput {
    /// Host or SRV name with no resolved address yet.
    Unresolved,
    /// Resolved address that has no performance row.
    NoRow,
    /// Resolved address with a performance row.
    Observed {
        scores: PeerScores,
        malus: f64,
        malus_as_of: Option<Instant>,
        half_life: Duration,
        never_connected: bool,
    },
}

struct OutboundWeight {
    candidate: PeerCandidate,
    score: f64,
    weight: f64,
}

/// Fill `n` slots from best score to worse. Ties are a weighted draw; a worse score is used
/// only after every better score in the bucket has been taken.
fn fill_by_worsening_score(rng: &mut StdRng, mut weights: Vec<OutboundWeight>, n: usize) -> Vec<PeerCandidate> {
    if n == 0 || weights.is_empty() {
        return Vec::new();
    }
    weights
        .sort_by(|left, right| right.score.total_cmp(&left.score).then_with(|| left.candidate.cmp(&right.candidate)));
    let mut out = Vec::with_capacity(n.min(weights.len()));
    let mut index = 0;
    while out.len() < n && index < weights.len() {
        let score = weights[index].score;
        let mut end = index + 1;
        while end < weights.len() && weights[end].score.total_cmp(&score).is_eq() {
            end += 1;
        }
        let group = weights[index..end].iter().map(|weight| (weight.candidate.clone(), weight.weight)).collect();
        let take = (n - out.len()).min(end - index);
        out.extend(weighted_sample_without_replacement(rng, group, take));
        index = end;
    }
    out
}

fn weighted_sample_without_replacement<T: Clone>(rng: &mut StdRng, mut items: Vec<(T, f64)>, n: usize) -> Vec<T> {
    let mut out = Vec::with_capacity(n.min(items.len()));
    for _ in 0..n {
        if items.is_empty() {
            break;
        }
        let total: f64 = items.iter().map(|(_, w)| *w).filter(|w| w.is_finite() && *w > 0.0).sum();
        let idx = if total <= 0.0 || !total.is_finite() {
            rng.random_range(0..items.len())
        } else {
            let mut r = rng.random::<f64>() * total;
            let mut chosen = items.len() - 1;
            for (i, (_, w)) in items.iter().enumerate() {
                if *w <= 0.0 || !w.is_finite() {
                    continue;
                }
                r -= w;
                if r <= 0.0 {
                    chosen = i;
                    break;
                }
            }
            chosen
        };
        out.push(items.swap_remove(idx).0);
    }
    out
}

fn outbound_goodness(scores: Option<&PeerScores>) -> f64 {
    // Reuse fetch-oriented heuristics with need_len=1 (no fragment context for dial).
    rank_score(scores, 1)
}

fn outbound_sampling_weight(score: f64) -> f64 {
    let t = OUTBOUND_PICK_TEMPERATURE.max(f64::EPSILON);
    let w = (score / t).exp();
    if w.is_finite() && w > 0.0 { w } else { f64::EPSILON }
}

impl PeerPerformance {
    /// Copy mix rows and per-candidate score inputs. Does not sample.
    pub fn outbound_inputs(&self, excluded: &BTreeSet<PeerCandidate>) -> OutboundInputs {
        let mut sources = Vec::new();
        for entry in self.peer_mix.entries() {
            if entry.source.is_inbound() {
                continue;
            }
            let candidates = self
                .eligible_for_source(entry.source, excluded)
                .into_iter()
                .map(|candidate| OutboundCandidateInput { score: self.score_input(&candidate), candidate })
                .collect();
            sources.push(OutboundSourceInputs { source: entry.source, candidates });
        }
        OutboundInputs { mix: self.peer_mix.clone(), sources }
    }

    /// Mix allotment: inbound Using promotions plus quality-weighted outbound dials.
    pub fn select_outbound(&self, params: SelectOutboundParams) -> SelectUsing {
        select_outbound_from(&self.outbound_inputs(&params.excluded), &params)
    }

    fn score_input(&self, candidate: &PeerCandidate) -> OutboundScoreInput {
        let Some(peer) = candidate.as_peer().or_else(|| self.last_peer.get(candidate).copied()) else {
            return OutboundScoreInput::Unresolved;
        };
        let Some(state) = self.peers.get(&peer) else {
            return OutboundScoreInput::NoRow;
        };
        // Fresh / unknown to Performance: no successful handshake yet and no scores
        // or tip activity. Failure-only stubs set `last_change` (and malus) so they
        // do not receive the never-connected exploration bonus.
        let never_connected = !state.ever_connected && state.scores.last_change.is_none() && state.tips.is_empty();
        OutboundScoreInput::Observed {
            scores: state.scores.clone(),
            malus: state.malus,
            malus_as_of: state.malus_as_of,
            half_life: self.half_life_for(&peer),
            never_connected,
        }
    }
}

/// Sample outbound dials from a copied snapshot. Same seed and inputs yield the same picks.
pub fn select_outbound_from(inputs: &OutboundInputs, params: &SelectOutboundParams) -> SelectUsing {
    if params.open == 0 {
        return SelectUsing { inbound: 0, outbound: Vec::new() };
    }
    let mut eligible_counts = BTreeMap::new();
    for entry in inputs.mix.entries() {
        if entry.source.is_inbound() {
            eligible_counts.insert(PeerSource::Inbound, params.eligible_inbound);
            continue;
        }
        let count = inputs
            .sources
            .iter()
            .find(|source| source.source == entry.source)
            .map(|source| source.candidates.len())
            .unwrap_or(0);
        eligible_counts.insert(entry.source, count);
    }
    let allotment = inputs.mix.allot(params.open, &eligible_counts);
    let inbound = allotment.get(&PeerSource::Inbound).copied().unwrap_or(0);
    if allotment.values().all(|&n| n == 0) {
        return SelectUsing { inbound: 0, outbound: Vec::new() };
    }

    let mut rng = StdRng::from_seed(params.seed);
    let mut picked = Vec::new();
    let mut already: BTreeSet<PeerCandidate> = BTreeSet::new();

    for entry in inputs.mix.entries() {
        if entry.source.is_inbound() {
            continue;
        }
        let n = allotment.get(&entry.source).copied().unwrap_or(0);
        if n == 0 {
            continue;
        }
        let Some(source) = inputs.sources.iter().find(|source| source.source == entry.source) else {
            continue;
        };
        let candidates: Vec<&OutboundCandidateInput> =
            source.candidates.iter().filter(|input| !already.contains(&input.candidate)).collect();
        if candidates.is_empty() {
            continue;
        }
        let weights = candidates
            .iter()
            .map(|input| {
                let (score, weight) = outbound_weight(&input.score, params.now);
                OutboundWeight { candidate: input.candidate.clone(), score, weight }
            })
            .collect();
        // Best score first, then worse scores, until the bucket is full. Malus makes a
        // peer less preferred; it does not drop the peer while a slot is still open.
        for candidate in fill_by_worsening_score(&mut rng, weights, n) {
            already.insert(candidate.clone());
            picked.push(OutboundPick { candidate, origin: entry.source });
        }
    }
    SelectUsing { inbound, outbound: picked }
}

fn outbound_weight(input: &OutboundScoreInput, now: Instant) -> (f64, f64) {
    let score = match input {
        OutboundScoreInput::Unresolved | OutboundScoreInput::NoRow => NEVER_CONNECTED_BONUS,
        OutboundScoreInput::Observed { scores, malus, malus_as_of, half_life, never_connected } => {
            let malus = malus_at(*malus, *malus_as_of, now, *half_life);
            let mut score = outbound_goodness(Some(scores)) - OUTBOUND_MALUS_LAMBDA * malus;
            if *never_connected {
                score += NEVER_CONNECTED_BONUS;
            }
            score
        }
    };
    (score, outbound_sampling_weight(score))
}
