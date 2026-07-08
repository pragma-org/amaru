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

use std::sync::Arc;

use amaru_kernel::{BlockHeader, ConsensusParameters, EraHistory, IsHeader, to_cbor};
use amaru_observability::debug_span;
use amaru_ouroboros::praos::{self, header::AssertHeaderError};
use amaru_ouroboros_traits::{ChainStore, PoolSummaries, Praos};

use crate::{
    errors::ConsensusError,
    store::{NoncesError, PraosChainStore},
};

#[derive(Debug, thiserror::Error, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ValidateHeaderError {
    #[error("evolve_nonce failed: {0}")]
    Nonces(#[from] NoncesError),
    #[error("header validation failed: {0}")]
    Assert(#[from] AssertHeaderError),
    #[error("{0}")]
    Consensus(#[from] ConsensusError),
}

/// Validate a block header.
///
/// This is the core implementation, intended to be called from within an external effect
/// (ValidateHeaderEffect) so that up-to-date resources (in particular PoolSummaries) can be
/// obtained on each use.
#[allow(clippy::result_large_err)]
pub fn validate_header(
    header: &BlockHeader,
    consensus_parameters: Arc<ConsensusParameters>,
    store: Arc<dyn ChainStore>,
    pool_summaries: Arc<PoolSummaries>,
    era_history: Arc<EraHistory>,
) -> Result<(), ValidateHeaderError> {
    let _span = debug_span!(consensus::header::VALIDATE, header_hash = &header.hash()).entered();
    let _guard = _span.enter();

    let epoch_nonce = debug_span!(consensus::header::EVOLVE_NONCE, header_hash = header.hash())
        .in_scope(|| PraosChainStore::new(consensus_parameters.clone(), store.clone()).evolve_nonce(header))?
        .active;

    debug_span!(consensus::header::CHECK, issuer_key = &header.header_body().issuer_vkey).in_scope(|| {
        let pool_id = header.pool_id();
        let last_opcert_sequence_number =
            store.get_latest_opcert_sequence_number(&pool_id, header).map_err(ConsensusError::StoreError)?;

        let pool_summary = pool_summaries
            .get_pool(header.slot(), &pool_id, era_history.as_ref())
            .map_err(ConsensusError::GetPoolError)?
            .ok_or(ConsensusError::UnknownPool { pool_id })?;

        praos::header::assert_all(
            consensus_parameters,
            header.header(),
            to_cbor(&header.header_body()).as_slice(),
            last_opcert_sequence_number,
            &pool_summary,
            &epoch_nonce,
        )
        .and_then(|assertions| {
            use rayon::prelude::*;
            assertions.into_par_iter().try_for_each(|assert| assert())
        })
        .map_err(ValidateHeaderError::Assert)
    })?;

    Ok(())
}
