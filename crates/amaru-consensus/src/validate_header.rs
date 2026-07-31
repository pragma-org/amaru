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

use amaru_kernel::{BlockHeader, ConsensusParameters, Epoch, EraHistory, IsHeader, to_cbor};
use amaru_observability::debug_span;
use amaru_ouroboros::praos::{self, header::AssertHeaderError};
use amaru_ouroboros_traits::{ChainStore, Nonces, PoolSummaries, Praos, has_stake_distribution::GetPoolError};

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

impl ValidateHeaderError {
    pub fn missing_stake_distribution(&self) -> Option<Epoch> {
        match self {
            ValidateHeaderError::Consensus(ConsensusError::GetPoolError(
                GetPoolError::StakeDistributionNotAvailable(_, Some(target)),
            )) => Some(*target),
            ValidateHeaderError::Nonces(_) | ValidateHeaderError::Assert(_) | ValidateHeaderError::Consensus(_) => None,
        }
    }
}

/// Validate a block header and return its evolved nonces.
///
/// This is the core implementation, intended to be called from within an external effect
/// (ValidateHeaderEffect) so that up-to-date resources (in particular PoolSummaries) can be
/// obtained on each use.
///
/// Nothing is written to the store here. The returned nonces are meant to be stored atomically
/// with the header itself, so that stored nonces always denote a fully validated header.
#[allow(clippy::result_large_err)]
pub fn validate_header(
    header: &BlockHeader,
    consensus_parameters: Arc<ConsensusParameters>,
    store: Arc<dyn ChainStore>,
    pool_summaries: Arc<PoolSummaries>,
    era_history: Arc<EraHistory>,
) -> Result<Nonces, ValidateHeaderError> {
    let _span = debug_span!(consensus::header::VALIDATE, header_hash = &header.hash()).entered();
    let _guard = _span.enter();

    let nonces = debug_span!(consensus::header::EVOLVE_NONCE, header_hash = header.hash())
        .in_scope(|| PraosChainStore::new(consensus_parameters.clone(), store.clone()).evolve_nonce(header))?;

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
            &nonces.active,
        )
        .and_then(|assertions| {
            use rayon::prelude::*;
            assertions.into_par_iter().try_for_each(|assert| assert())
        })
        .map_err(ValidateHeaderError::Assert)
    })?;

    Ok(nonces)
}

#[cfg(test)]
mod test {
    use amaru_kernel::{Epoch, PREPROD_ERA_HISTORY, PREPROD_GLOBAL_PARAMETERS, hash};
    use amaru_ouroboros_traits::{
        BaseReadChainStore, Nonces, WriteChainStore, in_memory_chain_store::InMemoryChainStore,
    };

    use super::*;
    use crate::test::include_header;

    include_header!(PREPROD_HEADER_69638382, 69638382);
    include_header!(PREPROD_HEADER_70070331, 70070331);
    include_header!(PREPROD_HEADER_70070379, 70070379);

    /// Nonces are only made durable by the caller, so a header failing the Praos checks leaves
    /// nothing behind: stored nonces always denote a fully validated header.
    #[test]
    fn no_nonces_are_stored_for_an_invalid_header() {
        let store = Arc::new(InMemoryChainStore::default());
        store.store_header(&PREPROD_HEADER_69638382).expect("database failure");
        store
            .put_nonces(
                &PREPROD_HEADER_70070331.hash(),
                &Nonces {
                    epoch: Epoch::from(165),
                    active: hash!("a7c4477e9fcfd519bf7dcba0d4ffe35a399125534bc8c60fa89ff6b50a060a7a"),
                    candidate: hash!("74fe03b10c4f52dd41105a16b5f6a11015ec890a001a5253db78a779fe43f6b6"),
                    evolving: hash!("9b945f3c45b140f796f0d2ec81c48b50730044bf75eb7208c85f6195f68e9b8c"),
                    tail: hash!("5da6ba37a4a07df015c4ea92c880e3600d7f098b97e73816f8df04bbb5fad3b7"),
                },
            )
            .expect("database failure");

        let header = &*PREPROD_HEADER_70070379;
        let consensus_parameters =
            Arc::new(ConsensusParameters::new(PREPROD_GLOBAL_PARAMETERS.clone(), &PREPROD_ERA_HISTORY));

        // No stake distribution is available, so the header cannot be validated.
        let result = validate_header(
            header,
            consensus_parameters,
            store.clone(),
            Arc::new(PoolSummaries::default()),
            Arc::new(PREPROD_ERA_HISTORY.clone()),
        );

        assert!(result.is_err(), "the header should not validate");
        assert_eq!(store.get_nonces(&header.hash()), None);
    }
}
