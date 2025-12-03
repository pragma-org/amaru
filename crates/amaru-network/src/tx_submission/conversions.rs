// Copyright 2025 PRAGMA
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

use amaru_kernel::Hash;
use amaru_kernel::tx_submission_events::TxId;
use pallas_network::miniprotocols::txsubmission::{EraTxBody, EraTxId};
use pallas_traverse::Era;

/// Retrieves the TxId from an EraTxId.
pub fn tx_id_from_era_tx_id(era_tx_id: &EraTxId) -> TxId {
    TxId::new(Hash::from(era_tx_id.1.as_slice()))
}

/// Create a new EraTxId for the Conway era.
pub fn new_era_tx_id(tx_id: TxId) -> EraTxId {
    EraTxId(Era::Conway.into(), tx_id.to_vec())
}

/// Create a new EraTxBody for the Conway era.
pub fn new_era_tx_body(tx_body: Vec<u8>) -> EraTxBody {
    EraTxBody(Era::Conway.into(), tx_body)
}

pub fn era_tx_id_to_string(era_tx_id: &EraTxId) -> String {
    Hash::<32>::from(era_tx_id.1.as_slice()).to_string()
}

pub fn era_tx_body_to_string(era_tx_body: &EraTxBody) -> String {
    String::from_utf8_lossy(&era_tx_body.1).to_string()
}
