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

use amaru_kernel::{BlockHeight, Header, HeaderHash, IsHeader, Point};
use amaru_protocols::store_effects::Store;

use crate::{
    effects::{Ledger, LedgerOps},
    errors::ConsensusError,
};

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum HeaderLinkError {
    #[error("header does not reference the expected parent")]
    Parent,
    #[error("invalid header height {actual}, expected {expected}")]
    Height { actual: BlockHeight, expected: BlockHeight },
}

/// Check the parent reference and consecutive height before validating a header.
/// Sources normalize their protocol's representation of the origin parent before calling.
pub fn validate_header_link(
    parent_hash: Option<HeaderHash>,
    height: BlockHeight,
    parent: Point,
) -> Result<(), HeaderLinkError> {
    if parent_hash != Some(parent.hash()) {
        return Err(HeaderLinkError::Parent);
    }
    let expected = parent.block_height() + 1;
    if height != expected {
        return Err(HeaderLinkError::Height { actual: height, expected });
    }
    Ok(())
}

/// Validate a header once and durably store it with its evolved nonces.
/// Returns whether the header was newly stored.
pub async fn validate_and_store_header(
    header: &Header,
    ledger: &Ledger,
    store: &Store,
) -> Result<bool, ConsensusError> {
    if store.get_nonces(&header.hash()).await.is_some() {
        return Ok(false);
    }
    let nonces = ledger
        .validate_header(header)
        .await
        .map_err(|error| ConsensusError::InvalidHeader(header.point(), Box::new(error)))?;
    store
        .store_validated_header(header, &nonces)
        .await
        .map_err(|error| ConsensusError::StoreHeaderFailed(header.hash(), error))?;
    Ok(true)
}
