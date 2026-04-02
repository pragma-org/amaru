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

use std::{fmt, sync::Arc};

use amaru_kernel::{BlockHeader, ConsensusParameters, IsHeader, Nonce, to_cbor};
use amaru_observability::trace;
use amaru_ouroboros::praos;
use amaru_ouroboros_traits::{
    ChainStore, HasStakeDistribution, Praos,
    can_validate_blocks::{CanValidateHeaders, HeaderValidationError},
};
use anyhow::anyhow;

use crate::{errors::ConsensusError, store::PraosChainStore};

pub struct ValidateHeader {
    consensus_parameters: Arc<ConsensusParameters>,
    store: Arc<dyn ChainStore<BlockHeader>>,
    ledger: Arc<dyn HasStakeDistribution>,
}

impl fmt::Debug for ValidateHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ValidateHeader")
            .field("store", &"Arc<dyn ChainStore<H>>")
            .field("ledger", &"Arc<dyn HasStakeDistribution>")
            .finish()
    }
}

impl ValidateHeader {
    pub fn new(
        consensus_parameters: Arc<ConsensusParameters>,
        store: Arc<dyn ChainStore<BlockHeader>>,
        ledger: Arc<dyn HasStakeDistribution>,
    ) -> Self {
        Self { consensus_parameters, store, ledger }
    }

    pub fn validate(&self, header: &BlockHeader) -> Result<(), ConsensusError> {
        let epoch_nonce = self.evolve_nonce(header)?;
        self.check_header(header, to_cbor(&header.header_body()).as_slice(), &epoch_nonce)?;
        Ok(())
    }

    #[trace(amaru::consensus::validate_header::EVOLVE_NONCE, hash = header.hash())]
    fn evolve_nonce(&self, header: &BlockHeader) -> Result<Nonce, ConsensusError> {
        let nonces =
            PraosChainStore::new(self.consensus_parameters.clone(), self.store.clone()).evolve_nonce(header)?;
        Ok(nonces.active)
    }

    #[trace(amaru::consensus::validate_header::VALIDATE, issuer_key = header.header_body().issuer_vkey)]
    fn check_header(
        &self,
        header: &BlockHeader,
        raw_header_body: &[u8],
        epoch_nonce: &Nonce,
    ) -> Result<(), ConsensusError> {
        praos::header::assert_all(
            self.consensus_parameters.clone(),
            header.header(),
            raw_header_body,
            self.ledger.clone(),
            epoch_nonce,
        )
        .and_then(|assertions| {
            use rayon::prelude::*;
            assertions.into_par_iter().try_for_each(|assert| assert())
        })
        .map_err(|e| ConsensusError::InvalidHeader(header.point(), HeaderValidationError::new(anyhow!(e))))
    }
}

impl CanValidateHeaders for ValidateHeader {
    fn validate_header(&self, header: &BlockHeader) -> Result<(), HeaderValidationError> {
        self.validate(header).map_err(|e| HeaderValidationError::new(anyhow!(e)))
    }
}
