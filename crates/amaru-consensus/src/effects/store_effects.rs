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

use amaru_kernel::{HeaderHash, IsHeader, ORIGIN_HASH};
use amaru_ouroboros::ReadChainStore;
use amaru_protocols::store_effects::ResourceHeaderStore;
use pure_stage::{BoxFuture, ExternalEffect, ExternalEffectAPI, Resources, SendData};

use crate::{errors::ConsensusError, stages::select_chain::cmp_tip};

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FindBestCandidate;

impl ExternalEffect for FindBestCandidate {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        #[expect(clippy::expect_used)]
        Self::wrap_sync_f(|| {
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("FindBestCandidate requires a ResourceHeaderStore")
                .clone();
            find_best_candidate(store.as_ref())
        })
    }
}

pub fn find_best_candidate(store: &dyn ReadChainStore) -> Result<HeaderHash, ConsensusError> {
    let anchor_hash = store.get_anchor_hash();
    let mut best_candidate = None;

    // ORIGIN_HASH cannot have a block, so we start from its direct children
    let mut to_visit = if anchor_hash == ORIGIN_HASH {
        store.get_children(&anchor_hash).into_iter().collect()
    } else {
        best_candidate = store.load_header(&anchor_hash);
        if best_candidate.is_none() {
            return Err(ConsensusError::UnknownPoint(anchor_hash));
        }
        vec![anchor_hash]
    };
    tracing::debug!(?to_visit, ?best_candidate, "starting best_tip_from_store");

    while let Some(hash) = to_visit.pop() {
        let (header, validity) = store.load_header_with_validity(&hash).ok_or(ConsensusError::UnknownPoint(hash))?;

        if validity == Some(false) {
            tracing::debug!(%hash, "skipping invalid");
            continue;
        };

        let children = store.get_children(&hash);

        if cmp_tip(Some(&header), best_candidate.as_ref()).is_gt() {
            best_candidate = Some(header);
        }
        tracing::debug!(?children, "extending to_visit");
        to_visit.extend(children);
    }

    Ok(best_candidate.map(|h| h.hash()).unwrap_or(ORIGIN_HASH))
}

impl ExternalEffectAPI for FindBestCandidate {
    type Response = Result<HeaderHash, ConsensusError>;
}
