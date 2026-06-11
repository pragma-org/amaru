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

use std::{
    fmt,
    sync::{Arc, Mutex},
};

use amaru_kernel::{BlockHeader, ConsensusParameters, IsHeader, Nonce, to_cbor};
use amaru_observability::trace_span;
use amaru_ouroboros_traits::{CanValidateHeaders, ChainStore, HasPools, HeaderValidationError, PoolSummary, Praos};
use anyhow::anyhow;

use crate::{
    header_validator::{assertions::assert_all, praos_chain_store::PraosChainStore},
    state::State,
    store::{HistoricalStores, Store},
};

pub struct HeaderValidator<S, HS>
where
    S: Store + Send,
    HS: HistoricalStores + Send,
{
    state: Arc<Mutex<State<S, HS>>>,
    consensus_parameters: Arc<ConsensusParameters>,
    store: Arc<dyn ChainStore>,
}

impl<S, HS> Clone for HeaderValidator<S, HS>
where
    S: Store + Send,
    HS: HistoricalStores + Send,
{
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            consensus_parameters: self.consensus_parameters.clone(),
            store: self.store.clone(),
        }
    }
}

impl<S: Store + Send, HS: HistoricalStores + Send> HeaderValidator<S, HS> {
    pub fn new(
        state: Arc<Mutex<State<S, HS>>>,
        consensus_parameters: Arc<ConsensusParameters>,
        store: Arc<dyn ChainStore>,
    ) -> anyhow::Result<Self> {
        Ok(Self { state, consensus_parameters, store })
    }

    pub fn validate(&self, header: &BlockHeader) -> Result<(), HeaderValidationError> {
        let epoch_nonce = self.evolve_nonce(header)?;
        self.check_header(header, to_cbor(&header.header_body()).as_slice(), &epoch_nonce)?;
        Ok(())
    }

    fn evolve_nonce(&self, header: &BlockHeader) -> Result<Nonce, HeaderValidationError> {
        let _span =
            trace_span!(amaru_observability::amaru::consensus::validate_header::EVOLVE_NONCE, hash = header.hash());
        let _guard = _span.enter();
        let nonces = PraosChainStore::new(self.consensus_parameters.clone(), self.store.clone())
            .evolve_nonce(header)
            .map_err(|e| HeaderValidationError::new(anyhow!(e)))?;
        Ok(nonces.active)
    }

    fn check_header(
        &self,
        header: &BlockHeader,
        raw_header_body: &[u8],
        epoch_nonce: &Nonce,
    ) -> Result<(), HeaderValidationError> {
        let _span = trace_span!(
            amaru_observability::amaru::consensus::validate_header::VALIDATE,
            issuer_key = &header.header_body().issuer_vkey
        );
        let _guard = _span.enter();
        assert_all(self, self.consensus_parameters.clone(), header.header(), raw_header_body, epoch_nonce)
            .and_then(|assertions| {
                use rayon::prelude::*;
                assertions.into_par_iter().try_for_each(|assert| assert())
            })
            .map_err(|e| HeaderValidationError::new(anyhow!(e)))
    }
}

impl<S, HS> fmt::Debug for HeaderValidator<S, HS>
where
    S: Store + Send,
    HS: HistoricalStores + Send,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HeaderValidator")
            .field("store", &"Arc<dyn ChainStore<H>>")
            .field("state", &"Arc<Mutex<State<S, HS>>>")
            .finish()
    }
}

impl<S: Store + Send, HS: HistoricalStores + Send> HasPools for HeaderValidator<S, HS> {
    #[expect(clippy::unwrap_used)]
    fn get_pool_summary(
        &self,
        slot: amaru_kernel::Slot,
        pool_id: &amaru_kernel::PoolId,
    ) -> Result<Option<PoolSummary>, amaru_ouroboros_traits::pools::GetPoolError> {
        self.state.lock().unwrap().get_pool_summary(slot, pool_id)
    }
}

impl<S: Store + Send, HS: HistoricalStores + Send> CanValidateHeaders for HeaderValidator<S, HS> {
    fn validate_header(&self, header: &BlockHeader) -> Result<(), HeaderValidationError> {
        self.validate(header)
    }
}
